use super::connection_tests::{api, bearer};
use super::*;
use axum::{body::Body, extract::OriginalUri};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use sha1::Sha1;
use tower::ServiceExt;

pub(super) const PROJECT: &str = "00000000-0000-4000-8000-000000000001";
pub(super) const TOKEN: &str = "fixture-api-token-not-a-credential";
pub(super) const SIGNING: &str = "fixture-signing-key-not-a-credential";
pub(super) async fn mock_signalwire(OriginalUri(uri): OriginalUri, headers: HeaderMap) -> Response {
    if !uri
        .path()
        .starts_with(&format!("/api/laml/2010-04-01/Accounts/{PROJECT}"))
        || headers.get("authorization").and_then(|h| h.to_str().ok())
            != Some(&format!(
                "Basic {}",
                STANDARD.encode(format!("{PROJECT}:{TOKEN}"))
            ))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"private":"never expose provider responses"})),
        )
            .into_response();
    }
    if uri.path().ends_with("/IncomingPhoneNumbers") {
        let query = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .collect::<std::collections::HashMap<_, _>>();
        let entries = if query.get("PhoneNumber").is_none_or(|p| p == "+12025550311") {
            vec![
                json!({"sid":"00000000-0000-4000-8000-000000000002", "account_sid":PROJECT, "phone_number":"+12025550311", "friendly_name":"SignalWire test line", "capabilities":{"voice":true}}),
            ]
        } else {
            vec![]
        };
        Json(json!({"incoming_phone_numbers": entries, "next_page_uri":null})).into_response()
    } else {
        Json(json!({"sid":PROJECT, "status":"active", "auth_token":TOKEN})).into_response()
    }
}
async fn callback(
    app: Shared,
    workspace: Uuid,
    endpoint: &str,
    signing: &str,
    fields: &std::collections::BTreeMap<&str, String>,
    query: &str,
) -> Response {
    let path = format!("/webhooks/signalwire/workspaces/{workspace}/{endpoint}");
    let mut mac = Hmac::<Sha1>::new_from_slice(signing.as_bytes()).unwrap();
    mac.update(format!("https://engine.example.com{path}").as_bytes());
    for (key, value) in fields {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    router(app)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{path}{query}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "x-signalwire-signature",
                    STANDARD.encode(mac.finalize().into_bytes()),
                )
                .body(Body::from(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable Postgres"]
async fn shared_signalwire_connections_and_calls() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async {
        axum::serve(listener, Router::new().fallback(get(mock_signalwire)))
            .await
            .unwrap()
    });
    let app = Arc::new(App {
        store: store.clone(),
        api_key: Uuid::new_v4().to_string(),
        public_url: "https://engine.example.com".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "d".repeat(64),
            cloud_origin: "https://cloud.example.com".into(),
            app_origin: None,
        }),
        vault: Some(secrets::Vault::from_hex(&"e".repeat(64)).unwrap()),
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::for_test(origin),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    });
    let workspace = Uuid::new_v4();
    let other = Uuid::new_v4();
    let session = bearer(&store, workspace).await;
    let other_session = bearer(&store, other).await;
    let tenant = Store {
        pool: store.pool.clone(),
        workspace,
    };
    let path = "/api/connections/signalwire";
    let credentials = |revision| json!({"space_host":"fixture.signalwire.com", "project_id":PROJECT, "api_token":TOKEN, "signing_key":SIGNING, "revision":revision});
    assert_eq!(
        api(app.clone(), "PUT", path, "bad", credentials(0)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let no_vault = Arc::new(App {
        vault: None,
        ..(*app).clone()
    });
    assert_eq!(
        api(no_vault, "PUT", path, &session, credentials(0)).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for (field, value) in [
        ("space_host", "https://evil.test"),
        ("space_host", "a.signalwire.com.evil.test"),
        ("api_token", SIGNING),
    ] {
        let mut body = credentials(0);
        body[field] = json!(value);
        let (status, response) = api(app.clone(), "PUT", path, &session, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!response.to_string().contains("never expose"));
    }
    assert_eq!(
        api(app.clone(), "PUT", path, &session, credentials(0))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api(app.clone(), "PUT", path, &session, credentials(0))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let state = api(app.clone(), "GET", path, &session, json!(null)).await.1;
    assert_eq!(state["connected"], true);
    assert!(state["last_webhook_at"].is_null());
    for secret in [TOKEN, SIGNING, "encrypted_token", "encrypted_signing_key"] {
        assert!(!state.to_string().contains(secret));
    }
    let c = tenant.signalwire_connection().await.unwrap().unwrap();
    assert!(!c.encrypted_token.contains(TOKEN));
    assert!(!c.encrypted_signing_key.contains(SIGNING));
    let vault = app.vault.as_ref().unwrap();
    assert!(
        vault
            .open_signalwire(
                other,
                &c.space_host,
                c.project_id,
                "api-token",
                &c.encrypted_token
            )
            .is_err()
    );
    assert!(
        vault
            .open_signalwire(
                workspace,
                &c.space_host,
                c.project_id,
                "signing-key",
                &c.encrypted_token
            )
            .is_err()
    );
    assert_eq!(
        api(app.clone(), "GET", path, &other_session, json!(null))
            .await
            .1["connected"],
        false
    );
    let number = |phone| json!({"provider":"signalwire","label":"Pilot","phone_number":phone,"destination":"+12025550398","source":"Yard signs","campaign":"Pilot"});
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers",
            &session,
            number("+12025550312")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers",
            &session,
            number("+12025550311")
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        tenant.numbers().await.unwrap()[0].provider,
        PhoneProvider::Signalwire
    );
    assert_eq!(
        api(app.clone(), "DELETE", path, &session, json!({"revision":1}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    // The other provider's connection can be rotated/disconnected with SignalWire numbers present.
    tenant
        .save_twilio_connection(&format!("AC{}", "1".repeat(32)), "fixture", 0)
        .await
        .unwrap();
    tenant.remove_twilio_connection(1).await.unwrap();
    assert!(
        tenant
            .save_signalwire_connection(
                "other.signalwire.com",
                c.project_id,
                &c.encrypted_signing_key,
                &c.encrypted_token,
                1
            )
            .await
            .is_err()
    );
    let call = Uuid::new_v4().to_string();
    let mut fields = std::collections::BTreeMap::from([
        ("AccountSid", PROJECT.into()),
        ("CallSid", call.clone()),
        ("From", "+12025550399".into()),
        ("To", "+12025550311".into()),
    ]);
    assert_eq!(
        callback(app.clone(), workspace, "voice", TOKEN, &fields, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        callback(app.clone(), other, "voice", SIGNING, &fields, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        !callback(
            app.clone(),
            workspace,
            "voice",
            SIGNING,
            &fields,
            "?extra=1"
        )
        .await
        .status()
        .is_success()
    );
    let mut wrong_project = fields.clone();
    wrong_project.insert("AccountSid", Uuid::new_v4().to_string());
    assert_eq!(
        callback(app.clone(), workspace, "voice", SIGNING, &wrong_project, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        let response = callback(app.clone(), workspace, "voice", SIGNING, &fields, "").await;
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
        assert!(xml.contains("+12025550398"));
        assert!(xml.contains("record=\"do-not-record\""));
        assert!(xml.contains(&format!("/signalwire/workspaces/{workspace}/dial-complete")));
    }
    assert!(
        api(app.clone(), "GET", path, &session, json!(null)).await.1["last_webhook_at"].is_string()
    );
    // An authenticated provider must not reuse a call ID to mutate another caller or number.
    let mut wrong_number = fields.clone();
    wrong_number.insert("To", "+12025550312".into());
    assert_eq!(
        callback(app.clone(), workspace, "voice", SIGNING, &wrong_number, "")
            .await
            .status(),
        StatusCode::CONFLICT
    );
    fields.insert("DialCallStatus", "completed".into());
    fields.insert("DialCallDuration", "24".into());
    assert_eq!(
        callback(
            app.clone(),
            workspace,
            "dial-complete",
            SIGNING,
            &fields,
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    fields.insert("CallStatus", "ringing".into());
    fields.insert("SequenceNumber", "1".into());
    assert_eq!(
        callback(app.clone(), workspace, "status", SIGNING, &fields, "")
            .await
            .status(),
        StatusCode::OK
    );
    let leads = api(app.clone(), "GET", "/api/leads", &session, json!(null))
        .await
        .1;
    assert_eq!(leads.as_array().unwrap().len(), 1);
    assert_eq!(leads[0]["call_status"], "destination_answered");
    assert_eq!(leads[0]["duration_seconds"], 24);
    assert_eq!(leads[0]["source"], "Yard signs");
    assert_eq!(
        api(
            app.clone(),
            "GET",
            "/api/leads",
            &other_session,
            json!(null)
        )
        .await
        .1,
        json!([])
    );
    // The same registered SignalWire number participates in the shared DNI attribution path.
    let site_origin = "https://signalwire-dni.example";
    let site = tenant
        .save_site(
            None,
            &convert_store::sites::SiteInput {
                name: "SignalWire DNI".into(),
                origin: site_origin.into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    let number_id = tenant.numbers().await.unwrap()[0].id;
    tenant
        .save_dni_pool(
            site,
            &convert_store::dni::PoolInput {
                fallback_phone: "+12025550398".into(),
                destination_phone: None,
                enabled: true,
                version: 0,
                number_ids: vec![number_id],
                dedicated_numbers: true,
            },
        )
        .await
        .unwrap();
    let visitor = Uuid::new_v4();
    tenant.capture_session(site, site_origin, visitor, &json!({"source":"newsletter", "utm_campaign":"SignalWire visit", "landing_url":"https://signalwire-dni.example/quote"})).await.unwrap();
    assert_eq!(
        tenant
            .allocate_dni(site, site_origin, visitor)
            .await
            .unwrap()["status"],
        "assigned"
    );
    let mut dni_fields = fields.clone();
    dni_fields.insert("CallSid", Uuid::new_v4().to_string());
    assert_eq!(
        callback(app.clone(), workspace, "voice", SIGNING, &dni_fields, "")
            .await
            .status(),
        StatusCode::OK
    );
    let leads = tenant.leads().await.unwrap();
    assert!(
        leads
            .iter()
            .any(|l| l.source == "newsletter" && l.campaign == "SignalWire visit")
    );
    // A Twilio webhook cannot create a lead for a SignalWire-owned number.
    assert!(
        tenant
            .call_event(&CallEvent {
                sid: format!("CA{}", Uuid::new_v4().simple()),
                to: "+12025550311".into(),
                from: "+12025550399".into(),
                fingerprint: "wrong-provider".into(),
                kind: CallEventKind::Voice
            })
            .await
            .is_err()
    );
    // Replacing the signing key invalidates the old key and clears callback readiness.
    let mut replacement = credentials(1);
    replacement["signing_key"] = json!("replacement-fixture-signing-key");
    assert_eq!(
        api(app.clone(), "PUT", path, &session, replacement).await.0,
        StatusCode::OK
    );
    assert!(
        api(app.clone(), "GET", path, &session, json!(null)).await.1["last_webhook_at"].is_null()
    );
    assert_eq!(
        callback(app.clone(), workspace, "voice", SIGNING, &fields, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        callback(
            app.clone(),
            workspace,
            "voice",
            "replacement-fixture-signing-key",
            &fields,
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Standalone uses exactly the same API and encrypted storage.
    let standalone = Arc::new(App {
        cloud: None,
        ..(*app).clone()
    });
    assert_eq!(
        api(
            standalone.clone(),
            "PUT",
            path,
            &app.api_key,
            credentials(0)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, key) = api(
        standalone.clone(),
        "POST",
        "/api/api-keys",
        &app.api_key,
        json!({"name":"SignalWire reader","scopes":["read"],"expires_at":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let secret = key["secret"].as_str().unwrap();
    for route in [
        "/api/v1/connections/signalwire",
        "/api/v1/connections/signalwire/numbers",
    ] {
        let (status, body) = api(standalone.clone(), "GET", route, secret, json!(null)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!body.to_string().contains(TOKEN));
        assert!(!body.to_string().contains(SIGNING));
    }
    assert_eq!(
        api(
            standalone.clone(),
            "POST",
            "/api/v1/numbers",
            secret,
            number("+12025550311")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        !api(
            standalone.clone(),
            "PUT",
            "/api/v1/connections/signalwire",
            secret,
            credentials(1)
        )
        .await
        .0
        .is_success()
    );
    assert!(
        !api(standalone.clone(), "PUT", path, secret, credentials(1))
            .await
            .0
            .is_success()
    );
    assert_eq!(
        api(
            standalone.clone(),
            "DELETE",
            path,
            &app.api_key,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            standalone.clone(),
            "PUT",
            path,
            &app.api_key,
            credentials(0)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(
            standalone,
            "DELETE",
            path,
            &app.api_key,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // A managed suspension returns successful Hangup XML only after signature
    // verification. Completion receipts remain available for final metering.
    sqlx::query("UPDATE signalwire_connections SET managed=true WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    let response = callback(
        app.clone(),
        workspace,
        "voice",
        "replacement-fixture-signing-key",
        &fields,
        "",
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
    assert_eq!(xml, twilio::HANGUP);
    assert_eq!(
        callback(
            app.clone(),
            workspace,
            "voice",
            "invalid-signature",
            &fields,
            ""
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    fields.insert("CallStatus", "completed".into());
    fields.insert("SequenceNumber", "20".into());
    assert_eq!(
        callback(
            app.clone(),
            workspace,
            "status",
            "replacement-fixture-signing-key",
            &fields,
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    let policy_store = Store {
        pool: store.pool.clone(),
        workspace,
    };
    policy_store
        .apply_service_policy(&convert_store::service_policy::ServicePolicy {
            project_id: Uuid::parse_str(PROJECT).unwrap(),
            revision: 1,
            state: "active".into(),
            reason: "paid".into(),
            valid_until: chrono::Utc::now().timestamp() + 3600,
            release_at: None,
        })
        .await
        .unwrap();
    let response = callback(
        app.clone(),
        workspace,
        "voice",
        "replacement-fixture-signing-key",
        &fields,
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec()
        )
        .unwrap()
        .contains("<Dial")
    );
    server.abort();
}
