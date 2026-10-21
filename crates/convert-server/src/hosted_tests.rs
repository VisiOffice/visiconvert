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
        cloud: Some(hosted::Config {
            service_key: service.clone(),
            cloud_origin: "http://127.0.0.1:3200".into(),
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
