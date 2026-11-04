//! Pipeline conversions over the real HTTP boundary, a mock Google service and a
//! disposable PostgreSQL database.
use super::*;
use chrono::{Duration, Utc};
use convert_store::{
    conversions::{Consent, Decision, Settings},
    sites::SiteInput,
};
use google_tests::{Provider, api, begin, callback, fixture};
use std::collections::BTreeMap;

const ORIGIN: &str = "https://pipeline.example";
struct World {
    a: Shared,
    p: Arc<Provider>,
    site: Uuid,
    key: String,
    task: tokio::task::JoinHandle<()>,
}
/// Google connected, destination saved with no reviewed actions, delivery enabled, and
/// `quote` (once per click, $20 default) and `order` (every) mapped to eligible actions.
async fn world(policy: &str, auto_send: bool) -> World {
    let (a, p, task) = fixture().await;
    // The worker claims and plans across workspaces; park pipeline work left by earlier
    // tests in this disposable database so it is never sent to this test's provider.
    for q in [
        "UPDATE conversion_deliveries d SET due_at=NULL FROM conversion_outbox q WHERE q.workspace_id=d.workspace_id AND q.id=d.outbox_id AND q.kind='pipeline'",
        "UPDATE pipeline_events SET plan_state='final' WHERE plan_state IN ('pending','blocked')",
        "UPDATE conversion_actions SET enabled=false WHERE trigger='new_lead'",
    ] {
        sqlx::query(q).execute(&a.store.pool).await.unwrap();
    }
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    assert!(
        callback(a.clone(), &state, &cookie, "good")
            .await
            .contains("connected")
    );
    let s = &a.store;
    let site = s
        .save_site(
            None,
            &SiteInput {
                name: "Pipeline fixture".into(),
                origin: ORIGIN.into(),
                allowed_fields: vec!["email".into(), "phone".into()],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    s.save_conversion_settings(
        &Settings {
            customer_id: "1234567890".into(),
            ..Default::default()
        },
        "test",
    )
    .await
    .unwrap();
    for (key, counting, value, google) in [
        ("quote", "once_per_click", json!(2000), "123"),
        ("order", "every", Value::Null, "456"),
    ] {
        action(&a, key, "events", counting, value, google, 0).await;
    }
    let (status, v) = api(
        a.clone(),
        "POST",
        "/api/connections/google/validate",
        &a.api_key,
        json!({"revision":1,"settings_version":1,"login":"1234567890"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(v["pipeline_actions_valid"], true);
    s.set_delivery_enabled(true, 0, "test").await.unwrap();
    let (status, v) = api(
        a.clone(),
        "PUT",
        "/api/conversions/pipeline/policy",
        &a.api_key,
        json!({"auto_send":auto_send,"consent_policy":policy,"attest":true,"version":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let key = s
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Quarry".into(),
                scopes: vec!["read".into(), "conversions:write".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    World {
        a,
        p,
        site,
        key,
        task,
    }
}
async fn action(
    a: &Shared,
    key: &str,
    trigger: &str,
    counting: &str,
    value: Value,
    google: &str,
    version: i32,
) -> Value {
    let (status, v) = api(
        a.clone(),
        "PUT",
        &format!("/api/conversions/actions/{key}"),
        &a.api_key,
        json!({"key":key,"name":key,"trigger":trigger,"default_value_minor":value,"counting":counting,"google_action_id":google,"enabled":true,"version":version}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}
/// A form lead from a visit, optionally with a Google click, `age_days` in the past.
async fn lead(w: &World, contact: (&str, &str), click: Option<&str>, age_days: i64) -> Uuid {
    let session = Uuid::new_v4();
    let mut evidence =
        json!({"source":"google","consent":"granted","landing_url":format!("{ORIGIN}/")});
    if let Some(c) = click {
        evidence["gclid"] = json!(c);
    }
    w.a.store
        .capture_session(w.site, ORIGIN, session, &evidence)
        .await
        .unwrap();
    let fields = BTreeMap::from([(contact.0.to_string(), contact.1.to_string())]);
    w.a.store
        .capture_form(w.site, ORIGIN, session, &session.to_string(), &fields)
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "SELECT lead_id FROM website_submissions WHERE site_id=$1 AND session_id=$2",
    )
    .bind(w.site)
    .bind(session)
    .fetch_one(&w.a.store.pool)
    .await
    .unwrap();
    if age_days > 0 {
        sqlx::query("UPDATE website_sessions SET created_at=created_at-make_interval(days=>$3) WHERE site_id=$1 AND id=$2")
            .bind(w.site).bind(session).bind(age_days as i32).execute(&w.a.store.pool).await.unwrap();
        sqlx::query("UPDATE leads SET created_at=created_at-make_interval(days=>$2) WHERE id=$1")
            .bind(id)
            .bind(age_days as i32)
            .execute(&w.a.store.pool)
            .await
            .unwrap();
    }
    id
}
async fn post(w: &World, body: Value) -> (StatusCode, Value) {
    api(
        w.a.clone(),
        "POST",
        "/api/v1/conversion-events",
        &w.key,
        body,
    )
    .await
}
async fn deliveries(a: &App) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM conversion_deliveries WHERE workspace_id=$1")
        .bind(a.store.workspace)
        .fetch_one(&a.store.pool)
        .await
        .unwrap()
}
async fn due(a: &App) {
    sqlx::query("UPDATE conversion_deliveries SET due_at=now()-interval '1 second' WHERE workspace_id=$1 AND due_at IS NOT NULL").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    sqlx::query("UPDATE pipeline_events SET plan_after=now() WHERE workspace_id=$1")
        .bind(a.store.workspace)
        .execute(&a.store.pool)
        .await
        .unwrap();
}
async fn consent(a: &App, lead: Uuid, at: chrono::DateTime<Utc>) {
    a.store
        .save_conversion_consent(
            lead,
            &Consent {
                ad_user_data: Decision::Granted,
                ad_personalization: Decision::Denied,
                effective_at: Some(at),
                reference: "PRIVATE CMP RECEIPT".into(),
                version: 0,
            },
            "test",
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn pipeline_reposts_restatements_and_once_per_click() {
    let w = world("not_required_outside_eea", true).await;
    let a = &w.a;
    let policy = a.store.pipeline_policy().await.unwrap();
    assert_eq!(policy["consent_policy"], "not_required_outside_eea");
    assert_eq!(policy["attested_by"], "workspace operator");
    let endpoint = Uuid::new_v4();
    sqlx::query("INSERT INTO webhook_endpoints(id,workspace_id,url,event_types,encrypted_secret) VALUES($1,$2,'https://hooks.example/visiconvert',ARRAY['conversion.delivered','conversion.failed'],'sealed')")
        .bind(endpoint).bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    let first = lead(&w, ("phone", "+1 (602) 555-0105"), Some("click-a"), 0).await;
    let order = json!({"action":"order","external_id":"order_8812","match":{"phone":"602.555.0105"},"value_minor":41250});
    let (status, v) = post(&w, order.clone()).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["lead_id"], json!(first));
    assert_eq!(v["matched_by"], "phone");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["delivery"]["google"], "queued", "{v}");
    // An identical repost is a no-op: same revision, no second delivery.
    let (_, again) = post(&w, order.clone()).await;
    assert_eq!(again["revision"], 1);
    assert_eq!(again["id"], v["id"]);
    assert_eq!(deliveries(a).await, 1);
    assert!(delivery::run_one(a).await.unwrap());
    let sent = w.p.deliveries.lock().unwrap()[0].clone();
    let event = &sent["events"][0];
    assert!(event.get("consent").is_none(), "{sent}");
    assert_eq!(event["conversionValue"], json!(412.5));
    assert_eq!(event["currency"], "USD");
    assert_eq!(event["adIdentifiers"]["gclid"], "click-a");
    assert_eq!(sent["destinations"][0]["productDestinationId"], "456");
    for private in ["order_8812", "602", "PRIVATE"] {
        assert!(!sent.to_string().contains(private), "{private}");
    }
    due(a).await;
    assert!(delivery::run_one(a).await.unwrap());
    let (_, v) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-events?external_id=order_8812",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(v["data"][0]["delivery"]["google"], "delivered", "{v}");
    // Pipeline deliveries emit the existing conversion webhooks, naming their event.
    let hook = sqlx::query("SELECT d.id,e.id AS event_id,e.type,e.resource_id,e.resource_version,e.occurred_at FROM webhook_deliveries d JOIN webhook_events e ON e.id=d.event_id WHERE d.workspace_id=$1")
        .bind(a.store.workspace).fetch_one(&a.store.pool).await.unwrap();
    use sqlx::Row;
    assert_eq!(hook.get::<String, _>("type"), "conversion.delivered");
    let claim = convert_store::webhooks::Claim {
        id: hook.get("id"),
        workspace: a.store.workspace,
        event_id: hook.get("event_id"),
        event_type: hook.get("type"),
        resource_kind: "conversion".into(),
        resource_id: hook.get("resource_id"),
        resource_version: hook.get("resource_version"),
        occurred_at: hook.get("occurred_at"),
        endpoint_id: endpoint,
        url: String::new(),
        include_contact: false,
        secret: String::new(),
        previous_secret: None,
        payload: None,
        attempts: 0,
    };
    let payload = a.store.webhook_payload(&claim).await.unwrap();
    assert_eq!(payload["data"]["state"], "processed", "{payload}");
    assert_eq!(
        payload["data"]["pipeline_event"]["external_id"],
        "order_8812"
    );
    assert_eq!(payload["data"]["pipeline_event"]["value_minor"], 41250);
    // A new value is the next revision, restated under the same transaction ID.
    let mut revised = order.clone();
    revised["value_minor"] = json!(50000);
    let (_, v) = post(&w, revised).await;
    assert_eq!(v["revision"], 2);
    assert_eq!(v["delivery"]["google"], "queued", "{v}");
    assert!(delivery::run_one(a).await.unwrap());
    let restated = w.p.deliveries.lock().unwrap()[1].clone();
    assert_eq!(restated["events"][0]["conversionValue"], json!(500.0));
    assert_eq!(
        restated["events"][0]["transactionId"],
        sent["events"][0]["transactionId"]
    );
    assert!(convert_adapters::google_api::same_delivery_identity(
        &sent, &restated
    ));
    // A revision waits while the previous one is still in flight.
    let mut third = order.clone();
    third["value_minor"] = json!(60000);
    let (_, v) = post(&w, third).await;
    assert_eq!(v["delivery"]["google"], "blocked");
    assert!(
        v["delivery"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Waiting for the previous delivery"),
        "{v}"
    );
    // once_per_click: a second quote for the same click is counted once.
    let (_, q1) = post(
        &w,
        json!({"action":"quote","external_id":"quote_1","lead_id":first}),
    )
    .await;
    assert_eq!(q1["value_minor"], 2000);
    assert_eq!(q1["delivery"]["google"], "queued", "{q1}");
    let (_, q2) = post(
        &w,
        json!({"action":"quote","external_id":"quote_2","lead_id":first}),
    )
    .await;
    assert_eq!(q2["delivery"]["google"], "blocked");
    assert_eq!(
        q2["delivery"]["reason"],
        "Already counted for this ad click (counted once per click)"
    );
    // Cancel stops unsent work; Google cannot retract, so nothing is uploaded.
    let (_, c) = post(
        &w,
        json!({"action":"quote","external_id":"quote_1","status":"canceled"}),
    )
    .await;
    assert_eq!(c["delivery"]["google"], "canceled", "{c}");
    assert_eq!(c["revision"], 2);
    let count = w.p.deliveries.lock().unwrap().len();
    due(a).await;
    while delivery::run_one(a).await.unwrap() {}
    assert!(
        !w.p.deliveries.lock().unwrap()[count..]
            .iter()
            .any(|d| d["destinations"][0]["productDestinationId"] == "123"),
        "canceled quote must not be sent"
    );
    let other = Store {
        pool: a.store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    assert_eq!(
        other.pipeline_events(&Default::default()).await.unwrap()["data"],
        json!([])
    );
    w.task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn pipeline_matching_windows_and_consent() {
    let w = world("per_lead", true).await;
    let a = &w.a;
    let acquired = lead(&w, ("phone", "6025550105"), Some("click-old"), 5).await;
    let latest = lead(&w, ("phone", "+16025550105"), Some("click-new"), 0).await;
    let unclicked = lead(&w, ("email", "Pat@Example.com"), None, 0).await;
    let (_, f) = post(
        &w,
        json!({"action":"order","external_id":"o1","match":{"phone":"+1 602 555 0105"}}),
    )
    .await;
    assert_eq!(f["lead_id"], json!(acquired), "{f}");
    let (_, l) = post(&w, json!({"action":"order","external_id":"o2","match":{"phone":"6025550105"},"attribution":"latest_click"})).await;
    assert_eq!(l["lead_id"], json!(latest), "{l}");
    assert_eq!(l["attribution"], "latest_click");
    assert_eq!(f["delivery"]["google"], "blocked");
    assert_eq!(
        f["delivery"]["reason"],
        "No advertising consent recorded for this lead"
    );
    assert_eq!(deliveries(a).await, 0);
    // Recording consent effective before the event lets the worker queue it.
    consent(a, acquired, Utc::now() - Duration::days(6)).await;
    due(a).await;
    delivery::plan(a).await.unwrap();
    assert_eq!(deliveries(a).await, 1);
    assert!(delivery::run_one(a).await.unwrap());
    let sent = w.p.deliveries.lock().unwrap()[0].clone();
    assert_eq!(
        sent["events"][0]["consent"]["adUserData"],
        "CONSENT_GRANTED"
    );
    assert_eq!(
        sent["events"][0]["consent"]["adPersonalization"],
        "CONSENT_DENIED"
    );
    assert!(sent["events"][0].get("conversionValue").is_none());
    // Matching only considers leads with a click; an exact lead without one is blocked.
    let (_, e) = post(
        &w,
        json!({"action":"order","external_id":"o3","match":{"email":"  pat@example.COM "}}),
    )
    .await;
    assert_eq!(e["matched_by"], "none");
    assert_eq!(e["lead_id"], Value::Null);
    assert_eq!(
        e["delivery"]["reason"],
        "No attributed lead matched this event"
    );
    let (_, n) = post(
        &w,
        json!({"action":"order","external_id":"o4","lead_id":unclicked}),
    )
    .await;
    assert_eq!(n["matched_by"], "lead_id");
    assert_eq!(n["delivery"]["reason"], "No Google click");
    // The mock action's click window is 30 days.
    let old = lead(&w, ("phone", "4805550199"), Some("click-stale"), 40).await;
    consent(a, old, Utc::now() - Duration::days(41)).await;
    let (_, o) = post(
        &w,
        json!({"action":"order","external_id":"o5","lead_id":old}),
    )
    .await;
    assert_eq!(o["delivery"]["reason"], "Outside the click window", "{o}");
    let (_, early) = post(&w, json!({"action":"order","external_id":"o6","lead_id":latest,"occurred_at":(Utc::now()-Duration::days(1)).to_rfc3339()})).await;
    assert_eq!(early["delivery"]["reason"], "Event predates the ad visit");
    for (body, status) in [
        (
            json!({"action":"missing","external_id":"x","lead_id":latest}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"action":"order","external_id":"never","status":"canceled"}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"action":"order","external_id":"x","lead_id":Uuid::new_v4()}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"action":"order","external_id":"x","lead_id":latest,"match":{"phone":"6025550105"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"action":"order","external_id":"x","match":{"phone":"555-0105"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"action":"order","external_id":"x","lead_id":latest,"occurred_at":(Utc::now()+Duration::days(1)).to_rfc3339()}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(post(&w, body.clone()).await.0, status, "{body}");
    }
    assert_eq!(w.p.deliveries.lock().unwrap().len(), 1);
    w.task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn pipeline_auto_send_isolation_new_leads_refresh_and_scopes() {
    let w = world("not_required_outside_eea", false).await;
    let a = &w.a;
    let first = lead(&w, ("phone", "6025550105"), Some("click-a"), 0).await;
    let (_, off) = post(
        &w,
        json!({"action":"order","external_id":"before","lead_id":first,"value_minor":100}),
    )
    .await;
    assert_eq!(off["delivery"]["google"], "blocked");
    assert!(
        off["delivery"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Recorded while automatic sending was off"),
        "{off}"
    );
    assert_eq!(deliveries(a).await, 0);
    // The attestation is required, and the policy uses compare-and-swap versions.
    let path = "/api/conversions/pipeline/policy";
    let (status, _) = api(
        a.clone(),
        "PUT",
        path,
        &a.api_key,
        json!({"auto_send":true,"consent_policy":"not_required_outside_eea","version":2}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = api(a.clone(), "PUT", path, &a.api_key, json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest":true,"version":1})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = api(a.clone(), "PUT", path, &a.api_key, json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest":true,"version":2})).await;
    assert_eq!(status, StatusCode::OK);
    // An event recorded while off is never sent later, even after a revision.
    let (_, still) = post(
        &w,
        json!({"action":"order","external_id":"before","lead_id":first,"value_minor":200}),
    )
    .await;
    assert_eq!(still["delivery"]["google"], "blocked");
    let (_, on) = post(
        &w,
        json!({"action":"order","external_id":"after","lead_id":first,"value_minor":300}),
    )
    .await;
    assert_eq!(on["delivery"]["google"], "queued", "{on}");
    // A booking edit invalidates reviewed preparations but not pipeline sends.
    a.store
        .put_outcome(
            first,
            &OutcomeInput {
                external_id: "booking-1".into(),
                amount_minor: 12345,
                currency: "USD".into(),
                status: "booked".into(),
                revision: 1,
            },
        )
        .await
        .unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM conversion_outbox WHERE workspace_id=$1 AND kind='pipeline'",
    )
    .bind(a.store.workspace)
    .fetch_one(&a.store.pool)
    .await
    .unwrap();
    assert_eq!(state, "awaiting_connection");
    assert!(delivery::run_one(a).await.unwrap());
    assert_eq!(w.p.deliveries.lock().unwrap().len(), 1);
    // new_lead: VisiConvert records the event for attributed leads after the action exists.
    action(
        a,
        "lead",
        "new_lead",
        "once_per_click",
        Value::Null,
        "123",
        0,
    )
    .await;
    let (status, _) = post(
        &w,
        json!({"action":"lead","external_id":"x","lead_id":first}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let fresh = lead(&w, ("email", "new@example.com"), Some("click-new"), 0).await;
    let wrong = lead(&w, ("email", "wrong@example.com"), Some("click-wrong"), 0).await;
    let _unattributed = lead(&w, ("email", "none@example.com"), None, 0).await;
    delivery::plan(a).await.unwrap();
    let lead_events = || async {
        let (_, v) = api(
            a.clone(),
            "GET",
            "/api/v1/conversion-events?action=lead",
            &w.key,
            Value::Null,
        )
        .await;
        let mut events = v["data"].as_array().unwrap().clone();
        events.sort_by_key(|e| e["external_id"] != fresh.to_string());
        events
    };
    let events = lead_events().await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0]["external_id"], fresh.to_string());
    assert_eq!(events[0]["matched_by"], "new_lead");
    // New leads are held for 6 hours so a person can mark wrong numbers first.
    assert_eq!(events[0]["delivery"]["google"], "blocked", "{events:?}");
    assert_eq!(
        events[0]["delivery"]["reason"],
        "Held for 6 hours so the lead can be reviewed before sending"
    );
    sqlx::query("UPDATE inquiries SET qualification='unqualified' WHERE id=(SELECT inquiry_id FROM leads WHERE id=$1)")
        .bind(wrong).execute(&a.store.pool).await.unwrap();
    sqlx::query(
        "UPDATE website_sessions SET created_at=created_at-interval '7 hours' WHERE site_id=$1",
    )
    .bind(w.site)
    .execute(&a.store.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE pipeline_events SET occurred_at=occurred_at-interval '6 hours 1 minute',plan_after=now() WHERE workspace_id=$1 AND action_key='lead'")
        .bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    delivery::plan(a).await.unwrap();
    let events = lead_events().await;
    assert_eq!(events[0]["delivery"]["google"], "queued", "{events:?}");
    assert_eq!(events[1]["delivery"]["google"], "blocked", "{events:?}");
    assert_eq!(
        events[1]["delivery"]["reason"],
        "The inquiry is marked unqualified"
    );
    delivery::plan(a).await.unwrap();
    assert_eq!(
        api(
            a.clone(),
            "GET",
            "/api/v1/conversion-events?action=lead",
            &w.key,
            Value::Null
        )
        .await
        .1["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // An expired destination check is refreshed by the worker before planning.
    sqlx::query(
        "UPDATE google_connections SET checked_at=now()-interval '25 hours' WHERE workspace_id=$1",
    )
    .bind(a.store.workspace)
    .execute(&a.store.pool)
    .await
    .unwrap();
    let (_, stale) = post(
        &w,
        json!({"action":"order","external_id":"stale","lead_id":fresh,"value_minor":1}),
    )
    .await;
    assert_eq!(
        stale["delivery"]["reason"],
        "Waiting for the Google destination check"
    );
    due(a).await;
    delivery::plan(a).await.unwrap();
    let checked: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT checked_at FROM google_connections WHERE workspace_id=$1")
            .bind(a.store.workspace)
            .fetch_one(&a.store.pool)
            .await
            .unwrap();
    assert!(checked > Utc::now() - Duration::minutes(1));
    let row = a
        .store
        .pipeline_event(stale["id"].as_str().unwrap().parse().unwrap())
        .await
        .unwrap();
    assert_eq!(row["delivery"]["google"], "queued", "{row}");
    // Pausing delivery holds automatic sends queued instead of sending or dropping them.
    a.store
        .set_delivery_enabled(false, 3, "test")
        .await
        .unwrap();
    due(a).await;
    let before = w.p.deliveries.lock().unwrap().len();
    // Saved receipts are still polled; nothing new is uploaded.
    while delivery::run_one(a).await.unwrap() {}
    assert_eq!(w.p.deliveries.lock().unwrap().len(), before);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversion_deliveries WHERE workspace_id=$1 AND state='queued'",
    )
    .bind(a.store.workspace)
    .fetch_one(&a.store.pool)
    .await
    .unwrap();
    assert_eq!(queued, 2);
    a.store.set_delivery_enabled(true, 4, "test").await.unwrap();
    while delivery::run_one(a).await.unwrap() {}
    assert_eq!(w.p.deliveries.lock().unwrap().len(), before + 2);
    // A failed destination check pauses an automatic send: still queued, never uploaded.
    let (_, held) = post(
        &w,
        json!({"action":"order","external_id":"held","lead_id":fresh,"value_minor":5}),
    )
    .await;
    assert_eq!(held["delivery"]["google"], "queued");
    sqlx::query("UPDATE google_connections SET validation=jsonb_set(validation,'{account,status}','\"SUSPENDED\"') WHERE workspace_id=$1").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    due(a).await;
    let uploads = w.p.deliveries.lock().unwrap().len();
    while delivery::run_one(a).await.unwrap() {}
    assert_eq!(w.p.deliveries.lock().unwrap().len(), uploads);
    let row = a
        .store
        .pipeline_event(held["id"].as_str().unwrap().parse().unwrap())
        .await
        .unwrap();
    assert_eq!(row["delivery"]["google"], "queued", "{row}");
    let (diagnostic, attempts): (String, i32) = sqlx::query_as(
        "SELECT diagnostic,attempts FROM conversion_deliveries WHERE workspace_id=$1 AND id=$2",
    )
    .bind(a.store.workspace)
    .bind(
        row["delivery"]["delivery_id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap(),
    )
    .fetch_one(&a.store.pool)
    .await
    .unwrap();
    assert!(diagnostic.starts_with("Paused:"), "{diagnostic}");
    assert_eq!(attempts, 0);
    // Scopes: read can list but not post; private routes refuse public keys.
    let read = a
        .store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Reader".into(),
                scopes: vec!["read".into(), "bookings:write".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-events",
        &read,
        json!({"action":"order","external_id":"nope","lead_id":first}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("conversions:write"));
    assert_eq!(
        api(
            a.clone(),
            "GET",
            "/api/v1/conversion-events",
            &read,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    for key in [&read, &w.key] {
        assert_eq!(
            api(
                a.clone(),
                "GET",
                "/api/conversions/pipeline",
                key,
                Value::Null
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (_, overview) = api(
        a.clone(),
        "GET",
        "/api/conversions/pipeline",
        &a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(overview["actions"].as_array().unwrap().len(), 3);
    assert_eq!(overview["policy"]["auto_send"], true);
    // Key, trigger and counting are fixed after creation; stale versions conflict.
    let (status, _) = api(a.clone(), "PUT", "/api/conversions/actions/order", &a.api_key, json!({"key":"order","name":"Order","trigger":"events","default_value_minor":null,"counting":"once_per_click","google_action_id":"456","enabled":true,"version":1})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = api(a.clone(), "PUT", "/api/conversions/actions/order", &a.api_key, json!({"key":"order","name":"Order","trigger":"events","default_value_minor":null,"counting":"every","google_action_id":"456","enabled":false,"version":0})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    w.task.abort();
}

/// Keys and MCP with conversions:write can define, map and create actions and post events,
/// but the owner switches (automatic sending, consent policy, delivery) need
/// conversions:send, and Google OAuth stays unreachable to every credential.
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn pipeline_keys_and_mcp_cannot_flip_owner_switches() {
    let w = world("per_lead", false).await;
    let a = &w.a;
    let full = a
        .store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Everything".into(),
                scopes: [
                    "read",
                    "leads:write",
                    "bookings:write",
                    "numbers:write",
                    "setup:write",
                    "webhooks:write",
                    "conversions:write",
                ]
                .map(String::from)
                .to_vec(),
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    for key in [&w.key, &full] {
        for (method, path, body) in [
            (
                "PUT",
                "/api/v1/conversions/pipeline/policy",
                json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest":true,"version":2}),
            ),
            (
                "PUT",
                "/api/v1/conversions/delivery/settings",
                json!({"enabled":true,"version":2}),
            ),
            (
                "POST",
                "/api/v1/connections/google/start",
                json!({"revision":1}),
            ),
            (
                "PUT",
                "/api/v1/conversions/settings",
                json!({"customer_id":"1234567890","qualified_action_id":"","booking_action_id":"","allow_inferred_calls":true,"version":1}),
            ),
        ] {
            let (status, _) = api(a.clone(), method, path, key, body.clone()).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            let private = path.replace("/api/v1/", "/api/");
            let (status, _) = api(a.clone(), method, &private, key, body).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{private}");
        }
        // The credential route needs conversions:send.
        let (status, v) = api(
            a.clone(),
            "PUT",
            "/api/v1/conversion-delivery",
            key,
            json!({"auto_send":true,"version":2}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(v["error"].as_str().unwrap().contains("conversions:send"));
        // Action and event bodies reject owner-only fields outright.
        let (status, _) = api(a.clone(), "PUT", "/api/v1/conversion-actions/lead", key, json!({"key":"lead","name":"Lead","trigger":"events","default_value_minor":null,"counting":"once_per_click","google_action_id":null,"enabled":true,"version":0,"auto_send":true})).await;
        assert!(
            status.is_client_error() && status != StatusCode::NOT_FOUND,
            "{status}"
        );
    }
    let policy = a.store.pipeline_policy().await.unwrap();
    assert_eq!(policy["auto_send"], false);
    assert_eq!(policy["consent_policy"], "per_lead");
    // The checklist names the owner-only switch that is left.
    let (status, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    assert_eq!(setup["ready"], false);
    assert_eq!(setup["google_connected"], true);
    assert_eq!(
        setup["next_steps"],
        json!([{"actor":"owner_approval","step":"Turn on automatic sending: the owner in Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner confirms that no other uploader sends the same Google actions."}])
    );
    assert!(
        setup["summary"]
            .as_str()
            .unwrap()
            .starts_with("Only owner steps are left")
    );
    // Discovery and mapping through a key.
    let (status, v) = api(
        a.clone(),
        "GET",
        "/api/v1/connections/google/accounts?login=1234567890",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["logins"], json!(["1234567890"]));
    let (status, v) = api(
        a.clone(),
        "GET",
        "/api/v1/connections/google/actions?login=1234567890&customer=1234567890",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "123" && x["eligible"] == true)
    );
    let (status, v) = api(a.clone(), "PUT", "/api/v1/conversion-actions/lead", &w.key, json!({"key":"lead","name":"Lead","trigger":"events","default_value_minor":null,"counting":"once_per_click","google_action_id":null,"enabled":true,"version":0})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (_, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(setup["next_steps"][0]["actor"], "agent", "{setup}");
    // Creating in Google needs conversions:write, recovers after a lost response, never replaces.
    let read = a
        .store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Read".into(),
                scopes: vec!["read".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let create = |key: String, version: i32| {
        let a = a.clone();
        async move {
            api(
                a,
                "POST",
                "/api/v1/conversion-actions/lead/google-action",
                &key,
                json!({"version":version,"login":"1234567890"}),
            )
            .await
        }
    };
    assert_eq!(create(read.clone(), 1).await.0, StatusCode::FORBIDDEN);
    w.p.setup_lost_response
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, _) = create(w.key.clone(), 1).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let (status, v) = create(w.key.clone(), 1).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let id = v["google_action_id"].as_str().unwrap().to_string();
    assert_eq!(
        w.p.setup_actions.lock().unwrap().len(),
        1,
        "a retry reuses the created action"
    );
    let created = w.p.setup_actions.lock().unwrap()[0].clone();
    assert_eq!(created["category"], "QUALIFIED_LEAD");
    assert_eq!(created["countingType"], "ONE_PER_CLICK");
    assert!(
        created["name"]
            .as_str()
            .unwrap()
            .starts_with("VisiConvert - lead (")
    );
    assert_eq!(
        a.store
            .created_google_action("1234567890", "pipeline:lead")
            .await
            .unwrap(),
        Some(id.clone())
    );
    assert_eq!(
        create(w.key.clone(), 2).await.0,
        StatusCode::CONFLICT,
        "never replaces a mapping"
    );
    // The MCP tool surface over the same key: setup, events and owner-only refusals.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = router(a.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mcp = convert_cli::mcp::Server::scoped(
        Ok(convert_cli::client::Client::new(&origin, &w.key).unwrap()),
        vec!["read".into(), "conversions:write".into()],
    );
    let call = |name: &str, args: Value| {
        let message = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        let mcp = &mcp;
        async move { mcp.handle(message).await.unwrap()["result"].clone() }
    };
    let r = call("get_conversion_setup", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Turn on automatic sending: the owner in Conversions")
    );
    let r = call("get_status", json!({})).await;
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Google conversions: Turn on automatic sending: the owner in Conversions"),
        "{r}"
    );
    let first = lead(&w, ("email", "mcp@example.com"), Some("click-mcp"), 0).await;
    let r = call(
        "record_conversion_event",
        json!({"action":"order","external_id":"mcp-1","lead_id":first,"value_minor":500}),
    )
    .await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Recorded while automatic sending was off")
    );
    let r = call("list_conversion_events", json!({"external_id":"mcp-1"})).await;
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"revision\": 1"),
        "{r}"
    );
    let r = call("update_conversion_action", json!({"key":"lead","name":"Lead","trigger":"events","counting":"once_per_click","enabled":true,"google_action_id":"123","version":2})).await;
    assert_eq!(r["isError"], false, "{r}");
    for tool in [
        "set_pipeline_policy",
        "turn_on_automatic_sending",
        "connect_google",
    ] {
        let r = call(tool, json!({"auto_send":true})).await;
        assert_eq!(r["isError"], true);
    }
    assert_eq!(a.store.pipeline_policy().await.unwrap()["auto_send"], false);
    server.abort();
    w.task.abort();
}

/// A conversions:send key flips the same switches as the owner routes, under the same
/// rules: attestation required, compare-and-swap versions, events recorded while sending
/// was off never sent, and an audit row naming the key.
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn conversions_send_key_follows_owner_rules() {
    let w = world("per_lead", false).await;
    let a = &w.a;
    let issue = |scopes: &[&str]| {
        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        async move {
            a.store
                .issue_api_key(
                    &convert_store::api_keys::KeyInput {
                        name: "Sender".into(),
                        scopes,
                        expires_at: None,
                    },
                    None,
                    None,
                )
                .await
                .unwrap()
        }
    };
    let created = issue(&["read", "conversions:send"]).await;
    let send = created["secret"].as_str().unwrap().to_string();
    let send_id = created["id"].as_str().unwrap().to_string();
    let path = "/api/v1/conversion-delivery";
    let put = |key: String, body: Value| {
        let a = a.clone();
        async move { api(a, "PUT", path, &key, body).await }
    };
    // conversions:write and read can read the switches but not change them.
    let (status, current) = api(a.clone(), "GET", path, &w.key, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(current["version"], 2);
    assert_eq!(current["enabled"], true);
    assert!(current["warning"].is_null());
    let (status, _) = put(w.key.clone(), json!({"auto_send":true,"version":2})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Attestation is required with not_required_outside_eea, and only there.
    for body in [
        json!({"consent_policy":"not_required_outside_eea","version":2}),
        json!({"consent_policy":"not_required_outside_eea","attest_no_eea_advertising":false,"version":2}),
        json!({"auto_send":true,"attest_no_eea_advertising":true,"version":2}),
        json!({"consent_policy":"per_lead","attest_no_eea_advertising":true,"version":2}),
        json!({"version":2}),
        json!({"auto_send":true,"attest":true,"version":2}),
    ] {
        let (status, v) = put(send.clone(), body.clone()).await;
        assert!(
            status.is_client_error() && status != StatusCode::FORBIDDEN,
            "{body} {v}"
        );
    }
    assert_eq!(a.store.pipeline_policy().await.unwrap()["version"], 2);
    // An event recorded while sending is off stays blocked after sending turns on.
    let first = lead(&w, ("phone", "6025550177"), Some("click-send"), 0).await;
    let (_, off) = post(
        &w,
        json!({"action":"order","external_id":"before-send","lead_id":first,"value_minor":100}),
    )
    .await;
    assert_eq!(off["delivery"]["google"], "blocked");
    let (status, _) = put(send.clone(), json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest_no_eea_advertising":true,"version":1})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, on) = put(send.clone(), json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest_no_eea_advertising":true,"version":2})).await;
    assert_eq!(status, StatusCode::OK, "{on}");
    assert_eq!(on["auto_send"], true);
    assert!(on["auto_send_at"].is_string());
    assert_eq!(on["attested_by"], "workspace operator");
    assert!(on["attested_at"].is_string());
    assert!(
        on["warning"]
            .as_str()
            .unwrap()
            .contains("cannot be retracted")
    );
    assert_eq!(on["setup"]["ready"], true, "{on}");
    let (actor, payload): (String, Value) = sqlx::query_as("SELECT actor,payload FROM conversion_events WHERE workspace_id=$1 AND kind='pipeline_policy_saved' ORDER BY id DESC LIMIT 1")
        .bind(a.store.workspace).fetch_one(&a.store.pool).await.unwrap();
    assert_eq!(actor, format!("API key {send_id}"));
    assert_eq!(payload["attested_by"], "workspace operator");
    assert_eq!(payload["version"], 3);
    let (_, still) = post(
        &w,
        json!({"action":"order","external_id":"before-send","lead_id":first,"value_minor":200}),
    )
    .await;
    assert_eq!(still["delivery"]["google"], "blocked", "{still}");
    let (_, sent) = post(
        &w,
        json!({"action":"order","external_id":"after-send","lead_id":first,"value_minor":300}),
    )
    .await;
    assert_eq!(sent["delivery"]["google"], "queued", "{sent}");
    // An unchanged request writes nothing; the attestation keeps its original record.
    let (status, same) = put(
        send.clone(),
        json!({"auto_send":true,"consent_policy":"not_required_outside_eea","attest_no_eea_advertising":true,"version":3}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(same["version"], 3);
    assert_eq!(same["attested_at"], on["attested_at"]);
    // Pausing delivery through the key uses the owner route's pause rules and audit.
    let (status, paused) = put(send.clone(), json!({"enabled":false,"version":3})).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["auto_send"], true, "omitted fields are unchanged");
    assert_eq!(paused["consent_policy"], "not_required_outside_eea");
    let actor: String = sqlx::query_scalar("SELECT actor FROM conversion_events WHERE workspace_id=$1 AND kind='delivery_enabled' ORDER BY id DESC LIMIT 1")
        .bind(a.store.workspace).fetch_one(&a.store.pool).await.unwrap();
    assert_eq!(actor, format!("API key {send_id}"));
    // Turning sending off clears auto_send_at; turning it on later starts a new window.
    let (status, v) = put(
        send.clone(),
        json!({"enabled":true,"auto_send":false,"version":4}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["auto_send_at"].is_null());
    assert!(v["warning"].is_null());
    let (_, between) = post(
        &w,
        json!({"action":"order","external_id":"while-off","lead_id":first,"value_minor":400}),
    )
    .await;
    assert_eq!(between["delivery"]["google"], "blocked");
    // MCP over the same key: the tool needs conversions:send in the connection's scopes.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = router(a.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let call = |scopes: &[&str], name: &str, args: Value| {
        let mcp = convert_cli::mcp::Server::scoped(
            Ok(convert_cli::client::Client::new(&origin, &send).unwrap()),
            scopes.iter().map(|s| s.to_string()).collect(),
        );
        let message = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        async move { mcp.handle(message).await.unwrap()["result"].clone() }
    };
    let r = call(
        &["read", "conversions:write"],
        "set_conversion_delivery",
        json!({"auto_send":true,"version":6}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&["read"], "get_conversion_delivery", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"version\": 6"),
        "{r}"
    );
    let r = call(
        &["read", "conversions:send"],
        "set_conversion_delivery",
        json!({"auto_send":true,"version":6}),
    )
    .await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("count these conversions twice")
    );
    assert_eq!(a.store.pipeline_policy().await.unwrap()["auto_send"], true);
    server.abort();
    w.task.abort();
}

/// conversions:write runs the owner's "Check saved connection" through the same code:
/// identical report, the same recorded check, and the checklist step goes to the agent.
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn conversions_write_runs_the_owner_destination_check() {
    let w = world("per_lead", false).await;
    let a = &w.a;
    sqlx::query("UPDATE google_connections SET validation=NULL,validated_settings_version=NULL,checked_at=NULL WHERE workspace_id=$1")
        .bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    let (_, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &w.key,
        Value::Null,
    )
    .await;
    let step = setup["next_steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| {
            s["step"]
                .as_str()
                .unwrap()
                .contains("check_google_destination")
        })
        .cloned()
        .unwrap_or_else(|| panic!("{setup}"));
    assert_eq!(step["actor"], "agent");
    let body = json!({"revision":setup["connection_revision"],"settings_version":setup["settings_version"],"login":"1234567890"});
    let read = a
        .store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Read".into(),
                scopes: vec!["read".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let path = "/api/v1/conversion-setup/check";
    let (status, v) = api(a.clone(), "POST", path, &read, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("conversions:write"));
    let (status, by_key) = api(a.clone(), "POST", path, &w.key, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{by_key}");
    assert_eq!(by_key["valid"], true, "{by_key}");
    assert_eq!(by_key["pipeline_actions_valid"], true);
    let actor: String = sqlx::query_scalar("SELECT actor FROM conversion_events WHERE workspace_id=$1 AND kind='google_destination_checked' ORDER BY id DESC LIMIT 1")
        .bind(a.store.workspace).fetch_one(&a.store.pool).await.unwrap();
    assert!(actor.starts_with("API key "), "{actor}");
    let (_, after) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &w.key,
        Value::Null,
    )
    .await;
    assert!(after["checked_at"].is_string());
    assert!(
        !after["next_steps"]
            .to_string()
            .contains("check_google_destination"),
        "{after}"
    );
    // The owner route returns the same report for the same inputs.
    let (status, by_owner) = api(
        a.clone(),
        "POST",
        "/api/connections/google/validate",
        &a.api_key,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_owner}");
    assert_eq!(by_key, by_owner);
    // Stale inputs conflict exactly as on the owner route.
    let mut stale = body.clone();
    stale["settings_version"] = json!(99);
    assert_eq!(
        api(a.clone(), "POST", path, &w.key, stale.clone()).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api(
            a.clone(),
            "POST",
            "/api/connections/google/validate",
            &a.api_key,
            stale
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    // MCP tool over the same key.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = router(a.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (scopes, error) in [
        (vec!["read"], true),
        (vec!["read", "conversions:write"], false),
    ] {
        let mcp = convert_cli::mcp::Server::scoped(
            Ok(convert_cli::client::Client::new(&origin, &w.key).unwrap()),
            scopes.into_iter().map(String::from).collect(),
        );
        let r = mcp.handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"check_google_destination","arguments":body}})).await.unwrap()["result"].clone();
        assert_eq!(r["isError"], error, "{r}");
    }
    server.abort();
    w.task.abort();
}
