use super::connection_tests::{api, bearer};
use super::signalwire_tests::{PROJECT, SIGNING, TOKEN};
use super::*;
use axum::{body::Body, extract::OriginalUri, http::Method};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use std::sync::Mutex;
use tower::ServiceExt;

const TAKEN: &str = "+12025550777";
/// Carrier calls the mock received: (method, path, form body).
static CARRIER: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

async fn mock_signalwire(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prefix = format!("/api/laml/2010-04-01/Accounts/{PROJECT}");
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some(&format!(
            "Basic {}",
            STANDARD.encode(format!("{PROJECT}:{TOKEN}"))
        ))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = uri.path().strip_prefix(&prefix).unwrap_or("?").to_string();
    let form = String::from_utf8(body.to_vec()).unwrap();
    CARRIER
        .lock()
        .unwrap()
        .push((method.to_string(), path.clone(), form.clone()));
    match (method.as_str(), path.as_str()) {
        ("GET", "/IncomingPhoneNumbers") => {
            Json(json!({"incoming_phone_numbers": [], "next_page_uri": null})).into_response()
        }
        ("GET", "/AvailablePhoneNumbers/US/Local") => Json(json!({"available_phone_numbers": [
            {"phone_number":"+12025550411","locality":"Washington","region":"DC"},
            {"phone_number":"+12025550412","locality":"Washington","region":"DC"},
            {"phone_number":"not a number"}
        ]}))
        .into_response(),
        ("POST", "/IncomingPhoneNumbers") => {
            let fields: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(form.as_bytes())
                    .into_owned()
                    .collect();
            if fields["PhoneNumber"] == TAKEN {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"message":"unavailable"})),
                )
                    .into_response();
            }
            (
                StatusCode::CREATED,
                Json(json!({"sid": Uuid::new_v4(), "account_sid": PROJECT,
                    "phone_number": fields["PhoneNumber"], "friendly_name": fields["FriendlyName"]})),
            )
                .into_response()
        }
        ("DELETE", p) if p.starts_with("/IncomingPhoneNumbers/") => {
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn carrier(method: &str, path: &str) -> Vec<String> {
    CARRIER
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _)| m == method && p.starts_with(path))
        .map(|(_, _, form)| form.clone())
        .collect()
}

async fn internal(app: Shared, workspace: Uuid, key: &str, body: Value) -> StatusCode {
    router(app)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/internal/cloud/workspaces/{workspace}/signalwire"))
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn numbers(app: Shared, session: &str) -> Vec<Value> {
    let response = router(app)
        .oneshot(
            Request::builder()
                .uri("/api/numbers")
                .header("authorization", format!("Bearer {session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<Vec<Value>>(&body).unwrap()
}

fn set_env(name: &str, value: Option<&str>) {
    // Tests run single-threaded against the disposable database.
    unsafe {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Postgres"]
async fn managed_numbers_purchase_hold_and_release() {
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
        axum::serve(listener, Router::new().fallback(mock_signalwire))
            .await
            .unwrap()
    });
    let service_key = "d".repeat(64);
    let app = Arc::new(App {
        store: store.clone(),
        api_key: Uuid::new_v4().to_string(),
        public_url: "https://engine.example.com".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: service_key.clone(),
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
    let session = bearer(&store, workspace).await;
    let other_session = bearer(&store, Uuid::new_v4()).await;
    set_env("VISICONVERT_PURCHASE_LIMIT", None);
    set_env("VISICONVERT_RELEASE_HOLD_DAYS", None);

    // Nothing to buy into until Cloud hands over the subproject.
    let search = json!({"area_code":"202"});
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/search",
            &session,
            search.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let managed = json!({"space_host":"fixture.signalwire.com","project_id":PROJECT,"api_token":TOKEN,"signing_key":SIGNING});
    assert_eq!(
        internal(app.clone(), workspace, "wrong", managed.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        internal(app.clone(), Uuid::new_v4(), &service_key, managed.clone()).await,
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        assert_eq!(
            internal(app.clone(), workspace, &service_key, managed.clone()).await,
            StatusCode::OK
        );
    }
    let state = api(
        app.clone(),
        "GET",
        "/api/connections/signalwire",
        &session,
        json!(null),
    )
    .await
    .1;
    assert_eq!(state["managed"], true);
    assert_eq!(state["can_disconnect"], false);
    assert!(!state.to_string().contains(TOKEN));
    let owner_edit = json!({"space_host":"fixture.signalwire.com","project_id":PROJECT,"api_token":TOKEN,"signing_key":SIGNING,"revision":state["revision"]});
    assert_eq!(
        api(
            app.clone(),
            "PUT",
            "/api/connections/signalwire",
            &session,
            owner_edit
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(
            app.clone(),
            "DELETE",
            "/api/connections/signalwire",
            &session,
            json!({"revision":state["revision"]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // Search.
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/search",
            &session,
            json!({"area_code":"2a2"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, found) = api(
        app.clone(),
        "POST",
        "/api/numbers/search",
        &session,
        search.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["numbers"].as_array().unwrap().len(), 2);
    assert_eq!(found["numbers"][0]["locality"], "Washington");

    // Purchase is idempotent and points the number at this workspace.
    let request = Uuid::new_v4();
    let buy = |request: Uuid, phone: &str| json!({"request_id":request,"phone_number":phone,"destination":"+12025550398","label":"LA pool 1","source":"Website","campaign":"Pool"});
    let (status, bought) = api(
        app.clone(),
        "POST",
        "/api/numbers/purchase",
        &session,
        buy(request, "+12025550411"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bought}");
    let posts = carrier("POST", "/IncomingPhoneNumbers");
    assert_eq!(posts.len(), 1);
    let voice =
        format!("https://engine.example.com/webhooks/signalwire/workspaces/{workspace}/voice");
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(posts[0].as_bytes())
            .into_owned()
            .collect();
    assert_eq!(form["VoiceUrl"], voice);
    assert_eq!(form["StatusCallback"], voice.replace("/voice", "/status"));
    let (status, again) = api(
        app.clone(),
        "POST",
        "/api/numbers/purchase",
        &session,
        buy(request, "+12025550411"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], bought["id"]);
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), 1);
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(request, "+12025550412")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // Another tenant cannot buy into this subproject.
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &other_session,
            buy(Uuid::new_v4(), "+12025550412")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // A number someone else bought first fails cleanly, and its request is spent.
    let lost = Uuid::new_v4();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(lost, TAKEN)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(lost, TAKEN)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // The cap counts live purchased numbers.
    set_env("VISICONVERT_PURCHASE_LIMIT", Some("1"));
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(Uuid::new_v4(), "+12025550412")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    set_env("VISICONVERT_PURCHASE_LIMIT", None);

    let listed = numbers(app.clone(), &session).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["purchased"], true);
    assert!(listed[0]["release_requested_at"].is_null());

    // Release starts a hold that can be undone, then the carrier release runs.
    let id = bought["id"].as_str().unwrap();
    let release = format!("/api/numbers/{id}/release");
    let restore = format!("/api/numbers/{id}/restore");
    assert_eq!(
        api(app.clone(), "POST", &release, &other_session, json!(null))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        api(app.clone(), "POST", &release, &session, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    assert!(!numbers(app.clone(), &session).await[0]["release_requested_at"].is_null());
    assert_eq!(managed::release_due(&app).await.unwrap(), 0);
    assert_eq!(
        api(app.clone(), "POST", &restore, &session, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        api(app.clone(), "POST", &release, &session, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    set_env("VISICONVERT_RELEASE_HOLD_DAYS", Some("0"));
    assert_eq!(managed::release_due(&app).await.unwrap(), 1);
    set_env("VISICONVERT_RELEASE_HOLD_DAYS", None);
    assert_eq!(carrier("DELETE", "/IncomingPhoneNumbers/").len(), 1);
    assert!(numbers(app.clone(), &session).await.is_empty());
    assert_eq!(
        api(app.clone(), "POST", &restore, &session, json!(null))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // A released number can be bought again.
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(Uuid::new_v4(), "+12025550411")
        )
        .await
        .0,
        StatusCode::OK
    );

    // API keys need the separate numbers:purchase permission. Self-hosted operator keys
    // exercise the scope check without a Rails entitlement service.
    let app = Arc::new(App {
        cloud: None,
        ..(*app).clone()
    });
    for (scopes, expected) in [
        (json!(["read", "numbers:write"]), StatusCode::FORBIDDEN),
        (json!(["read", "numbers:purchase"]), StatusCode::CONFLICT),
    ] {
        let (status, key) = api(
            app.clone(),
            "POST",
            "/api/api-keys",
            &app.api_key,
            json!({"name":"Numbers","scopes":scopes,"expires_at":null}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{key}");
        let (status, body) = api(
            app.clone(),
            "POST",
            "/api/v1/numbers/search",
            key["secret"].as_str().unwrap(),
            search.clone(),
        )
        .await;
        assert_eq!(status, expected, "{body}");
    }
    server.abort();
}
