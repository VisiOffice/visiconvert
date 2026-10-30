use super::connection_tests::{api, bearer};
use super::signalwire_tests::{PROJECT, SIGNING, TOKEN};
use super::*;
use axum::{body::Body, extract::OriginalUri, http::Method};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use std::sync::Mutex;
use tower::ServiceExt;

const LOST: &str = "+12025550413";
const LOST_MANAGED: &str = "+12025550984";
static OWNED: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static FAIL_DELETE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const TAKEN: &str = "+12025550777";
/// Carrier calls the mock received: (method, path, form body).
static CARRIER: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

static BILLING_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESERVE_FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESERVE_REJECT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOKUP_FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESERVE_DELAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESERVE_BAD_IDENTITY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static ROUTING_RACE: Mutex<Option<(Uuid, String)>> = Mutex::new(None);
static USAGE_CALLS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static BAD_PAGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn mock_signalwire(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if uri.path() == "/internal/managed-number-access" {
        if !BILLING_ENABLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Json(json!({"allowed":false})).into_response();
        }
        if headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some(&format!("Bearer {}", "d".repeat(64)))
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let input: Value = serde_json::from_slice(&body).unwrap();
        CARRIER.lock().unwrap().push((
            "BILLING".into(),
            input["operation"].as_str().unwrap_or("check").into(),
            input.to_string(),
        ));
        if (input["operation"] == "reserve"
            && RESERVE_REJECT.load(std::sync::atomic::Ordering::SeqCst))
            || (input["operation"] == "lookup"
                && LOOKUP_FAIL.load(std::sync::atomic::Ordering::SeqCst))
        {
            return StatusCode::CONFLICT.into_response();
        }
        if input["operation"] == "reserve" && RESERVE_FAIL.load(std::sync::atomic::Ordering::SeqCst)
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if input["operation"] == "reserve"
            && RESERVE_DELAY.load(std::sync::atomic::Ordering::SeqCst)
        {
            tokio::time::sleep(std::time::Duration::from_secs(9)).await;
        }
        if input["operation"] == "reserve"
            && RESERVE_BAD_IDENTITY.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Json(json!({"workspace_id":Uuid::new_v4(),"project_id":input["project_id"],"request_id":input["request_id"],"reservation_id":321,"status":"held"})).into_response();
        }
        let status = if input["operation"] == "settle" {
            if input["outcome"] == "registered" {
                "committed"
            } else {
                "released"
            }
        } else {
            "held"
        };
        return Json(json!({"allowed":true,"workspace_id":input["workspace_id"],"project_id":input["project_id"],"request_id":input["request_id"],"reservation_id":321,"status":status,"amount_cents":200})).into_response();
    }
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
        ("GET", "/Calls") => Json(json!({"calls":USAGE_CALLS.lock().unwrap().clone(),"next_page_uri":if BAD_PAGE.load(std::sync::atomic::Ordering::SeqCst) {Some("https://evil.invalid/Calls")} else {None}})).into_response(),
        ("GET", p) if p.starts_with("/Calls/") => {
            let calls=USAGE_CALLS.lock().unwrap();
            match calls.iter().find(|c|c["sid"]==p.trim_start_matches("/Calls/")) {Some(c)=>Json(c.clone()).into_response(),None=>StatusCode::NOT_FOUND.into_response()}
        },
        ("GET", "/IncomingPhoneNumbers") => {
            let phone = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .find(|(k, _)| k == "PhoneNumber")
                .map(|(_, v)| v.into_owned());
            let owned: Vec<Value> = OWNED
                .lock()
                .unwrap()
                .iter()
                .filter(|n| phone.as_ref().is_none_or(|p| n["phone_number"] == *p))
                .cloned()
                .collect();
            Json(json!({"incoming_phone_numbers": owned, "next_page_uri": null})).into_response()
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
            let number = json!({"sid": Uuid::new_v4(), "account_sid": PROJECT,
                "phone_number": fields["PhoneNumber"], "friendly_name": fields["FriendlyName"], "capabilities":{"voice":true}});
            OWNED.lock().unwrap().push(number.clone());
            if fields["PhoneNumber"] == LOST || fields["PhoneNumber"] == LOST_MANAGED {
                // Carrier committed the purchase but the response was lost/failed.
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let race=ROUTING_RACE.lock().unwrap().take();
            if let Some((workspace,phone))=race {
                // Model another/legacy writer changing routing after preflight.
                // The registration-time guard must still reject and retain cleanup.
                let pool=sqlx::PgPool::connect(&std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap()).await.unwrap();
                sqlx::query("INSERT INTO tracking_numbers(id,workspace_id,label,phone_number,destination,source,campaign,provider) VALUES($1,$2,'Race fixture',$3,'+12025550800','Test','Test','signalwire')")
                    .bind(Uuid::new_v4()).bind(workspace).bind(phone).execute(&pool).await.unwrap();
                pool.close().await;
            }
            (StatusCode::CREATED, Json(number)).into_response()
        }
        ("DELETE", p) if p.starts_with("/IncomingPhoneNumbers/") => {
            if FAIL_DELETE.load(std::sync::atomic::Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let sid = p.strip_prefix("/IncomingPhoneNumbers/").unwrap();
            OWNED.lock().unwrap().retain(|n| n["sid"] != sid);
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
    BILLING_ENABLED.store(false, std::sync::atomic::Ordering::SeqCst);
    RESERVE_FAIL.store(false, std::sync::atomic::Ordering::SeqCst);
    OWNED.lock().unwrap().clear();
    CARRIER.lock().unwrap().clear();
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
            cloud_origin: origin.clone(),
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

    // Managed spending cannot use ordinary workspace access or test billing.
    let blocked = json!({"request_id":Uuid::new_v4(),"phone_number":"+12025550411","destination":"+12025550398","label":"Blocked","source":"Website","campaign":"Pool"});
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            blocked
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(carrier("POST", "/IncomingPhoneNumbers").is_empty());
    // Continue lifecycle coverage using a BYOC project: its customer pays the carrier.
    sqlx::query("UPDATE signalwire_connections SET managed=false WHERE workspace_id=$1")
        .bind(workspace)
        .execute(&store.pool)
        .await
        .unwrap();
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
    let hosted_app = app.clone();
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
    let app = hosted_app;
    let workspace_store = Store {
        pool: store.pool.clone(),
        workspace,
    };
    // Persist an ambiguous carrier success, recover it without a second purchase.
    let lost_request = Uuid::new_v4();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(lost_request, LOST)
        )
        .await
        .0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(
        workspace_store.purchase_status(lost_request).await.unwrap()["status"],
        "pending"
    );
    let posts = carrier("POST", "/IncomingPhoneNumbers").len();
    let mut changed = buy(lost_request, LOST);
    changed["destination"] = json!("+12025550399");
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            changed
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
            buy(Uuid::new_v4(), LOST)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // Reserve capacity while the carrier result is uncertain; no second request
    // or manual registration may sidestep the recovery receipt.
    set_env("VISICONVERT_PURCHASE_LIMIT", Some("2"));
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(Uuid::new_v4(), "+12025550419")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    set_env("VISICONVERT_PURCHASE_LIMIT", None);
    let c = workspace_store
        .signalwire_connection()
        .await
        .unwrap()
        .unwrap();
    let manual = NewNumber {
        provider: PhoneProvider::Signalwire,
        phone_number: LOST.into(),
        destination: "+12025550398".into(),
        label: "Manual".into(),
        source: "Website".into(),
        campaign: "Pool".into(),
    };
    assert!(
        workspace_store
            .add_number_with_connection(&manual, Some(c.revision))
            .await
            .is_err()
    );
    let path = format!("/api/numbers/purchases/{lost_request}");
    assert_eq!(
        api(app.clone(), "GET", &path, &other_session, json!(null))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Never adopt another number based only on its phone; the request marker is required.
    let marker = OWNED
        .lock()
        .unwrap()
        .iter()
        .find(|n| n["phone_number"] == LOST)
        .unwrap()["friendly_name"]
        .clone();
    OWNED
        .lock()
        .unwrap()
        .iter_mut()
        .find(|n| n["phone_number"] == LOST)
        .unwrap()["friendly_name"] = json!("Unrelated carrier number");
    age_purchases(&store, workspace).await;
    managed::reconcile_purchases(&app).await.unwrap();
    assert_eq!(
        workspace_store.purchase_status(lost_request).await.unwrap()["status"],
        "pending"
    );
    OWNED
        .lock()
        .unwrap()
        .iter_mut()
        .find(|n| n["phone_number"] == LOST)
        .unwrap()["friendly_name"] = marker;
    age_purchases(&store, workspace).await;
    managed::reconcile_purchases(&app).await.unwrap();
    let recovered = workspace_store.purchase_status(lost_request).await.unwrap();
    assert_eq!(recovered["status"], "registered");
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), posts);

    // Known conflicts must not reach the carrier or create an intent.
    let before = carrier("POST", "/IncomingPhoneNumbers").len();
    let invalid_id = Uuid::new_v4();
    let mut invalid = buy(invalid_id, "+12025550414");
    invalid["destination"] = json!(LOST);
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            invalid
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), before);
    assert!(matches!(
        workspace_store.purchase_status(invalid_id).await,
        Err(convert_store::Error::NotFound)
    ));

    // A genuine post-preflight race still queues cleanup. Failed DELETE retains it.
    *ROUTING_RACE.lock().unwrap() = Some((workspace, "+12025550666".into()));
    let cleanup = Uuid::new_v4();
    let mut invalid = buy(cleanup, "+12025550414");
    invalid["destination"] = json!("+12025550666");
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            invalid
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        workspace_store.purchase_status(cleanup).await.unwrap()["status"],
        "cleanup"
    );
    FAIL_DELETE.store(true, std::sync::atomic::Ordering::SeqCst);
    age_purchases(&store, workspace).await;
    managed::reconcile_purchases(&app).await.unwrap();
    assert_eq!(
        workspace_store.purchase_status(cleanup).await.unwrap()["status"],
        "cleanup"
    );
    FAIL_DELETE.store(false, std::sync::atomic::Ordering::SeqCst);
    age_purchases(&store, workspace).await;
    managed::reconcile_purchases(&app).await.unwrap();
    assert_eq!(
        workspace_store.purchase_status(cleanup).await.unwrap()["status"],
        "failed"
    );
    assert!(
        !OWNED
            .lock()
            .unwrap()
            .iter()
            .any(|n| n["phone_number"] == "+12025550414")
    );

    let attempts:i64=sqlx::query_scalar("SELECT count(*) FROM number_purchases WHERE workspace_id=$1 AND carrier_started_at >= now()-interval '1 hour'").bind(workspace).fetch_one(&store.pool).await.unwrap();
    assert!(
        attempts >= 3,
        "Failed carrier attempts and cleanup stay in the rolling budget"
    );
    set_env(
        "VISICONVERT_PURCHASE_ATTEMPT_LIMIT",
        Some(&attempts.to_string()),
    );
    let before = carrier("POST", "/IncomingPhoneNumbers").len();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(Uuid::new_v4(), "+12025550419")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), before);
    // Existing successful receipt is still idempotent when all new spend is blocked.
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            buy(lost_request, LOST)
        )
        .await
        .0,
        StatusCode::OK
    );
    set_env("VISICONVERT_PURCHASE_ATTEMPT_LIMIT", None);

    let restored = Uuid::parse_str(recovered["number_id"].as_str().unwrap()).unwrap();
    workspace_store.request_release(restored).await.unwrap();
    // Worker snapshots before a successful restore; its subsequent claim must fail.
    let snapshot = convert_store::managed::due_releases(&store.pool, 0)
        .await
        .unwrap();
    assert!(snapshot.iter().any(|n| n.number_id == restored));
    workspace_store.restore_number(restored).await.unwrap();
    assert!(!workspace_store.claim_release(restored, 0).await.unwrap());
    workspace_store.request_release(restored).await.unwrap();
    assert!(workspace_store.claim_release(restored, 0).await.unwrap());
    assert!(workspace_store.restore_number(restored).await.is_err());
    FAIL_DELETE.store(true, std::sync::atomic::Ordering::SeqCst);
    set_env("VISICONVERT_RELEASE_HOLD_DAYS", Some("0"));
    assert_eq!(managed::release_due(&app).await.unwrap(), 0);
    assert!(workspace_store.restore_number(restored).await.is_err());
    FAIL_DELETE.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(managed::release_due(&app).await.unwrap(), 1);
    set_env("VISICONVERT_RELEASE_HOLD_DAYS", None);

    // Static published fallbacks cannot be released, even on a disabled pool.
    let active = workspace_store
        .numbers()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.phone_number == "+12025550411")
        .unwrap();
    let site = Uuid::new_v4();
    sqlx::query("INSERT INTO sites(id,workspace_id,name,origin,allowed_fields) VALUES($1,$2,'Fixture','https://fixture.example','{}')").bind(site).bind(workspace).execute(&store.pool).await.unwrap();
    let pool = convert_store::dni::PoolInput {
        fallback_phone: active.phone_number.clone(),
        destination_phone: Some(active.destination.clone()),
        enabled: false,
        number_ids: vec![],
        dedicated_numbers: false,
        version: 0,
    };
    workspace_store.save_dni_pool(site, &pool).await.unwrap();
    assert!(workspace_store.request_release(active.id).await.is_err());
    sqlx::query("UPDATE dni_pools SET fallback_phone='+12025550177' WHERE site_id=$1")
        .bind(site)
        .execute(&store.pool)
        .await
        .unwrap();
    workspace_store.request_release(active.id).await.unwrap();
    assert!(
        workspace_store
            .save_dni_pool(site, &convert_store::dni::PoolInput { version: 1, ..pool })
            .await
            .is_err()
    );
    server.abort();
}

async fn age_purchases(store: &Store, workspace: Uuid) {
    sqlx::query("UPDATE number_purchases SET last_attempt_at=now()-interval '2 minutes' WHERE workspace_id=$1").bind(workspace).execute(&store.pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Postgres"]
async fn managed_budget_and_verified_quantities() {
    use chrono::{Duration as Delta, Utc};
    use std::sync::atomic::Ordering::SeqCst;
    OWNED.lock().unwrap().clear();
    CARRIER.lock().unwrap().clear();
    USAGE_CALLS.lock().unwrap().clear();
    BILLING_ENABLED.store(true, SeqCst);
    RESERVE_FAIL.store(false, SeqCst);
    BAD_PAGE.store(false, SeqCst);
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
            cloud_origin: origin.clone(),
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
    assert_eq!(internal(app.clone(),workspace,&service_key,json!({"space_host":"fixture.signalwire.com","project_id":PROJECT,"api_token":TOKEN,"signing_key":SIGNING})).await,StatusCode::OK);
    let request = Uuid::new_v4();
    let buy = json!({"request_id":request,"phone_number":"+12025550981","destination":"+12025550398","label":"Metered","source":"Website","campaign":"Pool"});
    let (status, value) = api(
        app.clone(),
        "POST",
        "/api/numbers/purchase",
        &session,
        buy.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let events = CARRIER.lock().unwrap().clone();
    let reserve = events
        .iter()
        .position(|(m, p, _)| m == "BILLING" && p == "reserve")
        .unwrap();
    let purchase = events
        .iter()
        .position(|(m, p, _)| m == "POST" && p == "/IncomingPhoneNumbers")
        .unwrap();
    assert!(reserve < purchase);
    assert_eq!(
        api(app.clone(), "POST", "/api/numbers/purchase", &session, buy)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), 1);
    age_purchases(&store, workspace).await;
    managed::settle_reservations(&app).await.unwrap();
    let settled:bool=sqlx::query_scalar("SELECT reservation_settled_at IS NOT NULL AND reservation_id=321 FROM number_purchases WHERE workspace_id=$1 AND request_id=$2").bind(workspace).bind(request).fetch_one(&store.pool).await.unwrap();
    assert!(settled);
    sqlx::query(
        "UPDATE tracking_numbers SET purchased_at=now()-interval '2 hours' WHERE workspace_id=$1",
    )
    .bind(workspace)
    .execute(&store.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE number_purchases SET created_at=now()-interval '2 hours' WHERE workspace_id=$1",
    )
    .bind(workspace)
    .execute(&store.pool)
    .await
    .unwrap();
    let now = Utc::now();
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    let root = json!({"sid":parent,"account_sid":PROJECT,"direction":"inbound","to":"+12025550981","status":"completed","start_time":(now-Delta::hours(1)).to_rfc2822(),"end_time":(now-Delta::minutes(58)).to_rfc2822(),"duration":120});
    let call = json!({"sid":child,"account_sid":PROJECT,"direction":"outbound-dial","parent_call_sid":parent,"status":"completed","end_time":(now-Delta::minutes(59)).to_rfc2822(),"duration":"60"});
    *USAGE_CALLS.lock().unwrap() = vec![root, call];
    let path = format!("/internal/cloud/workspaces/{workspace}/usage");
    let input = json!({"project_id":PROJECT,"period_start":(now-Delta::days(1)).timestamp(),"period_end":(now+Delta::days(29)).timestamp()});
    assert_eq!(
        api(app.clone(), "POST", &path, "wrong", input.clone())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, report) = api(app.clone(), "POST", &path, &service_key, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["complete"], true);
    assert_eq!(report["calls"].as_array().unwrap().len(), 1);
    assert_eq!(report["calls"][0]["connected_seconds"], 60);
    assert_eq!(report["numbers"].as_array().unwrap().len(), 1);
    assert!(!report.to_string().contains("+12025550398"));
    BAD_PAGE.store(true, SeqCst);
    assert_ne!(
        api(app.clone(), "POST", &path, &service_key, input.clone())
            .await
            .0,
        StatusCode::OK
    );
    BAD_PAGE.store(false, SeqCst);
    USAGE_CALLS.lock().unwrap()[1]["account_sid"] = json!(Uuid::new_v4());
    assert_ne!(
        api(app.clone(), "POST", &path, &service_key, input.clone())
            .await
            .0,
        StatusCode::OK
    );
    USAGE_CALLS.lock().unwrap().clear();
    OWNED
        .lock()
        .unwrap()
        .push(json!({"sid":Uuid::new_v4(),"account_sid":PROJECT,"phone_number":"+12025550982"}));
    assert_ne!(
        api(app.clone(), "POST", &path, &service_key, input).await.0,
        StatusCode::OK
    );
    // The carrier bought the number but its response was lost. Recovery uses
    // the marker and existing reservation; it must never POST the purchase twice.
    let lost_request = Uuid::new_v4();
    let lost = json!({"request_id":lost_request,"phone_number":LOST_MANAGED,"destination":"+12025550398","label":"Lost response","source":"Website","campaign":"Pool"});
    assert_eq!(
        api(app.clone(), "POST", "/api/numbers/purchase", &session, lost)
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
    age_purchases(&store, workspace).await;
    managed::reconcile_purchases(&app).await.unwrap();
    let receipt = Store {
        pool: store.pool.clone(),
        workspace,
    }
    .purchase_status(lost_request)
    .await
    .unwrap();
    assert_eq!(receipt["status"], "registered");
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), 2);
    let ws = Store {
        pool: store.pool.clone(),
        workspace,
    };
    let connection = ws.signalwire_connection().await.unwrap().unwrap();
    let payload = |id, phone: &str| json!({"request_id":id,"phone_number":phone,"destination":"+12025550398","label":"Recovery","source":"Website","campaign":"Pool"});
    // Definite reserve rejection frees the intent and metering, with no lookup hold.
    RESERVE_REJECT.store(true, SeqCst);
    let rejected = Uuid::new_v4();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            payload(rejected, "+12025550983")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let receipt = ws.purchase_status(rejected).await.unwrap();
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt["billing_reconciliation_pending"], false);
    assert!(
        ws.metered_numbers(Uuid::parse_str(PROJECT).unwrap())
            .await
            .is_ok()
    );
    let requests = carrier("BILLING", "reserve").len();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            payload(rejected, "+12025550983")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(carrier("BILLING", "reserve").len(), requests);
    RESERVE_REJECT.store(false, SeqCst);
    // Lost/5xx reserve response never purchases; a lookup failure does not free its hold.
    RESERVE_FAIL.store(true, SeqCst);
    let unknown = Uuid::new_v4();
    assert_eq!(
        api(
            app.clone(),
            "POST",
            "/api/numbers/purchase",
            &session,
            payload(unknown, "+12025550983")
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        ws.purchase_status(unknown).await.unwrap()["status"],
        "failed"
    );
    assert_eq!(
        ws.purchase_status(unknown).await.unwrap()["billing_reconciliation_pending"],
        true
    );
    assert!(
        ws.metered_numbers(Uuid::parse_str(PROJECT).unwrap())
            .await
            .is_ok()
    );
    LOOKUP_FAIL.store(true, SeqCst);
    age_purchases(&store, workspace).await;
    managed::settle_reservations(&app).await.unwrap();
    assert_eq!(
        ws.purchase_status(unknown).await.unwrap()["billing_reconciliation_pending"],
        true
    );
    LOOKUP_FAIL.store(false, SeqCst);
    RESERVE_FAIL.store(false, SeqCst);
    age_purchases(&store, workspace).await;
    managed::settle_reservations(&app).await.unwrap();
    assert_eq!(
        ws.purchase_status(unknown).await.unwrap()["billing_reconciliation_pending"],
        false
    );
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), 2);

    // Crash recovery before reserve, during reserve, and after a confirmed hold.
    for (index, phase) in ["prepared", "reserving", "reserved"].iter().enumerate() {
        let id = Uuid::new_v4();
        let n = NewNumber {
            provider: PhoneProvider::Signalwire,
            phone_number: format!("+1202555099{index}"),
            destination: "+12025550398".into(),
            label: "Crash".into(),
            source: "Test".into(),
            campaign: "Test".into(),
        };
        assert!(matches!(
            ws.begin_purchase(id, &n, &connection, 25, 25)
                .await
                .unwrap(),
            convert_store::managed::Purchase::Started
        ));
        if *phase != "prepared" {
            ws.start_reservation(id).await.unwrap();
        }
        if *phase == "reserved" {
            ws.record_reservation(id, 321).await.unwrap();
        }
        age_purchases(&store, workspace).await;
        managed::reconcile_purchases(&app).await.unwrap();
        assert_eq!(ws.purchase_status(id).await.unwrap()["status"], "failed");
        assert!(
            ws.start_carrier_purchase(id, 25).await.is_err(),
            "Late API task must not purchase after cancellation"
        );
        if *phase == "reserving" {
            ws.record_reservation(id, 321).await.unwrap();
        }
        age_purchases(&store, workspace).await;
        managed::settle_reservations(&app).await.unwrap();
        assert_eq!(
            ws.purchase_status(id).await.unwrap()["billing_reconciliation_pending"],
            false
        );
    }
    // Actual HTTP timeout and malformed identity retain a recoverable hold, never a purchase.
    for (index, delay) in [true, false].into_iter().enumerate() {
        RESERVE_DELAY.store(delay, SeqCst);
        RESERVE_BAD_IDENTITY.store(!delay, SeqCst);
        let id = Uuid::new_v4();
        assert_eq!(
            api(
                app.clone(),
                "POST",
                "/api/numbers/purchase",
                &session,
                payload(id, &format!("+1202555088{index}"))
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        RESERVE_DELAY.store(false, SeqCst);
        RESERVE_BAD_IDENTITY.store(false, SeqCst);
        age_purchases(&store, workspace).await;
        managed::reconcile_purchases(&app).await.unwrap();
        assert_eq!(ws.purchase_status(id).await.unwrap()["status"], "failed");
        age_purchases(&store, workspace).await;
        managed::settle_reservations(&app).await.unwrap();
        assert_eq!(
            ws.purchase_status(id).await.unwrap()["billing_reconciliation_pending"],
            false
        );
    }
    // Cancel versus irreversible claim: exactly one state wins, including a late caller.
    let racing = Uuid::new_v4();
    let rn = NewNumber {
        provider: PhoneProvider::Signalwire,
        phone_number: "+12025550996".into(),
        destination: "+12025550398".into(),
        label: "Race".into(),
        source: "Test".into(),
        campaign: "Test".into(),
    };
    ws.begin_purchase(racing, &rn, &connection, 25, 25)
        .await
        .unwrap();
    ws.start_reservation(racing).await.unwrap();
    ws.record_reservation(racing, 321).await.unwrap();
    let (claim, cancel) = tokio::join!(
        ws.start_carrier_purchase(racing, 25),
        ws.cancel_unstarted_purchase(racing)
    );
    cancel.unwrap();
    let state = ws.purchase_status(racing).await.unwrap();
    if claim.is_ok() {
        assert_eq!(state["phase"], "carrier");
        assert_eq!(state["status"], "pending");
        // Fixture proves no POST was issued; simulate a definite carrier rejection.
        ws.fail_purchase(racing).await.unwrap();
    } else {
        assert_eq!(state["status"], "failed");
    }
    assert!(ws.start_carrier_purchase(racing, 25).await.is_err());

    // Two requests racing for the same number cannot both reserve/buy it.
    let n = NewNumber {
        provider: PhoneProvider::Signalwire,
        phone_number: "+12025550995".into(),
        destination: "+12025550398".into(),
        label: "Concurrent".into(),
        source: "Test".into(),
        campaign: "Test".into(),
    };
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let (a, b) = tokio::join!(
        ws.begin_purchase(first, &n, &connection, 25, 25),
        ws.begin_purchase(second, &n, &connection, 25, 25)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    ws.cancel_unstarted_purchase(if a.is_ok() { first } else { second })
        .await
        .unwrap();
    assert_eq!(carrier("POST", "/IncomingPhoneNumbers").len(), 2);
    BILLING_ENABLED.store(false, SeqCst);
    RESERVE_FAIL.store(false, SeqCst);
    OWNED.lock().unwrap().clear();
    CARRIER.lock().unwrap().clear();
    USAGE_CALLS.lock().unwrap().clear();
    server.abort();
}

#[tokio::test]
async fn carrier_quantity_pagination_preserves_filters_and_rejects_duplicates() {
    use axum::extract::Query;
    use std::collections::HashMap;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let path = format!("/api/laml/2010-04-01/Accounts/{PROJECT}/Calls");
    let next = format!("{path}?Page=1&PageToken=PAfixture");
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route(&path,get(move |Query(q):Query<HashMap<String,String>>| {
            let next=next.clone();async move {
                assert_eq!(q.get("EndTime>").map(String::as_str),Some("boundary"));
                let first=q.get("Page").map(String::as_str)==Some("0");
                if !first { assert_eq!(q.get("PageToken").map(String::as_str),Some("PAfixture")); }
                let duplicate=q.contains_key("duplicate");
                Json(json!({"calls":[{"account_sid":PROJECT,"sid":if first || duplicate {"11111111-1111-4111-8111-111111111111"} else {"22222222-2222-4222-8222-222222222222"}}],"next_page_uri":if first {Some(next)} else {None}}))
            }
        }))).await.unwrap();
    });
    let client = convert_adapters::signalwire_api::Client::for_test(origin);
    assert_eq!(
        client
            .usage_records(
                "fixture.signalwire.com",
                PROJECT,
                TOKEN,
                "/Calls",
                &[("EndTime>", "boundary")]
            )
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        client
            .usage_records(
                "fixture.signalwire.com",
                PROJECT,
                TOKEN,
                "/Calls",
                &[("EndTime>", "boundary"), ("duplicate", "1")]
            )
            .await
            .is_err()
    );
    server.abort();
}
