//! Real HTTP boundary with a mock Google service and disposable PostgreSQL.
use super::*;
use chrono::{Duration, Utc};
use convert_store::{
    conversions::{Consent, Decision, Settings, StageInput},
    sites::SiteInput,
};
use google_tests::{api, begin, callback, fixture};
use std::sync::atomic::Ordering;

async fn prepare(a: Shared) -> (Uuid, Value) {
    let s = &a.store;
    let site = s
        .save_site(
            None,
            &SiteInput {
                name: "Delivery fixture".into(),
                origin: "https://delivery.example".into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    let session = Uuid::new_v4();
    s.capture_session(site,"https://delivery.example",session,&json!({"source":"google","gclid":"test-click","consent":"granted","landing_url":"https://delivery.example/"})).await.unwrap();
    s.capture_form(
        site,
        "https://delivery.example",
        session,
        "confirmed-quote",
        &Default::default(),
    )
    .await
    .unwrap();
    let lead = s.leads().await.unwrap()[0].id;
    s.update_inquiry(
        lead,
        &LeadUpdate {
            qualification: Qualification::Qualified,
            notes: "PRIVATE NOTES".into(),
            version: 1,
        },
        "test",
    )
    .await
    .unwrap();
    s.put_outcome(
        lead,
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
    s.save_conversion_settings(
        &Settings {
            customer_id: "1234567890".into(),
            qualified_action_id: "123".into(),
            booking_action_id: "456".into(),
            ..Default::default()
        },
        "test",
    )
    .await
    .unwrap();
    s.save_conversion_consent(
        lead,
        &Consent {
            ad_user_data: Decision::Granted,
            ad_personalization: Decision::Denied,
            effective_at: Some(Utc::now() - Duration::hours(1)),
            reference: "PRIVATE CMP RECEIPT".into(),
            version: 0,
        },
        "test",
    )
    .await
    .unwrap();
    let (status, v) = api(
        a.clone(),
        "POST",
        "/api/connections/google/validate",
        &a.api_key,
        json!({"revision":1,"settings_version":1,"login":"1234567890"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["valid"], true);
    (lead, stage(&a, "booking").await)
}
async fn stage(a: &App, kind: &str) -> Value {
    let preview = a.store.conversion_preview().await.unwrap();
    let c = preview["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == kind)
        .unwrap();
    assert_eq!(c["eligible"], true, "{c}");
    a.store
        .stage_conversion(
            &StageInput {
                kind: kind.into(),
                entity_key: c["entity_key"].as_str().unwrap().into(),
                fingerprint: c["fingerprint"].as_str().unwrap().into(),
                version: c["job_version"].as_i64().unwrap() as i32,
            },
            "test",
        )
        .await
        .unwrap()
}
async fn validate(a: Shared, j: &Value) -> Value {
    let (status, v) = api(
        a.clone(),
        "POST",
        &format!("/api/conversions/{}/validate", j["id"].as_str().unwrap()),
        &a.api_key,
        json!({"version":j["version"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}
async fn due(a: &App) {
    sqlx::query("UPDATE conversion_deliveries SET due_at=now()-interval '1 second' WHERE workspace_id=$1 AND due_at IS NOT NULL").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
}
async fn row(a: &App, id: &Value) -> Value {
    a.store
        .delivery_list()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v["id"] == *id)
        .unwrap()
}
async fn queue(a: &App, d: &Value) {
    a.store
        .queue_delivery(d["id"].as_str().unwrap().parse().unwrap(), "test")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn delivery_opt_in_receipts_restatement_and_withdrawal() {
    let (a, p, task) = fixture().await;
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    assert!(
        callback(a.clone(), &state, &cookie, "good")
            .await
            .contains("connected")
    );
    let (lead, j) = prepare(a.clone()).await;
    let d = validate(a.clone(), &j).await;
    assert!(
        p.deliveries.lock().unwrap().is_empty(),
        "validateOnly must not record events"
    );
    assert!(
        a.store
            .queue_delivery(d["id"].as_str().unwrap().parse().unwrap(), "test")
            .await
            .is_err(),
        "default is paused"
    );
    a.store.set_delivery_enabled(true, 0, "test").await.unwrap();
    queue(&a, &d).await;
    assert!(
        a.store
            .queue_delivery(d["id"].as_str().unwrap().parse().unwrap(), "test")
            .await
            .is_err()
    );
    assert!(delivery::run_one(&a).await.unwrap());
    assert_eq!(row(&a, &d["id"]).await["state"], "submitted");
    let sent = p.deliveries.lock().unwrap()[0].clone();
    assert_eq!(sent["events"][0]["conversionValue"], json!(123.45));
    assert_eq!(
        sent["events"][0]["consent"]["adPersonalization"],
        "CONSENT_DENIED"
    );
    assert!(!sent.to_string().contains("PRIVATE"));
    assert!(!sent.to_string().contains("lead_id"));
    p.processing.store(true, Ordering::SeqCst);
    due(&a).await;
    assert!(delivery::run_one(&a).await.unwrap());
    assert_eq!(row(&a, &d["id"]).await["state"], "submitted");
    p.processing.store(false, Ordering::SeqCst);
    due(&a).await;
    assert!(delivery::run_one(&a).await.unwrap());
    assert_eq!(row(&a, &d["id"]).await["state"], "processed");
    assert!(!delivery::run_one(&a).await.unwrap());
    assert_eq!(p.deliveries.lock().unwrap().len(), 1);
    // Revision is a value restatement with the original transaction/time/action.
    a.store
        .put_outcome(
            lead,
            &OutcomeInput {
                external_id: "booking-1".into(),
                amount_minor: 19999,
                currency: "USD".into(),
                status: "booked".into(),
                revision: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(row(&a, &d["id"]).await["review_required"], true);
    let j2 = stage(&a, "booking").await;
    let d2 = validate(a.clone(), &j2).await;
    queue(&a, &d2).await;
    delivery::run_one(&a).await.unwrap();
    let sent2 = p.deliveries.lock().unwrap()[1].clone();
    assert_eq!(sent2["events"][0]["conversionValue"], json!(199.99));
    assert!(convert_adapters::google_api::same_delivery_identity(
        &sent, &sent2
    ));
    // Withdrawal stops unsent work, while a submitted receipt remains pollable.
    a.store
        .save_conversion_consent(
            lead,
            &Consent {
                ad_user_data: Decision::Denied,
                ad_personalization: Decision::Denied,
                effective_at: Some(Utc::now()),
                reference: "withdrawn".into(),
                version: 1,
            },
            "test",
        )
        .await
        .unwrap();
    assert_eq!(row(&a, &d2["id"]).await["review_required"], true);
    due(&a).await;
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d2["id"]).await["state"], "processed");
    assert_eq!(
        p.deliveries.lock().unwrap().len(),
        2,
        "withdrawal must not pretend to retract"
    );
    let other = Store {
        pool: a.store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    assert!(other.delivery_list().await.unwrap().is_empty());
    assert!(
        other
            .queue_delivery(d2["id"].as_str().unwrap().parse().unwrap(), "test")
            .await
            .is_err()
    );
    task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn delivery_retry_ambiguity_crash_and_stale_preparations() {
    let (a, p, task) = fixture().await;
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    callback(a.clone(), &state, &cookie, "good").await;
    let (_, j) = prepare(a.clone()).await;
    let d = validate(a.clone(), &j).await;
    a.store.set_delivery_enabled(true, 0, "test").await.unwrap();
    queue(&a, &d).await;
    p.delivery_http.store(429, Ordering::SeqCst);
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "retry");
    p.delivery_http.store(503, Ordering::SeqCst);
    due(&a).await;
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "uncertain");
    due(&a).await;
    assert!(!delivery::run_one(&a).await.unwrap());
    assert_eq!(
        p.deliveries.lock().unwrap().len(),
        2,
        "ambiguous upload must never replay"
    );
    // Another event is never allowed to change the previously attempted destination.
    let q = stage(&a, "qualified").await;
    let qd = validate(a.clone(), &q).await;
    queue(&a, &qd).await;
    let claims = tokio::join!(a.store.claim_delivery(), a.store.claim_delivery());
    assert_eq!(
        usize::from(claims.0.unwrap().is_some()) + usize::from(claims.1.unwrap().is_some()),
        1
    );
    due(&a).await;
    assert!(a.store.claim_delivery().await.unwrap().is_none());
    assert_eq!(
        row(&a, &qd["id"]).await["state"],
        "uncertain",
        "crashed send must not replay"
    );
    // A disabled destination doesn't send even if a queue claim raced with pause.
    a.store
        .set_delivery_enabled(false, 1, "test")
        .await
        .unwrap();
    assert_eq!(
        a.store.setup_status().await.unwrap()["google_uploads_enabled"],
        false
    );
    task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn delivery_pause_owner_access_and_fresh_validation() {
    let (a, p, task) = fixture().await;
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    callback(a.clone(), &state, &cookie, "good").await;
    let (_, j) = prepare(a.clone()).await;
    let d = validate(a.clone(), &j).await;
    let path = format!(
        "/api/conversions/deliveries/{}/queue",
        d["id"].as_str().unwrap()
    );
    assert_eq!(
        api(a.clone(), "POST", &path, "wrong", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    let key = a
        .store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "Not a delivery credential".into(),
                scopes: vec!["read".into(), "setup:write".into()],
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        api(
            a.clone(),
            "PUT",
            "/api/conversions/delivery/settings",
            key["secret"].as_str().unwrap(),
            json!({"enabled":true,"version":0})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let other = Store {
        pool: a.store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    let (member, user) = google_tests::hosted_token(&other).await;
    assert_eq!(
        api(
            a.clone(),
            "PUT",
            "/api/conversions/delivery/settings",
            &member,
            json!({"enabled":true,"version":0})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        api(a.clone(), "POST", &path, &member, json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE hosted_memberships SET enabled=false WHERE workspace_id=$1 AND user_id=$2")
        .bind(other.workspace)
        .bind(user)
        .execute(&a.store.pool)
        .await
        .unwrap();
    assert_eq!(
        api(
            a.clone(),
            "PUT",
            "/api/conversions/delivery/settings",
            &member,
            json!({"enabled":true,"version":0})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        api(a.clone(), "POST", &path, &member, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    a.store.set_delivery_enabled(true, 0, "test").await.unwrap();
    queue(&a, &d).await;
    a.store
        .set_delivery_enabled(false, 1, "test")
        .await
        .unwrap();
    assert!(!delivery::run_one(&a).await.unwrap());
    assert_eq!(row(&a, &d["id"]).await["state"], "needs_review");
    assert!(p.deliveries.lock().unwrap().is_empty());
    a.store.set_delivery_enabled(true, 2, "test").await.unwrap();
    let d = validate(a.clone(), &j).await;
    queue(&a, &d).await;
    // An expired account/action check stops a queued send even if no UI changed.
    sqlx::query(
        "UPDATE google_connections SET checked_at=now()-interval '25 hours' WHERE workspace_id=$1",
    )
    .bind(a.store.workspace)
    .execute(&a.store.pool)
    .await
    .unwrap();
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "needs_review");
    assert!(p.deliveries.lock().unwrap().is_empty());
    // Clearing validation is safe and must not deserialize NULL into a panic.
    a.store.clear_google_validation(1).await.unwrap();
    assert_ne!(
        api(
            a.clone(),
            "POST",
            &format!("/api/conversions/{}/validate", j["id"].as_str().unwrap()),
            &a.api_key,
            json!({"version":j["version"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn delivery_confirmed_rejection_and_bounded_receipt_checks() {
    let (a, p, task) = fixture().await;
    let (state, cookie) = begin(a.clone(), &p, &a.api_key, 0).await;
    callback(a.clone(), &state, &cookie, "good").await;
    let (_, j) = prepare(a.clone()).await;
    let d = validate(a.clone(), &j).await;
    a.store.set_delivery_enabled(true, 0, "test").await.unwrap();
    queue(&a, &d).await;
    p.delivery_http.store(403, Ordering::SeqCst);
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "needs_review");
    assert_eq!(row(&a, &d["id"]).await["attempted_at"], Value::Null);
    // Explicit revalidation can recover a definitively rejected request.
    let d = validate(a.clone(), &j).await;
    queue(&a, &d).await;
    p.delivery_http.store(0, Ordering::SeqCst);
    delivery::run_one(&a).await.unwrap();
    p.processing.store(true, Ordering::SeqCst);
    sqlx::query("UPDATE conversion_deliveries SET polls=23,due_at=now() WHERE workspace_id=$1 AND state='submitted'").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "uncertain");
    let count = p.deliveries.lock().unwrap().len();
    // Recheck polls a saved receipt, never re-sends conversion data.
    let id = d["id"].as_str().unwrap().parse().unwrap();
    a.store.recheck_delivery(id).await.unwrap();
    p.processing.store(false, Ordering::SeqCst);
    delivery::run_one(&a).await.unwrap();
    assert_eq!(row(&a, &d["id"]).await["state"], "processed");
    assert_eq!(p.deliveries.lock().unwrap().len(), count);
    task.abort();
}
