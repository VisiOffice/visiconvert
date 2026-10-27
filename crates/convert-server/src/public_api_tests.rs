use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;
async fn call(
    app: Shared,
    method: &str,
    path: &str,
    key: &str,
    payload: Value,
) -> (StatusCode, Value) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn token(store: &Store, workspace: Uuid) -> String {
    let user = Uuid::new_v4();
    store
        .provision_hosted(workspace, user, "Inquiry tests")
        .await
        .unwrap();
    let token = format!(
        "vc_session_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let code = Uuid::new_v4().to_string();
    assert!(
        store
            .issue_hosted_code(workspace, user, "browser", &code, 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .exchange_hosted_code(&code, "browser", &hosted::digest(&token))
            .await
            .unwrap()
    );
    token
}
fn app(store: Store, cloud: Option<hosted::Config>) -> Shared {
    Arc::new(App {
        store,
        api_key: "public-test-operator-at-least-32-characters".into(),
        public_url: "https://api.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud,
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    })
}
fn input(scopes: Value) -> Value {
    json!({"name":"Automation","scopes":scopes,"expires_at":null})
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn keys_scopes_pagination_rotation_retries_and_isolation() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let app = app(store.clone(), None);
    let operator = app.api_key.clone();
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/api-keys",
        &operator,
        input(json!(["read"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let secret = created["secret"].as_str().unwrap();
    let id = created["id"].as_str().unwrap();
    let metadata = call(app.clone(), "GET", "/api/api-keys", &operator, Value::Null)
        .await
        .1;
    assert!(!metadata.to_string().contains(secret));
    assert!(!metadata.to_string().contains("secret_hash"));
    let stored: String =
        sqlx::query_scalar("SELECT secret_hash FROM workspace_api_keys WHERE id=$1")
            .bind(id.parse::<Uuid>().unwrap())
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_ne!(stored, secret);
    assert_eq!(stored, hosted::digest(secret));
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", secret, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    for path in [
        "/api/api-keys",
        "/api/bootstrap",
        "/api/conversions",
        "/internal/cloud/workspaces",
    ] {
        assert_ne!(
            call(app.clone(), "GET", path, secret, Value::Null).await.0,
            StatusCode::OK
        );
    }
    let lead = json!({"external_id":"public-1","name":"API customer","contact":"customer@example.com","source":"API","campaign":"Test","notes":""});
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/leads", secret, lead.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/api-keys",
            &operator,
            input(json!(["read", "admin"]))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, created_write) = call(
        app.clone(),
        "POST",
        "/api/api-keys",
        &operator,
        input(json!(["read", "leads:write", "bookings:write"])),
    )
    .await;
    let write = created_write["secret"].as_str().unwrap();
    let (status, added) = call(
        app.clone(),
        "POST",
        "/api/v1/interactions",
        write,
        lead.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/v1/interactions",
            write,
            lead.clone()
        )
        .await
        .1,
        added
    );
    let lead_id = added["id"].as_str().unwrap();
    let mut changed = lead;
    changed["name"] = json!("Changed");
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/interactions", write, changed)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &format!("/api/v1/leads/{lead_id}"),
            write,
            json!({"version":1,"qualification":"qualified","notes":"Shared notes"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        store
            .inquiry_history(lead_id.parse().unwrap())
            .await
            .unwrap()
            .iter()
            .any(|event| event.to_string().contains("API key"))
    );
    let booking = json!({"external_id":"JOB-API","amount_minor":12345,"currency":"USD","status":"booked","revision":1});
    for _ in 0..2 {
        assert_eq!(
            call(
                app.clone(),
                "PUT",
                &format!("/api/v1/inquiries/{lead_id}/outcomes"),
                write,
                booking.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let report = call(
        app.clone(),
        "GET",
        "/api/v1/reports/sources",
        secret,
        Value::Null,
    )
    .await
    .1;
    assert_eq!(report[0]["booked_minor"], 12345);
    // Older than 500 interactions: direct lookup and cursor pagination still work.
    sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign) SELECT gen_random_uuid(),$1,'page-'||n,'Page','page@example.com','form','Pagination','' FROM generate_series(1,505) n").bind(store.workspace).execute(&store.pool).await.unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            &format!("/api/v1/interactions/{lead_id}"),
            secret,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut cursor: Option<String> = None;
    let mut ids = std::collections::HashSet::new();
    loop {
        let mut url = url::Url::parse("http://localhost/api/v1/interactions?limit=100").unwrap();
        if let Some(c) = cursor {
            url.query_pairs_mut().append_pair("cursor", &c);
        }
        let (status, page) = call(
            app.clone(),
            "GET",
            &format!("{}?{}", url.path(), url.query().unwrap()),
            secret,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        for lead in page["data"].as_array().unwrap() {
            assert!(ids.insert(lead["id"].as_str().unwrap().to_owned()));
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 506);
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/interactions?cursor=bad",
            secret,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/interactions?limit=101",
            secret,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    let other_secret = other
        .issue_api_key(
            &serde_json::from_value(input(json!(["read"]))).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            &format!("/api/v1/leads/{lead_id}"),
            other_secret["secret"].as_str().unwrap(),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(
        other
            .revoke_api_key(id.parse().unwrap(), 1, "test")
            .await
            .is_err()
    );
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/bookings",
            other_secret["secret"].as_str().unwrap(),
            Value::Null
        )
        .await
        .1["data"],
        json!([])
    );
    let (status, rotated) = call(
        app.clone(),
        "POST",
        &format!("/api/api-keys/{id}/rotate"),
        &operator,
        json!({"version":1,"key":input(json!(["read"]))}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", secret, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let current = rotated["secret"].as_str().unwrap();
    let current_id: Uuid = rotated["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/workspace",
            current,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE workspace_api_keys SET rate_count=120,rate_started_at=now() WHERE id=$1")
        .bind(current_id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/workspace",
            current,
            Value::Null
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query(
        "UPDATE workspace_api_keys SET rate_started_at=now()-interval '61 seconds' WHERE id=$1",
    )
    .bind(current_id)
    .execute(&store.pool)
    .await
    .unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/workspace",
            current,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    store.revoke_api_key(current_id, 1, "test").await.unwrap();
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/v1/workspace",
            current,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE workspace_api_keys SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(
            created_write["id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", write, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn hosted_keys_require_billing_membership_and_current_owner_credentials() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let allowed = Arc::new(AtomicBool::new(true));
    let flag = allowed.clone();
    let cloud_router=Router::new().route("/internal/api-access",post(move |headers:HeaderMap,Json(body):Json<Value>| {let flag=flag.clone();async move {
        if headers.get("authorization").and_then(|s|s.to_str().ok())!=Some("Bearer aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"){return StatusCode::UNAUTHORIZED.into_response();}
        if !flag.load(Ordering::SeqCst){return StatusCode::FORBIDDEN.into_response();}
        Json(json!({"allowed":true,"workspace_id":body["workspace_id"],"user_id":body["user_id"],"credential_version":body["credential_version"]})).into_response()
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, cloud_router).await.unwrap() });
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let workspace = Uuid::new_v4();
    let owner = token(&store, workspace).await;
    let app = app(
        store.clone(),
        Some(hosted::Config {
            service_key: "a".repeat(64),
            cloud_origin: format!("http://{address}"),
            app_origin: None,
        }),
    );
    let (status, result) = call(
        app.clone(),
        "POST",
        "/api/api-keys",
        &owner,
        input(json!(["read"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let key = result["secret"].as_str().unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", key, Value::Null)
            .await
            .1["id"],
        workspace.to_string()
    );
    allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", key, Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/api-keys",
            &owner,
            input(json!(["read"]))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    allowed.store(true, Ordering::SeqCst);
    sqlx::query("UPDATE hosted_memberships SET enabled=false WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", key, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE hosted_memberships SET enabled=true WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    task.abort();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", key, Value::Null)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let user: Uuid =
        sqlx::query_scalar("SELECT owner_id FROM hosted_workspaces WHERE workspace_id=$1")
            .bind(workspace)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    store.revoke_hosted_user(user, 1).await.unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/workspace", key, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
#[test]
fn openapi_covers_public_routes() {
    let spec: Value = serde_json::from_str(include_str!("../assets/openapi.json")).unwrap();
    assert_eq!(spec["openapi"], "3.1.0");
    for path in [
        "/workspace",
        "/interactions",
        "/interactions/{id}",
        "/inquiries",
        "/inquiries/{id}",
        "/bookings",
        "/reports/sources",
        "/numbers",
        "/connections/twilio",
        "/connections/signalwire",
        "/connections/signalwire/numbers",
    ] {
        assert!(spec["paths"][path]["get"].is_object(), "{path}");
    }
    assert!(
        spec["paths"]["/api-keys"].is_null(),
        "Management endpoints must stay outside the integration API"
    );
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn setup_scope_sites_pool_versions_evidence_and_tenant_isolation() {
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let app = app(store.clone(), None);
    let operator = &app.api_key;
    let site = json!({"name":"Setup pilot","origin":"https://setup.example","allowed_fields":["email"],"enabled":true,"version":0});
    let mut writer = String::new();
    for scopes in [
        json!(["read"]),
        json!(["read", "leads:write", "bookings:write", "numbers:write"]),
        json!(["read", "setup:write"]),
    ] {
        let (status, key) = call(
            app.clone(),
            "POST",
            "/api/api-keys",
            operator,
            input(scopes.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{key}");
        let secret = key["secret"].as_str().unwrap();
        let (status, evidence) =
            call(app.clone(), "GET", "/api/v1/setup", secret, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{evidence}");
        assert_eq!(evidence["sites"], json!([]));
        assert_eq!(evidence["numbers"], json!([]));
        assert_eq!(evidence["google_uploads_enabled"], false);
        assert_eq!(evidence["workspace_id"], store.workspace.to_string());
        if scopes.as_array().unwrap().contains(&json!("setup:write")) {
            writer = secret.into();
        } else {
            let result = call(app.clone(), "POST", "/api/v1/sites", secret, site.clone()).await;
            assert_eq!(result.0, StatusCode::FORBIDDEN, "{:?}", result);
            assert!(result.1["error"].as_str().unwrap().contains("setup:write"));
            for (method, path) in [
                ("PATCH", format!("/api/v1/sites/{}", Uuid::new_v4())),
                (
                    "PUT",
                    format!("/api/v1/sites/{}/number-pool", Uuid::new_v4()),
                ),
            ] {
                assert_eq!(
                    call(app.clone(), method, &path, secret, site.clone())
                        .await
                        .0,
                    StatusCode::FORBIDDEN
                );
            }
        }
    }
    for (method, path) in [
        ("POST", "/api/v1/numbers"),
        ("POST", "/api/v1/leads"),
        ("PUT", "/api/connections/signalwire"),
        ("POST", "/api/api-keys"),
    ] {
        assert_ne!(
            call(app.clone(), method, path, &writer, json!({})).await.0,
            StatusCode::OK
        );
    }
    let mut invalid = site.clone();
    invalid["origin"] = json!("https://setup.example/path");
    assert_eq!(
        call(app.clone(), "POST", "/api/v1/sites", &writer, invalid)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, result) = call(app.clone(), "POST", "/api/v1/sites", &writer, site.clone()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let id: Uuid = result["id"].as_str().unwrap().parse().unwrap();
    let site_path = format!("/api/v1/sites/{id}");
    let pool_path = format!("{site_path}/number-pool");
    let listed = call(app.clone(), "GET", "/api/v1/sites", &writer, Value::Null)
        .await
        .1;
    assert_eq!(listed["sites"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sites"][0]["version"], 1);
    assert_eq!(listed["tracker_url"], "https://api.example/tracker.js");
    assert_eq!(
        call(app.clone(), "PATCH", &site_path, &writer, site.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut edited = site.clone();
    edited["version"] = json!(1);
    edited["name"] = json!("Edited pilot");
    assert_eq!(
        call(app.clone(), "PATCH", &site_path, &writer, edited.clone())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(app.clone(), "PATCH", &site_path, &writer, edited)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let pool = json!({"fallback_phone":"+12025550978","enabled":false,"version":0,"number_ids":[],"dedicated_numbers":false});
    assert!(
        call(app.clone(), "GET", &pool_path, &writer, Value::Null)
            .await
            .1["pool"]
            .is_null()
    );
    assert_eq!(
        call(app.clone(), "PUT", &pool_path, &writer, pool.clone())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(app.clone(), "PUT", &pool_path, &writer, pool.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut bad_pool = pool.clone();
    bad_pool["version"] = json!(1);
    bad_pool["enabled"] = json!(true);
    assert_eq!(
        call(app.clone(), "PUT", &pool_path, &writer, bad_pool)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let before = call(app.clone(), "GET", "/api/v1/setup", &writer, Value::Null)
        .await
        .1;
    assert!(before["sites"][0]["last_session_at"].is_null());
    assert!(before["sites"][0]["last_form_at"].is_null());
    assert_eq!(before["sites"][0]["pool_configured"], true);
    assert_eq!(before["sites"][0]["pool_enabled"], false);
    let session = Uuid::new_v4();
    store
        .capture_session(
            id,
            "https://setup.example",
            session,
            &json!({"source":"Newsletter","utm_campaign":"Setup","landing_url":"https://setup.example/quote"}),
        )
        .await
        .unwrap();
    store
        .capture_form(
            id,
            "https://setup.example",
            session,
            "setup-confirmed-form",
            &std::collections::BTreeMap::from([("email".into(), "private@example.test".into())]),
        )
        .await
        .unwrap();
    let after = call(app.clone(), "GET", "/api/v1/setup", &writer, Value::Null)
        .await
        .1;
    assert!(after["sites"][0]["last_session_at"].is_string());
    assert!(after["sites"][0]["last_form_at"].is_string());
    assert!(!after.to_string().contains("private@example.test"));
    store
        .add_number(&NewNumber {
            provider: PhoneProvider::Twilio,
            label: "Setup line".into(),
            phone_number: "+12025550976".into(),
            destination: "+12025550978".into(),
            source: "Offline".into(),
            campaign: "Setup".into(),
        })
        .await
        .unwrap();
    let number_id = store.numbers().await.unwrap()[0].id;
    let mut enabled_pool = json!({"fallback_phone":"+12025550978","enabled":true,"version":1,"number_ids":[number_id],"dedicated_numbers":false});
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            &pool_path,
            &writer,
            enabled_pool.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    enabled_pool["dedicated_numbers"] = json!(true);
    assert_eq!(
        call(app.clone(), "PUT", &pool_path, &writer, enabled_pool)
            .await
            .0,
        StatusCode::OK
    );
    store
        .allocate_dni(id, "https://setup.example", session)
        .await
        .unwrap();
    store
        .call_event(&convert_store::CallEvent {
            sid: Uuid::new_v4().to_string(),
            to: "+12025550976".into(),
            from: "+12025550977".into(),
            fingerprint: Uuid::new_v4().to_string(),
            kind: convert_store::CallEventKind::Voice,
        })
        .await
        .unwrap();
    let received = call(app.clone(), "GET", "/api/v1/setup", &writer, Value::Null)
        .await
        .1;
    assert_eq!(received["sites"][0]["pool_enabled"], true);
    assert_eq!(received["sites"][0]["assignment_count"], 1);
    assert!(received["sites"][0]["last_assignment_at"].is_string());
    assert_eq!(received["numbers"][0]["call_count"], 1);
    assert!(received["numbers"][0]["last_call_at"].is_string());
    assert!(!received.to_string().contains("+12025550977"));

    let other = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    other.migrate().await.unwrap();
    let key = other
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Other tenant".into(),
                scopes: vec!["read".into(), "setup:write".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap();
    let other_key = key["secret"].as_str().unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/sites", other_key, Value::Null)
            .await
            .1["sites"],
        json!([])
    );
    assert_eq!(
        call(app.clone(), "GET", "/api/v1/setup", other_key, Value::Null)
            .await
            .1["sites"],
        json!([])
    );
    assert_eq!(
        call(app.clone(), "GET", &pool_path, other_key, Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let mut foreign = pool;
    foreign["version"] = json!(1);
    assert_eq!(
        call(app.clone(), "PUT", &pool_path, other_key, foreign)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(app.clone(), "PATCH", &site_path, other_key, site)
            .await
            .0,
        StatusCode::CONFLICT
    );
}
