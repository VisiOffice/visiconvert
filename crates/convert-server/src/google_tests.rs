use super::*;
use axum::{body::Body, extract::OriginalUri};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[derive(Default)]
pub(super) struct Provider {
    pub(super) deliveries: Mutex<Vec<Value>>,
    pub(super) delivery_http: AtomicUsize,
    pub(super) processing: AtomicBool,
    challenge: Mutex<String>,
    exchanges: AtomicUsize,
    deny: AtomicBool,
    calls: Mutex<Vec<String>>,
}
async fn provider(
    State(p): State<Arc<Provider>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    p.calls.lock().unwrap().push(uri.path().into());
    if uri.path() == "/token" {
        let form = url::form_urlencoded::parse(&body).collect::<std::collections::HashMap<_, _>>();
        assert_eq!(form["client_secret"], "test-secret-not-for-production");
        if form["grant_type"] == "authorization_code" {
            p.exchanges.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
                *p.challenge.lock().unwrap()
            );
            assert_eq!(
                form["redirect_uri"],
                "https://engine.example/auth/google/callback"
            );
            if form["code"] == "missing-scope" {
                return Json(json!({"access_token":"private-google-access","refresh_token":"private-google-refresh","scope":"https://www.googleapis.com/auth/adwords","token_type":"Bearer"})).into_response();
            }
            if form["code"] == "no-refresh" {
                return Json(json!({"access_token":"private-google-access","scope":convert_adapters::google_api::SCOPES,"token_type":"Bearer"})).into_response();
            }
        } else {
            assert_eq!(form["refresh_token"], "private-google-refresh");
        }
        if p.deny.load(Ordering::SeqCst) {
            return (StatusCode::BAD_REQUEST,Json(json!({"error":"invalid_grant","error_description":"PRIVATE_PROVIDER_ERROR private-google-refresh"}))).into_response();
        }
        return Json(json!({"access_token":"private-google-access","refresh_token":"private-google-refresh","scope":convert_adapters::google_api::SCOPES,"token_type":"Bearer"})).into_response();
    }
    assert_eq!(headers["authorization"], "Bearer private-google-access");
    assert!(headers.get("developer-token").is_none());
    if uri.path() == "/v1/events:ingest" {
        let v: Value = serde_json::from_slice(&body).unwrap();
        if v["validateOnly"] == true {
            return Json(json!({})).into_response();
        }
        p.deliveries.lock().unwrap().push(v);
        let status = p.delivery_http.load(Ordering::SeqCst);
        if status != 0 {
            return (
                StatusCode::from_u16(status as u16).unwrap(),
                Json(json!({"error":"PRIVATE_PROVIDER_ERROR"})),
            )
                .into_response();
        }
        return Json(json!({"requestId":"google-receipt-1"})).into_response();
    }
    if uri.path() == "/v1/requestStatus:retrieve" {
        let v = p.deliveries.lock().unwrap().last().unwrap().clone();
        return Json(json!({"requestStatusPerDestination":[{"destination":v["destinations"][0],"requestStatus":if p.processing.load(Ordering::SeqCst) {"PROCESSING"} else {"SUCCESS"},"eventsIngestionStatus":{"recordCount":"1"}}]})).into_response();
    }
    if uri.path() == "/v26/customers:listAccessibleCustomers" {
        return Json(json!({"resourceNames":["customers/1234567890"]})).into_response();
    }
    assert!(
        uri.path().ends_with("/googleAds:search"),
        "Unexpected Google endpoint"
    );
    assert_eq!(headers["login-customer-id"], "1234567890");
    let q: Value = serde_json::from_slice(&body).unwrap();
    let query = q["query"].as_str().unwrap();
    if query.contains("FROM customer_client") {
        return Json(json!({"results":[{"customerClient":{"id":"1234567890","descriptiveName":"Google test account","manager":false,"status":"ENABLED"}}]})).into_response();
    }
    if query.contains("FROM customer LIMIT") {
        return Json(json!({"results":[{"customer":{"id":"1234567890","descriptiveName":"Google test account","manager":false,"status":"ENABLED","currencyCode":"USD","timeZone":"America/Chicago","conversionTrackingSetting":{"googleAdsConversionCustomer":"customers/1234567890"}}}]})).into_response();
    }
    assert!(query.contains("FROM conversion_action"));
    Json(json!({"results":[
        {"conversionAction":{"id":"123","name":"Qualified","status":"ENABLED","type":"UPLOAD_CLICKS","ownerCustomer":"customers/1234567890","primaryForGoal":false,"clickThroughLookbackWindowDays":30}},
        {"conversionAction":{"id":"456","name":"Booked","status":"ENABLED","type":"UPLOAD_CLICKS","ownerCustomer":"customers/1234567890","primaryForGoal":true,"clickThroughLookbackWindowDays":30}},
        {"conversionAction":{"id":"789","name":"Wrong owner","status":"ENABLED","type":"UPLOAD_CLICKS","ownerCustomer":"customers/9999999999"}},
        {"conversionAction":{"id":"987","name":"Wrong type","status":"ENABLED","type":"WEBPAGE","ownerCustomer":"customers/1234567890"}},
        {"conversionAction":{"id":"654","name":"Removed","status":"REMOVED","type":"UPLOAD_CLICKS","ownerCustomer":"customers/1234567890"}}
    ]})).into_response()
}
async fn request(
    app: Shared,
    method: &str,
    path: &str,
    bearer: &str,
    cookie: &str,
    value: Value,
) -> (StatusCode, HeaderMap, String) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!text.contains("private-google"));
    assert!(!text.contains("PRIVATE_PROVIDER_ERROR"));
    (code, headers, text)
}
pub(super) async fn api(
    app: Shared,
    method: &str,
    path: &str,
    key: &str,
    v: Value,
) -> (StatusCode, Value) {
    let (status, _, body) = request(app, method, path, key, "", v).await;
    (status, serde_json::from_str(&body).unwrap())
}
pub(super) async fn fixture() -> (Shared, Arc<Provider>, tokio::task::JoinHandle<()>) {
    let p = Arc::new(Provider::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let provider_app = Router::new().fallback(provider).with_state(p.clone());
    let task = tokio::spawn(async move { axum::serve(listener, provider_app).await.unwrap() });
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let a = Arc::new(App {
        store,
        api_key: "google-test-operator-key-at-least-32".into(),
        public_url: "https://engine.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "google-service-key-at-least-32".into(),
            cloud_origin: "https://cloud.example".into(),
            app_origin: None,
        }),
        vault: Some(secrets::Vault::from_hex(&"a".repeat(64)).unwrap()),
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: Some(google::Config {
            client_id: "test.apps.googleusercontent.com".into(),
            client_secret: "test-secret-not-for-production".into(),
            redirect: "https://engine.example/auth/google/callback".into(),
        }),
        google_client: convert_adapters::google_api::Client::for_test(origin),
    });
    (a, p, task)
}
pub(super) async fn begin(a: Shared, p: &Provider, key: &str, revision: i64) -> (String, String) {
    let (s, h, b) = request(
        a,
        "POST",
        "/api/connections/google/start",
        key,
        "",
        json!({"revision":revision}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_str(&b).unwrap();
    let u = url::Url::parse(v["authorization_url"].as_str().unwrap()).unwrap();
    assert_eq!(
        u.origin().ascii_serialization(),
        "https://accounts.google.com"
    );
    let q = u.query_pairs().collect::<std::collections::HashMap<_, _>>();
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["access_type"], "offline");
    *p.challenge.lock().unwrap() = q["code_challenge"].to_string();
    let cookie = h["set-cookie"].to_str().unwrap();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("SameSite=Lax")
    );
    (
        q["state"].to_string(),
        cookie.split(';').next().unwrap().into(),
    )
}
pub(super) async fn callback(a: Shared, state: &str, cookie: &str, code: &str) -> String {
    let (s, h, b) = request(
        a,
        "GET",
        &format!("/auth/google/callback?state={state}&code={code}"),
        "",
        cookie,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(b.is_empty());
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(h["referrer-policy"], "no-referrer");
    h["location"].to_str().unwrap().into()
}
pub(super) async fn hosted_token(store: &Store) -> (String, Uuid) {
    let user = Uuid::new_v4();
    store
        .provision_hosted(store.workspace, user, "Google test")
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
            .issue_hosted_code(store.workspace, user, "browser", &code, 0)
            .await
            .unwrap()
    );
    assert!(
        store
            .exchange_hosted_code(&code, "browser", &hosted::digest(&token))
            .await
            .unwrap()
    );
    (token, user)
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn google_oauth_binding_encryption_replay_and_disconnect() {
    let (a, p, task) = fixture().await;
    let key = &a.api_key;
    for bad in ["", "wrong", "google-service-key-at-least-32", "vc_key_bad"] {
        assert_eq!(
            api(
                a.clone(),
                "GET",
                "/api/connections/google",
                bad,
                Value::Null
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        )
    }
    let (state, cookie) = begin(a.clone(), &p, key, 0).await;
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM google_oauth_attempts t WHERE workspace_id=$1")
            .bind(a.store.workspace)
            .fetch_one(&a.store.pool)
            .await
            .unwrap();
    assert!(!row.to_string().contains(&state));
    assert!(!row.to_string().contains(cookie.split('=').nth(1).unwrap()));
    assert!(
        callback(a.clone(), &state, "vc_google_browser=bad", "ok")
            .await
            .ends_with("failed")
    );
    assert_eq!(p.exchanges.load(Ordering::SeqCst), 0);
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("connected")
    );
    assert_eq!(p.exchanges.load(Ordering::SeqCst), 1);
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("failed")
    );
    assert_eq!(p.exchanges.load(Ordering::SeqCst), 1);
    let c = a.store.google_connection().await.unwrap();
    assert_eq!(c.revision, 1);
    let encrypted = c.encrypted_token.unwrap();
    assert!(!encrypted.contains("private-google"));
    let vault = a.vault.as_ref().unwrap();
    assert_eq!(
        vault
            .open_google(a.store.workspace, "refresh", &encrypted)
            .unwrap(),
        "private-google-refresh"
    );
    assert!(
        vault
            .open_google(Uuid::new_v4(), "refresh", &encrypted)
            .is_err()
    );
    assert!(
        vault
            .open(a.store.workspace, "refresh", &encrypted)
            .is_err()
    );
    // A new process can use the same encrypted connection without session memory.
    let mut restarted = (*a).clone();
    restarted.store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        a.store.workspace,
    )
    .await
    .unwrap();
    assert_eq!(
        api(
            Arc::new(restarted),
            "GET",
            "/api/connections/google/accounts",
            key,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let (state, cookie) = begin(a.clone(), &p, key, 1).await;
    assert_eq!(
        api(
            a.clone(),
            "DELETE",
            "/api/connections/google",
            key,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("failed")
    );
    assert!(
        !api(
            a.clone(),
            "GET",
            "/api/connections/google",
            key,
            Value::Null
        )
        .await
        .1["connected"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        api(
            a.clone(),
            "POST",
            "/api/connections/google/start",
            key,
            json!({"revision":0})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // A callback already exchanging its code cannot overwrite a newer attempt.
    let (old_state, old_cookie) = begin(a.clone(), &p, key, 2).await;
    let old_attempt = a
        .store
        .consume_google_oauth(
            &hosted::digest(&old_state),
            &hosted::digest(old_cookie.split_once('=').unwrap().1),
        )
        .await
        .unwrap()
        .unwrap();
    let (new_state, new_cookie) = begin(a.clone(), &p, key, 2).await;
    assert!(matches!(
        a.store.finish_google_oauth(&old_attempt, &encrypted).await,
        Err(convert_store::Error::Conflict)
    ));
    assert!(
        a.store
            .google_connection()
            .await
            .unwrap()
            .encrypted_token
            .is_none()
    );
    assert!(
        callback(a.clone(), &new_state, &new_cookie, "ok")
            .await
            .ends_with("connected")
    );
    assert!(
        p.calls
            .lock()
            .unwrap()
            .iter()
            .all(|v| v == "/token" || v.starts_with("/v26/customers"))
    );
    task.abort();
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn google_scopes_expiry_cancellation_and_owner_revocation() {
    let (a, p, task) = fixture().await;
    for code in ["missing-scope", "no-refresh"] {
        let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
        assert!(
            callback(a.clone(), &state, &cookie, code)
                .await
                .ends_with("failed")
        );
        assert!(
            a.store
                .google_connection()
                .await
                .unwrap()
                .encrypted_token
                .is_none()
        );
    }
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    sqlx::query("UPDATE google_oauth_attempts SET expires_at=now()-interval '1 second' WHERE workspace_id=$1").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("failed")
    );
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    let (_,h,_)=request(a.clone(),"GET",&format!("/auth/google/callback?state={state}&error=access_denied&error_description=PRIVATE_PROVIDER_ERROR"),"",&cookie,Value::Null).await;
    assert!(h["location"].to_str().unwrap().ends_with("failed"));
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    // The schema already exists; migrate() would reserve this ID as self-hosted.
    let (token, user) = hosted_token(&store).await;
    let (state, cookie) = begin(a.clone(), &p, &token, 0).await;
    sqlx::query("UPDATE hosted_memberships SET enabled=false WHERE workspace_id=$1 AND user_id=$2")
        .bind(store.workspace)
        .bind(user)
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        api(
            a.clone(),
            "GET",
            "/api/connections/google",
            &token,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("failed")
    );
    assert!(
        store
            .google_connection()
            .await
            .unwrap()
            .encrypted_token
            .is_none()
    );
    sqlx::query("UPDATE hosted_memberships SET enabled=true WHERE workspace_id=$1 AND user_id=$2")
        .bind(store.workspace)
        .bind(user)
        .execute(&store.pool)
        .await
        .unwrap();
    let (state, cookie) = begin(a.clone(), &p, &token, 0).await;
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("connected")
    );
    assert!(
        store
            .google_connection()
            .await
            .unwrap()
            .encrypted_token
            .is_some()
    );
    assert!(
        a.store
            .google_connection()
            .await
            .unwrap()
            .encrypted_token
            .is_none()
    );
    assert!(
        !api(
            a.clone(),
            "GET",
            "/api/connections/google",
            &a.api_key,
            Value::Null
        )
        .await
        .1["connected"]
            .as_bool()
            .unwrap()
    );
    // Revocation while the provider exchange is in flight is checked again
    // inside the transaction that would persist its refresh token.
    let (state, cookie) = begin(a.clone(), &p, &token, 1).await;
    let attempt = store
        .consume_google_oauth(
            &hosted::digest(&state),
            &hosted::digest(cookie.split_once('=').unwrap().1),
        )
        .await
        .unwrap()
        .unwrap();
    let before = store.google_connection().await.unwrap();
    store.revoke_hosted_user(user, 1).await.unwrap();
    assert!(matches!(
        store
            .finish_google_oauth(&attempt, before.encrypted_token.as_deref().unwrap())
            .await,
        Err(convert_store::Error::Conflict)
    ));
    assert_eq!(
        store.google_connection().await.unwrap().revision,
        before.revision
    );
    task.abort();
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn google_discovery_validation_staleness_and_provider_errors() {
    let (a, p, task) = fixture().await;
    let key = &a.api_key;
    let (state, cookie) = begin(a.clone(), &p, key, 0).await;
    assert!(
        callback(a.clone(), &state, &cookie, "ok")
            .await
            .ends_with("connected")
    );
    let (_, accounts) = api(
        a.clone(),
        "GET",
        "/api/connections/google/accounts?login=1234567890",
        key,
        Value::Null,
    )
    .await;
    assert_eq!(accounts["accounts"][0]["name"], "Google test account");
    assert_eq!(
        api(
            a.clone(),
            "GET",
            "/api/connections/google/actions?login=9999999999&customer=1234567890",
            key,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, actions) = api(
        a.clone(),
        "GET",
        "/api/connections/google/actions?login=1234567890&customer=1234567890",
        key,
        Value::Null,
    )
    .await;
    assert_eq!(
        actions["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["eligible"] == true)
            .count(),
        2
    );
    let mut settings = json!({"customer_id":"1234567890","qualified_action_id":"123","booking_action_id":"456","allow_inferred_calls":false,"version":0});
    assert_eq!(
        api(
            a.clone(),
            "PUT",
            "/api/conversions/settings",
            key,
            settings.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let input = json!({"revision":1,"settings_version":1,"login":"1234567890"});
    let (code, report) = api(
        a.clone(),
        "POST",
        "/api/connections/google/validate",
        key,
        input.clone(),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(report["valid"], true);
    assert_eq!(report["uploads_enabled"], false);
    settings["version"] = json!(1);
    // Valid field type, invalid owning account.
    settings["booking_action_id"] = json!("789");
    assert_eq!(
        api(a.clone(), "PUT", "/api/conversions/settings", key, settings)
            .await
            .0,
        StatusCode::OK
    );
    assert!(
        a.store
            .google_connection()
            .await
            .unwrap()
            .validation
            .is_none()
    );
    assert_eq!(
        api(
            a.clone(),
            "POST",
            "/api/connections/google/validate",
            key,
            input
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let input = json!({"revision":1,"settings_version":2,"login":"1234567890"});
    assert_eq!(
        api(
            a.clone(),
            "POST",
            "/api/connections/google/validate",
            key,
            input.clone()
        )
        .await
        .1["valid"],
        false
    );
    assert!(
        a.store
            .save_google_validation(1, 1, &json!({"valid":true}), "operator")
            .await
            .is_err()
    );
    p.deny.store(true, Ordering::SeqCst);
    let (code, body) = api(
        a.clone(),
        "POST",
        "/api/connections/google/validate",
        key,
        input,
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("Reconnect"));
    assert!(
        a.store
            .google_connection()
            .await
            .unwrap()
            .validation
            .is_none()
    );
    assert!(
        p.calls
            .lock()
            .unwrap()
            .iter()
            .all(|s| !s.contains("ingest") && !s.contains("mutate"))
    );
    task.abort();
}
