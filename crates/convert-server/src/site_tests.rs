use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(
    app: Shared,
    method: &str,
    path: &str,
    key: &str,
    origin: &str,
    payload: Value,
) -> (StatusCode, HeaderMap, Value) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("origin", origin)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn hosted_token(store: &Store, workspace: Uuid) -> String {
    let user = Uuid::new_v4();
    store
        .provision_hosted(workspace, user, "Website tests")
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
#[tokio::test]
#[ignore = "Requires a disposable VISICONVERT_TEST_DATABASE_URL"]
async fn website_capture_isolated_atomic_and_consent_gated() {
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let key = "website-test-operator-key-32-characters";
    let workspace = Uuid::new_v4();
    let hosted = hosted_token(&store, workspace).await;
    let other = hosted_token(&store, Uuid::new_v4()).await;
    let app = Arc::new(App {
        store: store.clone(),
        api_key: key.into(),
        public_url: "https://engine.example".into(),
        twilio_account: None,
        twilio_token: None,
        vault: None,
        cloud: Some(hosted::Config {
            service_key: "service-key-long-enough-for-tests".into(),
            cloud_origin: "https://cloud.example".into(),
        }),
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
    });
    let website = "https://business.example";
    let input = json!({"name":"Business","origin":website,"allowed_fields":["name","email"],"enabled":true,"version":0});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/sites",
            "",
            website,
            input.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let created = call(
        app.clone(),
        "POST",
        "/api/sites",
        &hosted,
        website,
        input.clone(),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    let site = created.2["id"].as_str().unwrap();
    let prefix = format!("/collect/sites/{site}");
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/sites",
            &other,
            website,
            Value::Null
        )
        .await
        .2["sites"],
        json!([])
    );
    assert_eq!(
        call(app.clone(), "GET", "/api/sites", key, website, Value::Null)
            .await
            .2["sites"],
        json!([])
    );
    let mut edit = input.clone();
    edit["version"] = json!(1);
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &format!("/api/sites/{site}"),
            &other,
            website,
            edit.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for origin in [
        "null",
        "https://business.example.evil.test",
        "https://other.example",
        "",
    ] {
        let denied = call(
            app.clone(),
            "POST",
            &format!("{prefix}/sessions"),
            "",
            origin,
            json!({}),
        )
        .await;
        assert!(!denied.0.is_success());
        assert!(!denied.1.contains_key("access-control-allow-origin"));
    }
    let preflight = call(
        app.clone(),
        "OPTIONS",
        &format!("{prefix}/forms"),
        "",
        website,
        Value::Null,
    )
    .await;
    assert_eq!(preflight.0, StatusCode::NO_CONTENT);
    assert_eq!(preflight.1["access-control-allow-origin"], website);
    assert!(!preflight.1.contains_key("access-control-allow-credentials"));
    let session_id = Uuid::new_v4();
    let mut visit = json!({"id":session_id,"consent":"denied","landing_url":format!("{website}/quote?utm_source=google&utm_medium=cpc&utm_campaign=spring&gclid=test-click&password=excluded#secret"),"referrer":"https://search.example/private?email=excluded"});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/sessions"),
            "",
            website,
            visit.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    visit["consent"] = json!("granted");
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/sessions"),
            "",
            website,
            visit.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/sessions"),
            "",
            website,
            visit.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut form = json!({"session_id":session_id,"submission_id":"backend-001","consent":"granted","fields":{"name":"Site customer","email":"test@example.com"}});
    form["fields"]["password"] = json!("must-not-store");
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/forms"),
            "",
            website,
            form.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    form["fields"].as_object_mut().unwrap().remove("password");
    let path = format!("{prefix}/forms");
    let (a, b) = tokio::join!(
        call(app.clone(), "POST", &path, "", website, form.clone()),
        call(app.clone(), "POST", &path, "", website, form.clone())
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.2, json!({"accepted":true}));
    let leads = call(
        app.clone(),
        "GET",
        "/api/leads",
        &hosted,
        website,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(leads.as_array().unwrap().len(), 1);
    assert_eq!(leads[0]["source"], "google");
    assert_eq!(leads[0]["campaign"], "spring");
    let lead = leads[0]["id"].as_str().unwrap();
    let path = format!("/api/leads/{lead}/website-evidence");
    let evidence = call(app.clone(), "GET", &path, &hosted, website, Value::Null)
        .await
        .2;
    assert_eq!(
        evidence["evidence"]["landing_url"],
        format!("{website}/quote")
    );
    assert!(!evidence.to_string().contains("excluded"));
    assert!(!evidence.to_string().contains("must-not-store"));
    assert_eq!(
        call(app.clone(), "GET", &path, &other, website, Value::Null)
            .await
            .2,
        Value::Null
    );
    let mut changed = form.clone();
    changed["fields"]["name"] = json!("Different customer");
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/forms"),
            "",
            website,
            changed
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // A session from another site cannot provide attribution here.
    let other_site = call(app.clone(), "POST", "/api/sites", &other, website, input)
        .await
        .2["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("/collect/sites/{other_site}/forms"),
            "",
            website,
            form.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE website_sessions SET created_at=now()-interval '31 minutes' WHERE id=$1")
        .bind(session_id)
        .execute(&store.pool)
        .await
        .unwrap();
    // An acknowledged form can safely retry after expiry, but a new form cannot.
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/forms"),
            "",
            website,
            form.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    form["submission_id"] = json!("new-after-expiry");
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/forms"),
            "",
            website,
            form
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE sites SET rate_count=120,rate_started_at=now() WHERE id=$1")
        .bind(site.parse::<Uuid>().unwrap())
        .execute(&store.pool)
        .await
        .unwrap();
    visit["id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("{prefix}/sessions"),
            "",
            website,
            visit
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let sites = call(
        app.clone(),
        "GET",
        "/api/sites",
        &hosted,
        website,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(sites["sites"][0]["session_count"], 1);
    assert_eq!(sites["sites"][0]["form_count"], 1);
    edit["enabled"] = json!(false);
    assert_eq!(
        call(
            app.clone(),
            "PATCH",
            &format!("/api/sites/{site}"),
            &hosted,
            website,
            edit
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            app.clone(),
            "GET",
            &format!("{prefix}/config"),
            "",
            website,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Self-hosted creation and collection use the same routes without the Rails bridge.
    let local = call(app.clone(),"POST","/api/sites",key,website,json!({"name":"Self hosted","origin":website,"allowed_fields":[],"enabled":true,"version":0})).await;
    assert_eq!(local.0, StatusCode::OK);
}
