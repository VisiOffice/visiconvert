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
fn lead(external: &str, source: &str) -> NewLead {
    NewLead {
        external_id: external.into(),
        name: format!("Customer {external}"),
        contact: "repeat@example.com".into(),
        source: source.into(),
        campaign: "Test".into(),
        notes: format!("Original note {external}"),
    }
}
fn booking() -> OutcomeInput {
    OutcomeInput {
        external_id: "inquiry-booking".into(),
        amount_minor: 25005,
        currency: "USD".into(),
        status: "booked".into(),
        revision: 1,
    }
}
#[tokio::test]
#[ignore = "Requires a disposable VISICONVERT_TEST_DATABASE_URL"]
async fn grouping_preserves_interactions_isolation_retries_and_money() {
    let database = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&database, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let key = "grouping-test-operator-key-at-least-32";
    let workspace = Uuid::new_v4();
    let owner = token(&store, workspace).await;
    let other = token(&store, Uuid::new_v4()).await;
    let tenant = Store {
        pool: store.pool.clone(),
        workspace,
    };
    let app = Arc::new(App {
        store: store.clone(),
        api_key: key.into(),
        public_url: "https://engine.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: "grouping-service-test-key-at-least-32".into(),
            cloud_origin: "https://cloud.example".into(),
            app_origin: None,
        }),
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    });
    let a = tenant.add_lead(&lead("a", "Google")).await.unwrap();
    let b = tenant.add_lead(&lead("b", "Referral")).await.unwrap();
    // Same contact does not automatically group. Retried intake creates neither a lead nor an orphan inquiry.
    assert_eq!(tenant.add_lead(&lead("a", "Google")).await.unwrap(), a);
    assert_eq!(tenant.inquiries().await.unwrap().len(), 2);
    let input = OutcomeInput {
        external_id: "first-booking".into(),
        amount_minor: 10000,
        ..booking()
    };
    tenant.put_outcome(a, &input).await.unwrap();
    tenant.put_outcome(b, &booking()).await.unwrap();
    tenant
        .update_lead(
            b,
            &LeadUpdate {
                version: 2,
                qualification: Qualification::Qualified,
                notes: "Original note b".into(),
            },
        )
        .await
        .unwrap();
    let av = tenant.inquiry(a).await.unwrap().lead.version;
    let bv = tenant.inquiry(b).await.unwrap().lead.version;
    let combine = json!({"target_id":a,"version":bv,"target_version":av,"reason":"Same rental confirmed by customer"});
    let path = format!("/api/inquiries/{b}/combine");
    assert_eq!(
        call(app.clone(), "POST", &path, "wrong", combine.clone())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(app.clone(), "POST", &path, &other, combine.clone())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (first, second) = tokio::join!(
        call(app.clone(), "POST", &path, &owner, combine.clone()),
        call(app.clone(), "POST", &path, &owner, combine)
    );
    assert_eq!(
        [first.0, second.0]
            .iter()
            .filter(|s| s.is_success())
            .count(),
        1
    );
    let result = tenant.inquiry(a).await.unwrap();
    assert!(result.lead.version > av && result.lead.version > bv);
    assert!(
        tenant
            .update_lead(
                b,
                &LeadUpdate {
                    version: bv,
                    qualification: Qualification::Spam,
                    notes: "Stale review".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(result.interaction_count, 2);
    assert_eq!(result.lead.booked_minor, 35005);
    assert_eq!(result.lead.source, "Google");
    assert_eq!(result.lead.notes, "Original note a");
    assert_eq!(tenant.inquiries().await.unwrap().len(), 1);
    let legacy = tenant.leads().await.unwrap();
    assert_eq!(legacy.len(), 2);
    assert!(legacy.iter().all(|l| l.inquiry_id == Some(a)));
    assert_eq!(tenant.add_lead(&lead("b", "Referral")).await.unwrap(), b);
    assert_eq!(tenant.inquiries().await.unwrap().len(), 1);
    let summary = tenant.summary().await.unwrap();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].leads, 1);
    assert_eq!(summary[0].bookings, 2);
    assert_eq!(summary[0].booked_minor, 35005);
    assert_eq!(
        call(
            app.clone(),
            "GET",
            &format!("/api/inquiries/{a}"),
            &other,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            app.clone(),
            "GET",
            &format!("/api/inquiries/{b}/history"),
            &other,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let history = call(
        app.clone(),
        "GET",
        &format!("/api/inquiries/{a}/history"),
        &owner,
        Value::Null,
    )
    .await
    .1;
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "combined_from"
                && e["actor"].as_str().unwrap().starts_with("hosted user ")
                && e["payload"]["source_snapshot"]["notes"] == "Original note b")
    );
    // Booking retries after combine succeed against the original interaction and canonical inquiry.
    tenant.put_outcome(b, &booking()).await.unwrap();
    tenant
        .put_inquiry_outcome(a, &booking(), "test")
        .await
        .unwrap();
    assert_eq!(tenant.inquiry(a).await.unwrap().lead.booked_minor, 35005);
    tenant
        .update_lead(
            b,
            &LeadUpdate {
                version: result.lead.version,
                qualification: Qualification::Qualified,
                notes: "One confirmed rental".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        tenant
            .leads()
            .await
            .unwrap()
            .iter()
            .all(|l| l.qualification == "qualified")
    );
    assert_eq!(tenant.summary().await.unwrap()[0].qualified, 1);
    // A stale grouping form cannot override a review or newly recorded booking.
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("/api/inquiries/{a}/split"),
            &owner,
            json!({"interaction_id":b,"version":result.lead.version,"reason":"Wrong grouping"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let current = tenant.inquiry(a).await.unwrap();
    let split=call(app.clone(),"POST",&format!("/api/inquiries/{a}/split"),&owner,json!({"interaction_id":b,"version":current.lead.version,"reason":"Different rental after review"})).await;
    assert_eq!(split.0, StatusCode::OK);
    let new_id: Uuid = split.1["id"].as_str().unwrap().parse().unwrap();
    let separate = tenant.inquiry(new_id).await.unwrap();
    assert!(separate.lead.version > current.lead.version);
    assert!(
        tenant
            .update_lead(
                b,
                &LeadUpdate {
                    version: current.lead.version,
                    qualification: Qualification::Spam,
                    notes: "Stale split review".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(separate.lead.source, "Referral");
    assert_eq!(separate.lead.qualification, "unreviewed");
    assert_eq!(separate.lead.notes, "");
    assert_eq!(separate.lead.booked_minor, 25005);
    assert_eq!(tenant.inquiry(a).await.unwrap().lead.booked_minor, 10000);
    tenant.put_outcome(b, &booking()).await.unwrap();
    assert!(
        tenant
            .put_inquiry_outcome(a, &booking(), "test")
            .await
            .is_err()
    );
    let mut canceled = booking();
    canceled.status = "canceled".into();
    canceled.revision = 2;
    tenant
        .put_inquiry_outcome(new_id, &canceled, "test")
        .await
        .unwrap();
    assert_eq!(
        tenant
            .summary()
            .await
            .unwrap()
            .iter()
            .map(|r| r.booked_minor)
            .sum::<i64>(),
        10000
    );
    // Detach the reporting interaction: the remaining source must move explicitly.
    let c = tenant.add_lead(&lead("c", "Organic")).await.unwrap();
    tenant
        .combine_inquiries(
            c,
            &convert_store::inquiries::CombineInput {
                target_id: a,
                version: 1,
                target_version: tenant.inquiry(a).await.unwrap().lead.version,
                reason: "Related call".into(),
            },
            "test",
        )
        .await
        .unwrap();
    tenant
        .split_inquiry(
            a,
            &convert_store::inquiries::SplitInput {
                interaction_id: a,
                version: tenant.inquiry(a).await.unwrap().lead.version,
                reason: "Separate original opportunity".into(),
            },
            "test",
        )
        .await
        .unwrap();
    let remaining = tenant.inquiry(a).await.unwrap();
    assert_eq!(remaining.primary_lead_id, c);
    assert_eq!(remaining.lead.source, "Organic");
    // Legacy IDs still refer to their own interaction, even when its inquiry ID differs.
    assert_ne!(
        tenant
            .leads()
            .await
            .unwrap()
            .iter()
            .find(|l| l.id == a)
            .unwrap()
            .inquiry_id,
        Some(a)
    );
    assert_eq!(
        call(app.clone(), "GET", "/api/inquiries", key, Value::Null)
            .await
            .1,
        json!([])
    );
}

#[tokio::test]
#[ignore = "Requires a disposable VISICONVERT_TEST_DATABASE_URL"]
async fn call_callbacks_and_website_receipts_survive_grouping() {
    use convert_store::{CallEvent, CallEventKind, inquiries::CombineInput, sites::SiteInput};
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let target = store.add_lead(&lead("main", "Referral")).await.unwrap();
    let phone = format!("+12025{:06}", Uuid::new_v4().as_u128() % 900000);
    store
        .add_number(&NewNumber {
            label: "Grouping call test".into(),
            phone_number: phone.clone(),
            destination: "+12025559998".into(),
            source: "Campaign phone".into(),
            campaign: "Phone campaign".into(),
        })
        .await
        .unwrap();
    let sid = format!("CA{}", Uuid::new_v4().simple());
    let event = CallEvent {
        sid: sid.clone(),
        from: "+12025550000".into(),
        to: phone,
        fingerprint: format!("{sid}-voice"),
        kind: CallEventKind::Voice,
    };
    store.call_event(&event).await.unwrap();
    let call = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.channel == "call")
        .unwrap();
    store
        .combine_inquiries(
            call.id,
            &CombineInput {
                target_id: target,
                version: call.version,
                target_version: 1,
                reason: "Same job, confirmed by operator".into(),
            },
            "test",
        )
        .await
        .unwrap();
    store.call_event(&event).await.unwrap();
    store
        .call_event(&CallEvent {
            fingerprint: format!("{sid}-dial"),
            kind: CallEventKind::Dial {
                status: "completed".into(),
                duration: Some(42),
            },
            ..event
        })
        .await
        .unwrap();
    assert_eq!(store.inquiries().await.unwrap().len(), 1);
    let call = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.channel == "call")
        .unwrap();
    assert_eq!(call.call_status.as_deref(), Some("destination_answered"));
    assert_eq!(call.duration_seconds, Some(42));
    assert_eq!(call.source, "Campaign phone");
    let site = store
        .save_site(
            None,
            &SiteInput {
                name: "Grouping website".into(),
                origin: "https://example.com".into(),
                allowed_fields: vec!["name".into(), "email".into()],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    let session = Uuid::new_v4();
    store.capture_session(site,"https://example.com",session,&json!({"source":"Paid web","utm_campaign":"Web campaign","gclid":"test-click","consent":"granted"})).await.unwrap();
    let fields = std::collections::BTreeMap::from([
        ("name".into(), "Website customer".into()),
        ("email".into(), "repeat@example.com".into()),
    ]);
    store
        .capture_form(
            site,
            "https://example.com",
            session,
            "submission-1",
            &fields,
        )
        .await
        .unwrap();
    let web = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.source == "Paid web")
        .unwrap();
    let evidence = store.website_evidence(web.id).await.unwrap();
    store
        .combine_inquiries(
            web.id,
            &CombineInput {
                target_id: target,
                version: 1,
                target_version: store.inquiry(target).await.unwrap().lead.version,
                reason: "Website request for same job".into(),
            },
            "test",
        )
        .await
        .unwrap();
    store
        .capture_form(
            site,
            "https://example.com",
            session,
            "submission-1",
            &fields,
        )
        .await
        .unwrap();
    assert_eq!(store.website_evidence(web.id).await.unwrap(), evidence);
    let group = store.inquiry(target).await.unwrap();
    assert_eq!(group.interaction_count, 3);
    assert_eq!(group.lead.source, "Referral");
    let demo = store.add_lead(&lead("demo", "Demo")).await.unwrap();
    sqlx::query("UPDATE leads SET is_demo=true WHERE id=$1")
        .bind(demo)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .combine_inquiries(
                demo,
                &CombineInput {
                    target_id: target,
                    version: 1,
                    target_version: group.lead.version,
                    reason: "Invalid demo mix".into()
                },
                "test"
            )
            .await
            .is_err()
    );
    assert_eq!(store.inquiry(target).await.unwrap().interaction_count, 3);
}
