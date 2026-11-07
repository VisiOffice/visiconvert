//! Meta Conversions API over the real HTTP boundary, a local mock of the Graph API and a
//! disposable PostgreSQL database. Nothing here reaches Meta.
use super::*;
use axum::{body::Body, extract::OriginalUri};
use http_body_util::BodyExt;
use pipeline_tests::{ORIGIN, World, action, post, world};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tower::ServiceExt;

const GOOD: &str = "EAAgoodMetaTokenForTests0123456789";
const BAD: &str = "EAAbadMetaTokenForTests01234567890";
const DATASET: &str = "1234567890123";

#[derive(Default)]
struct Mock {
    /// Bodies POSTed to /{dataset}/events.
    events: Mutex<Vec<Value>>,
    /// Reply for the next send: ok, garbage, rate, or error:<code>:<subcode>.
    mode: Mutex<String>,
}
async fn mock(
    State(m): State<Arc<Mock>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // The token never travels in a URL.
    assert!(!uri.to_string().contains("EAA"), "{uri}");
    let error = |code: i64, sub: i64| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"message":"PRIVATE_META_ERROR","type":"OAuthException","code":code,"error_subcode":sub,"fbtrace_id":"t"}})),
        )
            .into_response()
    };
    if auth != format!("Bearer {GOOD}") {
        return error(190, 0);
    }
    let path = uri.path();
    if path == format!("/v24.0/{DATASET}") {
        return Json(json!({"id":DATASET,"name":"Pipeline Pixel"})).into_response();
    }
    if path == "/v24.0/999" {
        return error(100, 33);
    }
    if path == format!("/v24.0/{DATASET}/events") {
        let v: Value = serde_json::from_slice(&body).unwrap();
        m.events.lock().unwrap().push(v);
        let mode = std::mem::replace(&mut *m.mode.lock().unwrap(), "ok".into());
        return match mode.as_str() {
            "garbage" => (StatusCode::INTERNAL_SERVER_ERROR, "<html>").into_response(),
            "rate" => error(4, 0),
            m if m.starts_with("error:") => {
                let mut p = m.split(':').skip(1).map(|s| s.parse().unwrap());
                error(p.next().unwrap(), p.next().unwrap())
            }
            _ => Json(json!({"events_received":1,"messages":[],"fbtrace_id":"AbCtrace123"}))
                .into_response(),
        };
    }
    StatusCode::NOT_FOUND.into_response()
}
async fn api(app: Shared, method: &str, path: &str, key: &str, v: Value) -> (StatusCode, Value) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    // Tokens and upstream messages never reach a response.
    for private in [GOOD, BAD, "PRIVATE_META"] {
        assert!(!text.contains(private), "{path}: {text}");
    }
    (code, serde_json::from_str(&text).unwrap_or(Value::Null))
}
async fn with_meta(a: &Shared) -> (Shared, Arc<Mock>, tokio::task::JoinHandle<()>) {
    let m = Arc::new(Mock::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock_app = Router::new().fallback(mock).with_state(m.clone());
    let task = tokio::spawn(async move { axum::serve(listener, mock_app).await.unwrap() });
    // Park Meta work left in this disposable database by earlier tests.
    for q in [
        "UPDATE meta_deliveries SET due_at=NULL WHERE state IN ('queued','retry')",
        "UPDATE meta_event_plans SET state='final' WHERE state='blocked'",
        "UPDATE conversion_actions SET meta_event_name=NULL,meta_mapped_at=NULL WHERE meta_event_name IS NOT NULL",
    ] {
        sqlx::query(q).execute(&a.store.pool).await.unwrap();
    }
    let mut app = (**a).clone();
    app.meta = meta::Meta {
        client: convert_adapters::meta_api::Client::for_test(origin),
    };
    (Arc::new(app), m, task)
}
async fn key(a: &Shared, scopes: &[&str]) -> String {
    a.store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: format!("meta {}", scopes.join(" ")),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string()
}
async fn meta_lead(w: &World, phone: &str, evidence: Value) -> Uuid {
    let session = Uuid::new_v4();
    let mut e =
        json!({"source":"facebook","consent":"granted","landing_url":format!("{ORIGIN}/quote")});
    e.as_object_mut()
        .unwrap()
        .extend(evidence.as_object().unwrap().clone());
    w.a.store
        .capture_visit(
            w.site,
            ORIGIN,
            session,
            &e,
            &convert_store::sites::Browser {
                user_agent: Some("Mozilla/5.0 (iPhone) Test".into()),
                ip: Some("203.0.113.9".parse().unwrap()),
            },
        )
        .await
        .unwrap();
    let fields = BTreeMap::from([("phone".to_string(), phone.to_string())]);
    w.a.store
        .capture_form(w.site, ORIGIN, session, &session.to_string(), &fields)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT lead_id FROM website_submissions WHERE site_id=$1 AND session_id=$2")
        .bind(w.site)
        .bind(session)
        .fetch_one(&w.a.store.pool)
        .await
        .unwrap()
}
async fn rows(a: &App, event: &Value) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(d) FROM meta_deliveries d WHERE workspace_id=$1 AND event_id=$2 ORDER BY created_at")
        .bind(a.store.workspace).bind(event["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_all(&a.store.pool).await.unwrap()
}
async fn drain(a: &App) -> usize {
    let mut n = 0;
    while meta::run_one(a).await.unwrap() {
        n += 1;
    }
    n
}
async fn version(a: &App, action: &str) -> Value {
    a.store
        .conversion_actions()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v["key"] == action)
        .unwrap()["version"]
        .clone()
}
async fn setup(a: &Shared, key: &str) -> Value {
    let (s, v) = api(a.clone(), "GET", "/api/v1/conversion-setup", key, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["meta"].clone()
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn meta_connect_check_send_errors_and_settings() {
    let w = world("not_required_outside_eea", true).await;
    let (a, m, task) = with_meta(&w.a).await;
    let w = World { a: a.clone(), ..w };
    let send = key(&a, &["read", "conversions:send", "conversions:write"]).await;
    let write = key(&a, &["read", "conversions:write"]).await;

    let meta = setup(&a, &send).await;
    assert_eq!(meta["connected"], false);
    assert_eq!(meta["ready"], false);
    assert!(
        meta["summary"].as_str().unwrap().contains("connect meta"),
        "{meta}"
    );

    // Saving the token needs conversions:send, and Meta must accept it for the dataset.
    let body = json!({"revision":0,"dataset_id":DATASET,"access_token":GOOD});
    let (s, _) = api(
        a.clone(),
        "PUT",
        "/api/v1/connections/meta",
        &write,
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/connections/meta",
        &send,
        json!({"revision":0,"dataset_id":DATASET,"access_token":BAD}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("rejected the access token")
    );
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/connections/meta",
        &send,
        json!({"revision":0,"dataset_id":"999","access_token":GOOD}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("cannot use that dataset")
    );
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/connections/meta",
        &send,
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["dataset"]["name"], "Pipeline Pixel");
    assert_eq!(v["token_fingerprint"].as_str().unwrap().len(), 12);
    let (s, _) = api(a.clone(), "PUT", "/api/v1/connections/meta", &send, body).await;
    assert_eq!(s, StatusCode::CONFLICT, "stale revision");
    let sealed: String =
        sqlx::query_scalar("SELECT encrypted_token FROM meta_connections WHERE workspace_id=$1")
            .bind(a.store.workspace)
            .fetch_one(&a.store.pool)
            .await
            .unwrap();
    assert!(!sealed.contains(GOOD));
    let (_, status) = api(
        a.clone(),
        "GET",
        "/api/v1/connections/meta",
        &write,
        json!({}),
    )
    .await;
    assert_eq!(status["connected"], true);
    assert_eq!(status["check_current"], true);

    // Map: standard or custom names, and include_since is bounded by Meta's 7 days.
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-actions/order/meta-event",
        &write,
        json!({"version":version(&a,"order").await,"event_name":"Not valid!"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = api(a.clone(), "PUT", "/api/v1/conversion-actions/order/meta-event", &write,
        json!({"version":version(&a,"order").await,"event_name":"Purchase","include_since":(chrono::Utc::now()-chrono::Duration::days(8)).to_rfc3339()})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-actions/order/meta-event",
        &write,
        json!({"version":version(&a,"order").await,"event_name":"Purchase"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["meta_event_name"], "Purchase");

    // Check with a test event code: one test event, which never counts.
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-setup/meta-check",
        &write,
        json!({"revision":1,"test_event_code":"bad code"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-setup/meta-check",
        &write,
        json!({"revision":1,"test_event_code":"TEST12345"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(v["test_event"]["sent"], true);
    {
        let sent = m.events.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["test_event_code"], "TEST12345");
        assert_eq!(sent[0]["data"][0]["event_name"], "Lead");
    }
    let meta = setup(&a, &send).await;
    assert_eq!(meta["ready"], true, "{meta}");
    assert_eq!(meta["mapped_actions"], 1);

    // A Meta click lead: one hashed, deduplicable event with the browser details.
    let lead = meta_lead(
        &w,
        "602-555-0300",
        json!({"fbclid":"IwAR_click_1","fbp":"fb.1.1700000000000.42"}),
    )
    .await;
    let order = json!({"action":"order","external_id":"meta_1","match":{"phone":"6025550300"},"value_minor":41250});
    let (s, v) = post(&w, order.clone()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["lead_id"], json!(lead));
    assert_eq!(v["delivery"]["meta"]["status"], "queued", "{v}");
    let first = v.clone();
    assert_eq!(drain(&a).await, 1);
    let r = rows(&a, &first).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["state"], "processed", "{}", r[0]);
    assert_eq!(r[0]["tracking_id"], "AbCtrace123");
    let body = m.events.lock().unwrap().last().unwrap().clone();
    assert!(body.get("test_event_code").is_none());
    let e = &body["data"][0];
    assert_eq!(e["event_name"], "Purchase");
    assert_eq!(
        e["event_id"],
        format!("vc-{}", first["id"].as_str().unwrap())
    );
    // A business event posted from the backend (CRM-style), not the website visit.
    assert_eq!(e["action_source"], "system_generated");
    assert!(e.get("event_source_url").is_none());
    assert_eq!(e["custom_data"], json!({"value":412.5,"currency":"USD"}));
    let u = &e["user_data"];
    assert_eq!(
        u["ph"][0],
        convert_adapters::meta_api::sha256_hex("16025550300")
    );
    assert!(
        u["fbc"].as_str().unwrap().starts_with("fb.1.")
            && u["fbc"].as_str().unwrap().ends_with(".IwAR_click_1")
    );
    assert_eq!(u["fbp"], "fb.1.1700000000000.42");
    assert_eq!(u["client_ip_address"], "203.0.113.9");
    assert_eq!(u["client_user_agent"], "Mozilla/5.0 (iPhone) Test");
    assert!(!body.to_string().contains("6025550300") && !body.to_string().contains("602-555"));
    assert!(e.get("data_processing_options").is_none(), "ldu off");

    // Meta cannot restate: a changed value is final, and nothing more is sent.
    let (_, v) = post(&w, json!({"action":"order","external_id":"meta_1","match":{"phone":"6025550300"},"value_minor":50000})).await;
    assert_eq!(v["delivery"]["meta"]["status"], "blocked", "{v}");
    assert!(
        v["delivery"]["meta"]["reason"]
            .as_str()
            .unwrap()
            .contains("cannot restate")
    );
    assert_eq!(drain(&a).await, 0);

    // Purchase needs a value.
    meta_lead(&w, "602-555-0301", json!({"fbclid":"IwAR_click_2"})).await;
    let (_, v) = post(
        &w,
        json!({"action":"order","external_id":"meta_2","match":{"phone":"6025550301"}}),
    )
    .await;
    assert!(
        v["delivery"]["meta"]["reason"]
            .as_str()
            .unwrap()
            .contains("requires a value"),
        "{v}"
    );

    // Errors: a rejected event fails; an ambiguous reply is uncertain and never resent.
    meta_lead(&w, "602-555-0302", json!({"fbclid":"IwAR_click_3"})).await;
    *m.mode.lock().unwrap() = "error:100:2804003".into();
    let (_, rejected) = post(&w, json!({"action":"order","external_id":"meta_3","match":{"phone":"6025550302"},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    let r = rows(&a, &rejected).await;
    assert_eq!(r[0]["state"], "failed");
    assert!(r[0]["diagnostic"].as_str().unwrap().contains("2804003"));
    assert!(r[0]["attempted_at"].is_null());
    meta_lead(&w, "602-555-0303", json!({"fbclid":"IwAR_click_4"})).await;
    *m.mode.lock().unwrap() = "garbage".into();
    let (_, unsure) = post(&w, json!({"action":"order","external_id":"meta_4","match":{"phone":"6025550303"},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    let r = rows(&a, &unsure).await;
    assert_eq!(r[0]["state"], "uncertain");
    assert!(!r[0]["attempted_at"].is_null());
    let (_, v) = post(&w, json!({"action":"order","external_id":"meta_4","match":{"phone":"6025550303"},"value_minor":200})).await;
    assert!(
        v["delivery"]["meta"]["reason"]
            .as_str()
            .unwrap()
            .contains("may have reached Meta"),
        "{v}"
    );
    assert_eq!(drain(&a).await, 0);
    // A rate limit is retried later.
    meta_lead(&w, "602-555-0304", json!({"fbclid":"IwAR_click_5"})).await;
    *m.mode.lock().unwrap() = "rate".into();
    let (_, limited) = post(&w, json!({"action":"order","external_id":"meta_5","match":{"phone":"6025550304"},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    assert_eq!(rows(&a, &limited).await[0]["state"], "retry");

    // No Meta click: blocked in meta_clicks mode, sent in all_leads mode after the owner
    // approves (conversions:send), with Limited Data Use by location.
    meta_lead(
        &w,
        "602-555-0305",
        json!({"source":"google","gclid":"g-only"}),
    )
    .await;
    let (_, plain) = post(&w, json!({"action":"order","external_id":"meta_6","match":{"phone":"6025550305"},"value_minor":100})).await;
    assert_eq!(plain["delivery"]["meta"]["status"], "blocked", "{plain}");
    assert!(
        plain["delivery"]["meta"]["reason"]
            .as_str()
            .unwrap()
            .contains("No Meta ad click")
    );
    let (s, _) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/meta-settings",
        &write,
        json!({"settings_version":0,"match":"all_leads","ldu":"geolocate"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/meta-settings",
        &send,
        json!({"settings_version":0,"match":"all_leads","ldu":"geolocate"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    a.store.plan_meta(None).await.unwrap();
    let r = rows(&a, &plain).await;
    assert_eq!(r.len(), 1, "queued once all leads are sent");
    let e = &r[0]["request"]["event"];
    assert!(e["user_data"].get("fbc").is_none());
    assert_eq!(e["data_processing_options"], json!(["LDU"]));
    assert_eq!(e["data_processing_options_country"], 0);

    // Disconnecting removes the token and the kept browser details.
    let (s, v) = api(
        a.clone(),
        "DELETE",
        "/api/v1/connections/meta",
        &send,
        json!({"revision":1}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM website_sessions s JOIN sites t ON t.id=s.site_id WHERE t.workspace_id=$1 AND (s.client_user_agent IS NOT NULL OR s.client_ip IS NOT NULL)")
        .bind(a.store.workspace).fetch_one(&a.store.pool).await.unwrap();
    assert_eq!(kept, 0);
    // Without a connection, new sessions keep no browser details.
    let id = meta_lead(&w, "602-555-0306", json!({"fbclid":"IwAR_click_7"})).await;
    let ua: Option<String> = sqlx::query_scalar("SELECT s.client_user_agent FROM website_submissions f JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id WHERE f.lead_id=$1")
        .bind(id).fetch_one(&a.store.pool).await.unwrap();
    assert_eq!(ua, None);
    task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn meta_new_leads_are_website_lead_events_after_the_hold() {
    let w = world("not_required_outside_eea", true).await;
    let (a, m, task) = with_meta(&w.a).await;
    let w = World { a: a.clone(), ..w };
    let send = key(&a, &["read", "conversions:send", "conversions:write"]).await;
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/connections/meta",
        &send,
        json!({"revision":0,"dataset_id":DATASET,"access_token":GOOD}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    action(
        &a,
        "new",
        "new_lead",
        "once_per_click",
        Value::Null,
        "789",
        0,
    )
    .await;
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-actions/new/meta-event",
        &send,
        json!({"version":version(&a,"new").await,"event_name":"Lead"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A Meta click lead and a lead with no ad click at all, both after the mapping.
    let clicked = meta_lead(&w, "602-555-0400", json!({"fbclid":"IwAR_new_1"})).await;
    let organic = meta_lead(&w, "602-555-0401", json!({"source":"direct"})).await;
    // Past the 6-hour review hold (the action and mapping are older still).
    for q in [
        "UPDATE conversion_actions SET created_at=now()-interval '9 hours',meta_mapped_at=now()-interval '8 hours' WHERE workspace_id=$1 AND key='new'",
        "UPDATE conversion_delivery_settings SET auto_send_at=now()-interval '9 hours' WHERE workspace_id=$1",
        "UPDATE website_sessions s SET created_at=now()-interval '7 hours 1 minute' FROM sites t WHERE t.id=s.site_id AND t.workspace_id=$1",
        "UPDATE leads SET created_at=now()-interval '7 hours' WHERE workspace_id=$1",
    ] {
        sqlx::query(q)
            .bind(a.store.workspace)
            .execute(&a.store.pool)
            .await
            .unwrap();
    }
    meta::plan(&a).await.unwrap();
    let events: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id,lead_id FROM pipeline_events WHERE workspace_id=$1 AND action_key='new' ORDER BY created_at")
        .bind(a.store.workspace).fetch_all(&a.store.pool).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.1).collect::<Vec<_>>(),
        vec![Some(clicked)],
        "meta_clicks: only the Meta click lead"
    );
    assert_eq!(drain(&a).await, 1);
    let body = m.events.lock().unwrap().last().unwrap().clone();
    let e = &body["data"][0];
    assert_eq!(e["event_name"], "Lead");
    assert_eq!(e["action_source"], "website");
    assert_eq!(e["event_source_url"], format!("{ORIGIN}/quote"));
    assert!(e.get("custom_data").is_none());
    // all_leads: the organic lead becomes an event too, matched by its hashed phone.
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/meta-settings",
        &send,
        json!({"settings_version":0,"match":"all_leads","ldu":"off"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    meta::plan(&a).await.unwrap();
    let leads: Vec<Option<Uuid>> = sqlx::query_scalar("SELECT lead_id FROM pipeline_events WHERE workspace_id=$1 AND action_key='new' ORDER BY created_at")
        .bind(a.store.workspace).fetch_all(&a.store.pool).await.unwrap();
    assert_eq!(leads, vec![Some(clicked), Some(organic)]);
    assert_eq!(drain(&a).await, 1);
    let body = m.events.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        body["data"][0]["user_data"]["ph"][0],
        convert_adapters::meta_api::sha256_hex("16025550401")
    );
    task.abort();
}
