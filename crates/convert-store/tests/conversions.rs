use chrono::{Duration, Utc};
use convert_domain::{LeadUpdate, NewLead, OutcomeInput, Qualification};
use convert_store::{
    Error, Store,
    conversions::{Consent, Decision, Settings, StageInput},
    inquiries::{CombineInput, SplitInput},
    sites::SiteInput,
};
use serde_json::{Value, json};
use uuid::Uuid;
async fn candidate(store: &Store, kind: &str) -> Value {
    store.conversion_preview().await.unwrap()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == kind)
        .unwrap()
        .clone()
}
fn input(c: &Value) -> StageInput {
    StageInput {
        kind: c["kind"].as_str().unwrap().into(),
        entity_key: c["entity_key"].as_str().unwrap().into(),
        fingerprint: c["fingerprint"].as_str().unwrap().into(),
        version: c["job_version"].as_i64().unwrap() as i32,
    }
}
async fn version(store: &Store, id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT version FROM inquiries WHERE workspace_id=$1 AND id=$2")
        .bind(store.workspace)
        .bind(id)
        .fetch_one(&store.pool)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn consent_staging_retries_revisions_grouping_and_isolation() {
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    let site = store
        .save_site(
            None,
            &SiteInput {
                name: "Ads test".into(),
                origin: "https://ads.example".into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    let session = Uuid::new_v4();
    store.capture_session(site,"https://ads.example",session,&json!({"source":"google","gclid":"test-click","consent":"granted","landing_url":"https://ads.example/"})).await.unwrap();
    store
        .capture_form(
            site,
            "https://ads.example",
            session,
            "submission",
            &Default::default(),
        )
        .await
        .unwrap();
    let lead = store.leads().await.unwrap()[0].id;
    store
        .update_inquiry(
            lead,
            &LeadUpdate {
                qualification: Qualification::Qualified,
                notes: "Private notes".into(),
                version: 1,
            },
            "test",
        )
        .await
        .unwrap();
    let mut booking = OutcomeInput {
        external_id: "sale-123".into(),
        amount_minor: 12345,
        currency: "USD".into(),
        status: "booked".into(),
        revision: 1,
    };
    store.put_outcome(lead, &booking).await.unwrap();
    let mut config = Settings {
        customer_id: "1234567890".into(),
        qualified_action_id: "123".into(),
        booking_action_id: "456".into(),
        ..Default::default()
    };
    store
        .save_conversion_settings(&config, "test")
        .await
        .unwrap();
    assert!(matches!(
        store.save_conversion_settings(&config, "stale").await,
        Err(Error::Conflict)
    ));
    config.version = 1;
    let c = candidate(&store, "booking").await;
    assert_eq!(
        c["eligible"], false,
        "tracking consent must not imply advertising consent"
    );
    assert!(store.stage_conversion(&input(&c), "test").await.is_err());
    let mut consent = Consent {
        ad_user_data: Decision::Granted,
        ad_personalization: Decision::Denied,
        effective_at: Some(Utc::now() - Duration::hours(1)),
        reference: "CMP receipt 123".into(),
        version: 0,
    };
    assert!(matches!(
        other.save_conversion_consent(lead, &consent, "test").await,
        Err(Error::NotFound)
    ));
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    assert!(matches!(
        store.save_conversion_consent(lead, &consent, "test").await,
        Err(Error::Conflict)
    ));
    consent.version = 1;
    let c = candidate(&store, "booking").await;
    assert_eq!(c["eligible"], true, "{c}");
    assert_eq!(c["snapshot"]["amount_minor"], 12345);
    assert!(!c.to_string().contains("Private notes"));
    assert!(matches!(
        other.stage_conversion(&input(&c), "test").await,
        Err(Error::NotFound)
    ));
    let stage = input(&c);
    let (a, b) = tokio::join!(
        store.stage_conversion(&stage, "test"),
        store.stage_conversion(&stage, "test")
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap(), "concurrent retry must return the same job");
    let id: Uuid = serde_json::from_value(a["id"].clone()).unwrap();
    let transaction = a["transaction_id"].clone();
    let timestamp = a["snapshot"]["event_at"].clone();
    assert_eq!(
        store.conversion_preview().await.unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        other.conversion_preview().await.unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(other.cancel_conversion(id, 1, "test").await.is_err());
    // Exact retry of business input does not invalidate anything.
    store.put_outcome(lead, &booking).await.unwrap();
    assert_eq!(
        candidate(&store, "booking").await["job_state"],
        "awaiting_connection"
    );
    booking.revision = 2;
    booking.amount_minor = 67890;
    store.put_outcome(lead, &booking).await.unwrap();
    assert!(matches!(
        store.stage_conversion(&stage, "stale").await,
        Err(Error::Conflict)
    ));
    let c = candidate(&store, "booking").await;
    assert_eq!(c["job_state"], "needs_review");
    assert_eq!(c["snapshot"]["event_at"], timestamp);
    let job = store.stage_conversion(&input(&c), "test").await.unwrap();
    assert_eq!(job["id"], a["id"]);
    assert_eq!(job["transaction_id"], transaction);
    assert_eq!(job["snapshot"]["amount_minor"], 67890);
    // Withdrawing advertising consent prevents restaging, while preserving audit.
    consent.ad_user_data = Decision::Denied;
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    consent.version += 1;
    let c = candidate(&store, "booking").await;
    assert_eq!(c["job_state"], "needs_review");
    assert_eq!(c["eligible"], false);
    assert!(store.stage_conversion(&input(&c), "test").await.is_err());
    consent.ad_user_data = Decision::Granted;
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    consent.version += 1;
    let c = candidate(&store, "booking").await;
    let job = store.stage_conversion(&input(&c), "test").await.unwrap();
    store
        .cancel_conversion(id, job["version"].as_i64().unwrap() as i32, "test")
        .await
        .unwrap();
    assert!(matches!(
        store.stage_conversion(&input(&c), "stale").await,
        Err(Error::Conflict)
    ));
    let job = store
        .stage_conversion(&input(&candidate(&store, "booking").await), "test")
        .await
        .unwrap();
    assert_eq!(job["transaction_id"], transaction);
    // Remapping requires fresh review and cannot silently change the frozen row.
    config.booking_action_id = "789".into();
    store
        .save_conversion_settings(&config, "test")
        .await
        .unwrap();
    let preview = store.conversion_preview().await.unwrap();
    assert_eq!(
        preview["jobs"][0]["snapshot"]["settings"]["booking_action_id"],
        "456"
    );
    assert_eq!(
        candidate(&store, "booking").await["job_state"],
        "needs_review"
    );
    store
        .stage_conversion(&input(&candidate(&store, "booking").await), "test")
        .await
        .unwrap();
    let target = store
        .add_lead(&NewLead {
            external_id: "target".into(),
            name: "Other interaction".into(),
            contact: "private@example.com".into(),
            source: "Referral".into(),
            campaign: "".into(),
            notes: "".into(),
        })
        .await
        .unwrap();
    store
        .combine_inquiries(
            lead,
            &CombineInput {
                target_id: target,
                version: version(&store, lead).await,
                target_version: 1,
                reason: "Same inquiry".into(),
            },
            "test",
        )
        .await
        .unwrap();
    let c = candidate(&store, "booking").await;
    assert_eq!(c["job_state"], "needs_review");
    assert_eq!(c["snapshot"]["lead_id"], json!(lead));
    assert_eq!(c["snapshot"]["inquiry_id"], json!(target));
    assert_eq!(c["snapshot"]["transaction_id"], transaction);
    store.stage_conversion(&input(&c), "test").await.unwrap();
    store
        .split_inquiry(
            target,
            &SplitInput {
                interaction_id: lead,
                version: version(&store, target).await,
                reason: "Separate inquiry".into(),
            },
            "test",
        )
        .await
        .unwrap();
    let c = candidate(&store, "booking").await;
    assert_eq!(c["job_state"], "needs_review");
    assert_eq!(c["snapshot"]["transaction_id"], transaction);
    booking.revision = 3;
    booking.status = "canceled".into();
    store.put_outcome(lead, &booking).await.unwrap();
    assert_eq!(candidate(&store, "booking").await["eligible"], false);
    booking.revision = 4;
    booking.status = "booked".into();
    store.put_outcome(lead, &booking).await.unwrap();
    sqlx::query("UPDATE website_sessions SET created_at=now()-interval '31 days' WHERE site_id=$1")
        .bind(site)
        .execute(&store.pool)
        .await
        .unwrap();
    let c = candidate(&store, "booking").await;
    assert_eq!(c["eligible"], false);
    assert!(c["reasons"].to_string().contains("30-day"));
    sqlx::query("UPDATE website_sessions SET created_at=now()-interval '1 hour',evidence='{}'::jsonb WHERE site_id=$1").bind(site).execute(&store.pool).await.unwrap();
    let c = candidate(&store, "booking").await;
    assert!(c["reasons"].to_string().contains("No Google click"));
    sqlx::query("DELETE FROM audit_events WHERE workspace_id=$1 AND kind='outcome_updated'")
        .bind(store.workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        candidate(&store, "booking").await["reasons"]
            .to_string()
            .contains("Original event time")
    );
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn inferred_calls_require_opt_in_and_consent_at_event_time() {
    use convert_domain::NewNumber;
    use convert_store::{CallEvent, CallEventKind, dni::PoolInput};
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let site = store
        .save_site(
            None,
            &SiteInput {
                name: "Call ads".into(),
                origin: "https://call-ads.example".into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    let number = NewNumber {
        provider: Default::default(),
        label: "Ads call".into(),
        phone_number: "+12025550178".into(),
        destination: "+12025550179".into(),
        source: "Static".into(),
        campaign: "Static".into(),
    };
    store.add_number(&number).await.unwrap();
    store
        .save_dni_pool(
            site,
            &PoolInput {
                fallback_phone: number.destination.clone(),
                enabled: true,
                version: 0,
                number_ids: vec![store.numbers().await.unwrap()[0].id],
                dedicated_numbers: true,
            },
        )
        .await
        .unwrap();
    let session = Uuid::new_v4();
    store
        .capture_session(
            site,
            "https://call-ads.example",
            session,
            &json!({"source":"google","gbraid":"test-braid","consent":"granted","landing_url":"https://call-ads.example/"}),
        )
        .await
        .unwrap();
    store
        .allocate_dni(site, "https://call-ads.example", session)
        .await
        .unwrap();
    store
        .call_event(&CallEvent {
            sid: format!("CA{}", Uuid::new_v4().simple()),
            to: number.phone_number,
            from: "+12025550177".into(),
            fingerprint: "ads-call-voice".into(),
            kind: CallEventKind::Voice,
        })
        .await
        .unwrap();
    let lead = store.leads().await.unwrap()[0].id;
    store
        .update_inquiry(
            lead,
            &LeadUpdate {
                qualification: Qualification::Qualified,
                notes: "".into(),
                version: 1,
            },
            "test",
        )
        .await
        .unwrap();
    let mut settings = Settings {
        customer_id: "1234567890".into(),
        qualified_action_id: "123".into(),
        ..Default::default()
    };
    store
        .save_conversion_settings(&settings, "test")
        .await
        .unwrap();
    settings.version = 1;
    let mut consent = Consent {
        ad_user_data: Decision::Granted,
        ad_personalization: Decision::Granted,
        effective_at: Some(Utc::now() - Duration::hours(1)),
        reference: "call receipt".into(),
        version: 0,
    };
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    consent.version = 1;
    let c = candidate(&store, "qualified").await;
    assert_eq!(c["eligible"], false);
    assert_eq!(c["snapshot"]["click_identifiers"]["gbraid"], "test-braid");
    assert!(c["reasons"].to_string().contains("explicit opt-in"));
    settings.allow_inferred_calls = true;
    store
        .save_conversion_settings(&settings, "test")
        .await
        .unwrap();
    let c = candidate(&store, "qualified").await;
    assert_eq!(c["eligible"], true, "{c}");
    assert_eq!(c["snapshot"]["confidence"], "inferred");
    store.stage_conversion(&input(&c), "test").await.unwrap();
    consent.effective_at = Some(Utc::now());
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    consent.version += 1;
    let c = candidate(&store, "qualified").await;
    assert_eq!(c["eligible"], false);
    assert!(c["reasons"].to_string().contains("effective at the event"));
    consent.effective_at = Some(Utc::now() - Duration::hours(1));
    consent.ad_personalization = Decision::Unknown;
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    consent.version += 1;
    assert!(
        candidate(&store, "qualified").await["reasons"]
            .to_string()
            .contains("personalization decision is unknown")
    );
    consent.ad_personalization = Decision::Denied;
    store
        .save_conversion_consent(lead, &consent, "test")
        .await
        .unwrap();
    sqlx::query("UPDATE leads SET is_demo=true WHERE id=$1")
        .bind(lead)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        candidate(&store, "qualified").await["reasons"]
            .to_string()
            .contains("Demo data")
    );
}
