use super::*;
use axum::{body::Body, extract::OriginalUri};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use sha1::Sha1;
use tower::ServiceExt;

async fn api(
    app: Shared,
    method: &str,
    path: &str,
    bearer: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({"non_json":true}));
    (status, value)
}

async fn mock_twilio(OriginalUri(uri): OriginalUri, headers: HeaderMap) -> Response {
    let account = uri
        .path()
        .split('/')
        .nth(3)
        .unwrap_or("")
        .trim_end_matches(".json");
    let digit = account.as_bytes().get(2).copied().unwrap_or(b'0');
    let token = match digit {
        b'1' => "a",
        b'2' => "b",
        _ => "c",
    }
    .repeat(32);
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth
        == format!(
            "Basic {}",
            STANDARD.encode(format!("{account}:{}", "f".repeat(32)))
        )
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"message":"secret provider detail"})),
        )
            .into_response();
    }
    if auth != format!("Basic {}", STANDARD.encode(format!("{account}:{token}"))) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"private provider response"})),
        )
            .into_response();
    }
    if uri.path().ends_with("IncomingPhoneNumbers.json") {
        let phone = match digit {
            b'1' => "+12025550211",
            b'2' => "+12025550212",
            _ => "+12025550213",
        };
        let query = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .collect::<std::collections::HashMap<_, _>>();
        let numbers = if query.get("PhoneNumber").is_none_or(|p| p == phone) {
            vec![
                json!({"sid":format!("PN{}", "1".repeat(32)),"account_sid":account,"phone_number":phone,"friendly_name":"Test line","capabilities":{"voice":true}}),
            ]
        } else {
            vec![]
        };
        Json(json!({"incoming_phone_numbers":numbers,"next_page_uri":null})).into_response()
    } else {
        Json(json!({"sid":account,"status":"active","auth_token":token})).into_response()
    }
}

async fn bearer(store: &Store, workspace: Uuid) -> String {
    let user = Uuid::new_v4();
    store
        .provision_hosted(workspace, user, "Connection test")
        .await
        .unwrap();
    let secret = format!(
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
            .exchange_hosted_code(&code, "browser", &hosted::digest(&secret))
            .await
            .unwrap()
    );
    secret
}

#[allow(clippy::too_many_arguments)]
async fn callback(
    app: Shared,
    workspace: Uuid,
    signing_workspace: Uuid,
    account: &str,
    token: &str,
    phone: &str,
    sid: &str,
    endpoint: &str,
) -> Response {
    let mut fields = std::collections::BTreeMap::from([
        ("AccountSid", account.to_owned()),
        ("CallSid", sid.to_owned()),
        ("From", "+12025550299".into()),
        ("To", phone.to_owned()),
    ]);
    if endpoint == "dial-complete" {
        fields.insert("DialCallStatus", "completed".into());
        fields.insert("DialCallDuration", "24".into());
    }
    let mut mac = Hmac::<Sha1>::new_from_slice(token.as_bytes()).unwrap();
    mac.update(
        format!(
            "https://engine.example.com/webhooks/twilio/workspaces/{signing_workspace}/{endpoint}"
        )
        .as_bytes(),
    );
    for (key, value) in &fields {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(&fields)
        .finish();
    router(app)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/webhooks/twilio/workspaces/{workspace}/{endpoint}"
                ))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "x-twilio-signature",
                    STANDARD.encode(mac.finalize().into_bytes()),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires VISICONVERT_TEST_DATABASE_URL pointing to a disposable Postgres database"]
async fn shared_connections_verify_credentials_ownership_and_isolate_callbacks() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async {
        axum::serve(listener, Router::new().fallback(get(mock_twilio)))
            .await
            .unwrap()
    });
    let operator = "operator-key-for-test-at-least-32-characters";
    let app = Arc::new(App {
        store: store.clone(),
        api_key: operator.into(),
        public_url: "https://engine.example.com".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "d".repeat(64),
            cloud_origin: "https://cloud.example.com".into(),
        }),
        vault: Some(secrets::Vault::from_hex(&"e".repeat(64)).unwrap()),
        twilio_client: convert_adapters::twilio_api::Client::for_test(mock_origin),
    });
    let standalone = Arc::new(App {
        cloud: None,
        ..(*app).clone()
    });
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let one = bearer(&store, first).await;
    let two = bearer(&store, second).await;
    let sid = |digit: &str| format!("AC{}", digit.repeat(32));
    let credentials = |digit: &str, token: &str, revision: i64| json!({"account_sid":sid(digit),"auth_token":token.repeat(32),"revision":revision});
    let path = "/api/connections/twilio";

    // Authentication, configuration prerequisites, and both standalone/hosted paths.
    assert_eq!(
        api(app.clone(), "GET", path, "bad", json!(null)).await.0,
        StatusCode::UNAUTHORIZED
    );
    for candidate in [
        Arc::new(App {
            vault: None,
            ..(*app).clone()
        }),
        Arc::new(App {
            public_url: "http://127.0.0.1:3100".into(),
            ..(*app).clone()
        }),
    ] {
        assert_eq!(
            api(candidate, "PUT", path, &one, credentials("1", "a", 0))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(
        api(app.clone(), "PUT", path, &one, credentials("1", "0", 0))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        api(app.clone(), "GET", path, &one, json!(null)).await.1["connected"],
        false
    );
    assert_eq!(
        api(app.clone(), "PUT", path, &one, credentials("1", "a", 0))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            standalone.clone(),
            "PUT",
            path,
            operator,
            credentials("3", "c", 0)
        )
        .await
        .0,
        StatusCode::OK
    );
    let status = api(app.clone(), "GET", path, &one, json!(null)).await.1;
    assert_eq!(status["account_sid"], sid("1"));
    assert_eq!(status["revision"], 1);
    assert_eq!(
        status["voice_url"],
        format!("https://engine.example.com/webhooks/twilio/workspaces/{first}/voice")
    );
    assert!(!status.to_string().contains(&"a".repeat(32)));
    assert!(!status.to_string().contains("encrypted_token"));
    assert_eq!(
        api(app.clone(), "GET", path, &two, json!(null)).await.1["connected"],
        false
    );
    assert_eq!(
        api(app.clone(), "PUT", path, &one, credentials("1", "a", 0))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let throttled = api(app.clone(), "PUT", path, &one, credentials("1", "f", 1)).await;
    assert_eq!(throttled.0, StatusCode::BAD_GATEWAY);
    assert!(!throttled.1.to_string().contains("secret provider detail"));
    assert_eq!(
        api(app.clone(), "GET", path, &one, json!(null)).await.1["revision"],
        1
    );
    assert_eq!(
        api(app.clone(), "PUT", path, &two, credentials("2", "b", 0))
            .await
            .0,
        StatusCode::OK
    );
    let first_store = Store {
        pool: store.pool.clone(),
        workspace: first,
    };
    let saved = first_store.twilio_connection().await.unwrap().unwrap();
    assert!(!saved.encrypted_token.contains(&"a".repeat(32)));
    let wrong_key = Arc::new(App {
        vault: Some(secrets::Vault::from_hex(&"9".repeat(64)).unwrap()),
        ..(*app).clone()
    });
    assert_eq!(
        api(
            wrong_key,
            "GET",
            "/api/connections/twilio/numbers",
            &one,
            json!(null)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let inventory = api(
        app.clone(),
        "GET",
        "/api/connections/twilio/numbers",
        &one,
        json!(null),
    )
    .await;
    assert_eq!(inventory.0, StatusCode::OK);
    assert_eq!(inventory.1["numbers"][0]["phone_number"], "+12025550211");

    let number = |phone: &str| json!({"label":"Campaign","phone_number":phone,"destination":"+12025550298","source":"Google Ads","campaign":"Test"});
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers",
            &one,
            number("+12025550212")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for (bearer, phone) in [
        (&one[..], "+12025550211"),
        (&two[..], "+12025550212"),
        (operator, "+12025550213"),
    ] {
        assert_eq!(
            api(app.clone(), "POST", "/api/numbers", bearer, number(phone))
                .await
                .0,
            StatusCode::OK
        );
    }
    // Existing number ownership cannot silently follow an account replacement or disconnect.
    assert_eq!(
        api(app.clone(), "PUT", path, &one, credentials("2", "b", 1))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(app.clone(), "DELETE", path, &one, json!({"revision":1}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(app.clone(), "PUT", path, &one, credentials("1", "a", 1))
            .await
            .0,
        StatusCode::OK
    );
    let stale_number: NewNumber = serde_json::from_value(number("+12025550214")).unwrap();
    assert!(matches!(
        first_store
            .add_number_with_connection(&stale_number, Some(1))
            .await,
        Err(convert_store::Error::Conflict)
    ));

    let call_sid = format!("CA{}", Uuid::new_v4().simple());
    assert_eq!(
        callback(
            app.clone(),
            first,
            first,
            &sid("1"),
            &"b".repeat(32),
            "+12025550211",
            &call_sid,
            "voice"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        callback(
            app.clone(),
            second,
            first,
            &sid("1"),
            &"a".repeat(32),
            "+12025550211",
            &call_sid,
            "voice"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        callback(
            app.clone(),
            first,
            first,
            &sid("2"),
            &"a".repeat(32),
            "+12025550211",
            &call_sid,
            "voice"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        callback(
            app.clone(),
            first,
            first,
            &sid("1"),
            &"a".repeat(32),
            "+12025550212",
            &call_sid,
            "voice"
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let response = callback(
            app.clone(),
            first,
            first,
            &sid("1"),
            &"a".repeat(32),
            "+12025550211",
            &call_sid,
            "voice",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let xml = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(xml.contains("+12025550298"));
        assert!(xml.contains(&format!("/workspaces/{first}/dial-complete")));
    }
    assert_eq!(
        callback(
            app.clone(),
            first,
            first,
            &sid("1"),
            &"a".repeat(32),
            "+12025550211",
            &call_sid,
            "dial-complete"
        )
        .await
        .status(),
        StatusCode::OK
    );
    let leads = api(app.clone(), "GET", "/api/leads", &one, json!(null))
        .await
        .1;
    assert_eq!(leads.as_array().unwrap().len(), 1);
    assert_eq!(leads[0]["duration_seconds"], 24);
    assert_eq!(leads[0]["call_status"], "destination_answered");
    assert_eq!(
        api(app.clone(), "GET", "/api/leads", &two, json!(null))
            .await
            .1,
        json!([])
    );
    assert_eq!(
        callback(
            standalone.clone(),
            store.workspace,
            store.workspace,
            &sid("3"),
            &"c".repeat(32),
            "+12025550213",
            &format!("CA{}", Uuid::new_v4().simple()),
            "voice"
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Fresh process state decrypts persisted credentials using the retained server key.
    let restart = Arc::new((*app).clone());
    assert_eq!(
        api(
            restart,
            "GET",
            "/api/connections/twilio/numbers",
            &one,
            json!(null)
        )
        .await
        .0,
        StatusCode::OK
    );
    // Empty connections may be removed; no cross-workspace deletion is possible through the API.
    let empty = Uuid::new_v4();
    let empty_token = bearer(&store, empty).await;
    assert_eq!(
        api(
            app.clone(),
            "PUT",
            path,
            &empty_token,
            credentials("1", "a", 0)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            app.clone(),
            "DELETE",
            path,
            &empty_token,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(app.clone(), "GET", path, &one, json!(null)).await.1["connected"],
        true
    );
    assert_eq!(
        api(
            app.clone(),
            "PUT",
            path,
            &empty_token,
            credentials("1", "a", 0)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            app.clone(),
            "DELETE",
            path,
            &empty_token,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(
            app.clone(),
            "PUT",
            path,
            &empty_token,
            credentials("1", "a", 1)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    server.abort();
    // The entire database is disposable; retain failure evidence until its harness drops it.
}

/// Interactive QA only. Uses a disposable database and simulated Twilio; the
/// production binary has no provider-origin override. Stop with Ctrl-C.
#[tokio::test]
#[ignore = "manual browser fixture; requires VISICONVERT_BROWSER_FIXTURE_DIR"]
async fn manual_browser_fixture() {
    let Ok(directory) = std::env::var("VISICONVERT_BROWSER_FIXTURE_DIR") else {
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::set_current_dir(root).unwrap();
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async {
        axum::serve(listener, Router::new().fallback(get(mock_twilio)))
            .await
            .unwrap()
    });
    let operator = "browser-fixture-only-operator-key-32-characters";
    let workspace = Uuid::new_v4();
    let session = bearer(&store, workspace).await;
    let app = Arc::new(App {
        store,
        api_key: operator.into(),
        public_url: "https://engine.example.com".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "d".repeat(64),
            cloud_origin: "https://cloud.example.com".into(),
        }),
        vault: Some(secrets::Vault::from_hex(&"e".repeat(64)).unwrap()),
        twilio_client: convert_adapters::twilio_api::Client::for_test(origin),
    });
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        std::path::Path::new(&directory).join("browser-fixture.json"),
        json!({
            "operator": operator, "hosted_session": session, "workspace": workspace,
            "selfhost_account": format!("AC{}", "3".repeat(32)), "selfhost_token": "c".repeat(32),
            "hosted_account": format!("AC{}", "1".repeat(32)), "hosted_token": "a".repeat(32)
        })
        .to_string(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:43113")
        .await
        .unwrap();
    println!("Simulated Twilio browser fixture: http://127.0.0.1:43113");
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await
        .unwrap();
}
