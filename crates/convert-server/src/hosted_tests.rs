use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(
    app: Shared,
    method: &str,
    path: &str,
    bearer: &str,
    body: Value,
    extra: &[(&str, &str)],
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json");
    for (key, value) in extra {
        request = request.header(*key, *value);
    }
    router(app)
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
#[ignore = "requires VISICONVERT_TEST_DATABASE_URL pointing to a disposable Postgres database"]
async fn cloud_handoff_enforces_membership_browser_binding_expiry_and_single_use() {
    let database = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let local = Uuid::new_v4();
    let store = Store::connect(&database, local).await.unwrap();
    store.migrate().await.unwrap();
    let service = "c".repeat(64);
    let operator = "a".repeat(64);
    let app = Arc::new(App {
        store,
        api_key: operator.clone(),
        public_url: "http://127.0.0.1:3100".into(),
        twilio_account: None,
        twilio_token: None,
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
        cloud: Some(hosted::Config {
            service_key: service.clone(),
            cloud_origin: "http://127.0.0.1:3200".into(),
            app_origin: None,
        }),
    });
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    let payload = |id, user| json!({"workspace_id":id,"user_id":user,"name":"Cloud QA"});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/workspaces",
            &operator,
            payload(first, owner),
            &[]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/workspaces",
            &service,
            payload(local, owner),
            &[]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    for id in [first, first, second] {
        assert_eq!(
            call(
                app.clone(),
                "POST",
                "/internal/cloud/workspaces",
                &service,
                payload(id, owner),
                &[]
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/workspaces",
            &service,
            payload(first, stranger),
            &[]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(app.clone(), "GET", "/api/leads", &service, Value::Null, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let start = call(
        app.clone(),
        "GET",
        &format!("/auth/start?workspace_id={first}"),
        "",
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(start.status(), StatusCode::SEE_OTHER);
    let cookie = start.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(
        start.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("HttpOnly")
    );
    let url = url::Url::parse(start.headers()["location"].to_str().unwrap()).unwrap();
    let challenge = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    let launch = |id, user| json!({"workspace_id":id,"user_id":user,"state":challenge});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/launch",
            &service,
            launch(first, stranger),
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let code = value(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/launch",
            &service,
            launch(first, owner),
            &[],
        )
        .await,
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = json!({"code":code});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            body.clone(),
            &[("origin", "https://evil.example"), ("cookie", &cookie)]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            body.clone(),
            &[("origin", &app.public_url)]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let headers = [
        ("origin", app.public_url.as_str()),
        ("cookie", cookie.as_str()),
    ];
    let (one, two) = tokio::join!(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            body.clone(),
            &headers
        ),
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            body.clone(),
            &headers
        )
    );
    let good = if one.status() == StatusCode::OK {
        assert_eq!(two.status(), StatusCode::UNAUTHORIZED);
        one
    } else {
        assert_eq!(one.status(), StatusCode::UNAUTHORIZED);
        two
    };
    assert_eq!(good.status(), StatusCode::OK);
    let token = value(good).await["token"].as_str().unwrap().to_owned();
    let bootstrap = value(
        call(
            app.clone(),
            "GET",
            "/api/bootstrap",
            &token,
            Value::Null,
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(bootstrap["id"], first.to_string());
    assert_eq!(bootstrap["hosted"], true);
    let lead = json!({"external_id":"cloud-qa","name":"QA lead","contact":"test@example.com","source":"Website","campaign":"QA","notes":""});
    assert_eq!(
        call(app.clone(), "POST", "/api/leads", &token, lead, &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(app.store.leads().await.unwrap().len(), 0);
    let other = Store {
        pool: app.store.pool.clone(),
        workspace: second,
    };
    assert_eq!(other.leads().await.unwrap().len(), 0);
    // A membership is rechecked for every API request, not just at token issuance.
    sqlx::query("UPDATE hosted_memberships SET enabled=false WHERE workspace_id=$1")
        .bind(first)
        .execute(&app.store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/leads", &token, Value::Null, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE hosted_memberships SET enabled=true WHERE workspace_id=$1")
        .bind(first)
        .execute(&app.store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            app.clone(),
            "DELETE",
            "/api/session",
            &token,
            Value::Null,
            &[]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(app.clone(), "GET", "/api/leads", &token, Value::Null, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let code = value(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/launch",
            &service,
            launch(first, owner),
            &[],
        )
        .await,
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query(
        "UPDATE hosted_launch_codes SET expires_at=now()-interval '1 second' WHERE workspace_id=$1",
    )
    .bind(first)
    .execute(&app.store.pool)
    .await
    .unwrap();
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            json!({"code":code}),
            &headers
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let code = value(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/launch",
            &service,
            launch(first, owner),
            &[],
        )
        .await,
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = value(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            json!({"code":code}),
            &headers,
        )
        .await,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query(
        "UPDATE hosted_sessions SET expires_at=now()-interval '1 second' WHERE workspace_id=$1",
    )
    .bind(first)
    .execute(&app.store.pool)
    .await
    .unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/leads", &token, Value::Null, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            app.clone(),
            "GET",
            "/api/bootstrap",
            &operator,
            Value::Null,
            &[]
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[test]
fn hosted_origins_reject_insecure_or_untrusted_redirect_shapes() {
    for url in [
        "http://example.com",
        "https://user@example.com",
        "https://example.com/path",
        "https://example.com?x=1",
        "https://example.com/#x",
    ] {
        assert!(hosted::origin(url).is_err(), "{url}");
    }
    for url in ["https://cloud.example.com", "http://127.0.0.1:3200"] {
        assert!(hosted::origin(url).is_ok());
    }
}

#[tokio::test]
#[ignore = "requires VISICONVERT_TEST_DATABASE_URL pointing to a disposable Postgres database"]
async fn credential_revocation_is_monotonic_isolated_and_serializes_exchange_and_issuance() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    let other_workspace = Uuid::new_v4();
    store
        .provision_hosted(workspace, owner, "Revocation QA")
        .await
        .unwrap();
    store
        .provision_hosted(other_workspace, other, "Other QA")
        .await
        .unwrap();
    let key = |suffix: &str| format!("{}-{suffix}", Uuid::new_v4());
    let used = key("used");
    let unused = key("unused");
    let session = key("session");
    let other_code = key("other-code");
    let other_session = key("other-session");
    assert!(
        store
            .issue_hosted_code(workspace, owner, "challenge", &used, 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .exchange_hosted_code(&used, "challenge", &session)
            .await
            .unwrap()
    );
    assert!(
        store
            .issue_hosted_code(workspace, owner, "challenge", &unused, 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .issue_hosted_code(other_workspace, other, "challenge", &other_code, 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .exchange_hosted_code(&other_code, "challenge", &other_session)
            .await
            .unwrap()
    );
    let service = "c".repeat(64);
    let app = Arc::new(App {
        store: store.clone(),
        api_key: "a".repeat(64),
        public_url: "http://127.0.0.1:3100".into(),
        twilio_account: None,
        twilio_token: None,
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
        cloud: Some(hosted::Config {
            service_key: service.clone(),
            cloud_origin: "http://127.0.0.1:3200".into(),
            app_origin: None,
        }),
    });
    let payload = json!({"user_id":owner,"credential_version":1});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/internal/cloud/revoke-user",
            &"a".repeat(64),
            payload.clone(),
            &[]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(
        app.clone(),
        "POST",
        "/internal/cloud/revoke-user",
        &service,
        payload,
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("strict-transport-security"));
    assert!(store.hosted_session(&session).await.unwrap().is_none());
    assert!(
        !store
            .exchange_hosted_code(&unused, "challenge", &key("stolen"))
            .await
            .unwrap()
    );
    assert!(
        store
            .hosted_session(&other_session)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !store
            .issue_hosted_code(workspace, owner, "challenge", &key("late"), 0)
            .await
            .unwrap()
    );

    let fresh_code = key("fresh-code");
    let fresh_session = key("fresh-session");
    assert!(
        store
            .issue_hosted_code(workspace, owner, "challenge", &fresh_code, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .exchange_hosted_code(&fresh_code, "challenge", &fresh_session)
            .await
            .unwrap()
    );
    assert_eq!(store.revoke_hosted_user(owner, 1).await.unwrap(), 1);
    assert!(
        store
            .hosted_session(&fresh_session)
            .await
            .unwrap()
            .is_some(),
        "retry must not revoke newly authorized access"
    );

    for version in 2..10 {
        let code = key("racing-code");
        let token = key("racing-token");
        let late_code = key("late-code");
        assert!(
            store
                .issue_hosted_code(workspace, owner, "challenge", &code, version - 1)
                .await
                .unwrap()
        );
        let (exchange, revoke, issue) = tokio::join!(
            store.exchange_hosted_code(&code, "challenge", &token),
            store.revoke_hosted_user(owner, version),
            store.issue_hosted_code(workspace, owner, "challenge", &late_code, version - 1)
        );
        exchange.unwrap();
        revoke.unwrap();
        issue.unwrap();
        assert!(store.hosted_session(&token).await.unwrap().is_none());
        assert!(
            !store
                .exchange_hosted_code(&late_code, "challenge", &key("late-token"))
                .await
                .unwrap()
        );
        assert_eq!(
            store.revoke_hosted_user(owner, version - 1).await.unwrap(),
            version
        );
    }
    // Revocation before first provisioning must not be undone by provisioning.
    let future_user = Uuid::new_v4();
    let future_workspace = Uuid::new_v4();
    store.revoke_hosted_user(future_user, 3).await.unwrap();
    store
        .provision_hosted(future_workspace, future_user, "Later provisioning")
        .await
        .unwrap();
    assert!(
        !store
            .issue_hosted_code(future_workspace, future_user, "challenge", &key("old"), 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .issue_hosted_code(future_workspace, future_user, "challenge", &key("new"), 3)
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires VISICONVERT_TEST_DATABASE_URL pointing to a disposable Postgres database"]
async fn unified_handoff_requires_app_origin_cookie_owner_and_single_use() {
    let database = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&database, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let workspace = Uuid::new_v4();
    let owner = Uuid::new_v4();
    store
        .provision_hosted(workspace, owner, "Unified workspace")
        .await
        .unwrap();
    let service = "c".repeat(64);
    let app = Arc::new(App {
        store,
        api_key: "a".repeat(64),
        public_url: "https://legacy.example".into(),
        twilio_account: None,
        twilio_token: None,
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
        cloud: Some(hosted::Config {
            service_key: service.clone(),
            cloud_origin: "https://cloud.example".into(),
            app_origin: Some("https://cloud.example".into()),
        }),
    });
    assert_eq!(
        value(call(app.clone(), "GET", "/runtime.json", "", Value::Null, &[]).await).await["hosted_app_origin"],
        "https://cloud.example"
    );
    let payload = json!({"workspace_id":workspace});
    for origin in ["https://evil.example", "https://legacy.example", "null", ""] {
        assert_eq!(
            call(
                app.clone(),
                "POST",
                "/auth/prepare",
                "",
                payload.clone(),
                &[("origin", origin)]
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let prepared = call(
        app.clone(),
        "POST",
        "/auth/prepare",
        "",
        payload,
        &[("origin", "https://cloud.example")],
    )
    .await;
    assert_eq!(prepared.status(), StatusCode::OK);
    let cookie_header = prepared.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie_header.contains("HttpOnly")
            && cookie_header.contains("Secure")
            && cookie_header.contains("Path=/auth")
    );
    let cookie = cookie_header.split(';').next().unwrap().to_owned();
    let state = value(prepared).await["state"].as_str().unwrap().to_owned();
    assert!(!state.contains(cookie.split('=').nth(1).unwrap()));
    let launch_payload = json!({"workspace_id":workspace,"user_id":owner,"state":state});
    let launch = call(
        app.clone(),
        "POST",
        "/internal/cloud/launch",
        &service,
        launch_payload,
        &[],
    )
    .await;
    assert_eq!(launch.status(), StatusCode::OK);
    let code = value(launch).await["code"].as_str().unwrap().to_owned();
    for (origin, binding) in [
        ("https://evil.example", cookie.as_str()),
        ("https://cloud.example", ""),
    ] {
        assert_eq!(
            call(
                app.clone(),
                "POST",
                "/auth/exchange",
                "",
                json!({"code":code}),
                &[("origin", origin), ("cookie", binding)]
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let headers = [
        ("origin", "https://cloud.example"),
        ("cookie", cookie.as_str()),
    ];
    let exchange = call(
        app.clone(),
        "POST",
        "/auth/exchange",
        "",
        json!({"code":code}),
        &headers,
    )
    .await;
    assert_eq!(exchange.status(), StatusCode::OK);
    let token = value(exchange).await["token"].as_str().unwrap().to_owned();
    assert_eq!(
        value(
            call(
                app.clone(),
                "GET",
                "/api/bootstrap",
                &token,
                Value::Null,
                &[]
            )
            .await
        )
        .await["id"],
        workspace.to_string()
    );
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/auth/exchange",
            "",
            json!({"code":code}),
            &headers
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            app.clone(),
            "DELETE",
            "/api/session",
            &token,
            Value::Null,
            &[]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(app, "GET", "/api/bootstrap", &token, Value::Null, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
