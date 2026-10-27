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
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn conversion_routes_require_workspace_authentication() {
    let database = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&database, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let key = "conversion-test-operator-key-at-least-32";
    let workspace = Uuid::new_v4();
    let owner = token(&store, workspace).await;
    let other = token(&store, Uuid::new_v4()).await;
    let app = Arc::new(App {
        store: store.clone(),
        api_key: key.into(),
        public_url: "https://engine.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "conversion-service-key-at-least-32".into(),
            cloud_origin: "https://cloud.example".into(),
            app_origin: None,
        }),
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    });
    let config = json!({"customer_id":"1234567890","qualified_action_id":"123","booking_action_id":"456","allow_inferred_calls":false,"version":0});
    for credential in ["", "wrong", "conversion-service-key-at-least-32"] {
        assert_eq!(
            call(
                app.clone(),
                "GET",
                "/api/conversions",
                credential,
                Value::Null
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                app.clone(),
                "PUT",
                "/api/conversions/settings",
                credential,
                config.clone()
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            "/api/conversions/settings",
            &owner,
            config.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    for credential in [key, other.as_str()] {
        let (status, result) = call(
            app.clone(),
            "GET",
            "/api/conversions",
            credential,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["settings"]["version"], 0);
    }
    assert_eq!(
        call(app.clone(), "GET", "/api/conversions", &owner, Value::Null)
            .await
            .1["settings"]["version"],
        1
    );
    sqlx::query("DELETE FROM hosted_memberships WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        call(app.clone(), "GET", "/api/conversions", &owner, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
