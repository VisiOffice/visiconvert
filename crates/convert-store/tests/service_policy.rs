use chrono::Utc;
use convert_domain::{NewNumber, PhoneProvider};
use convert_store::{Store, dni::PoolInput, service_policy::ServicePolicy, sites::SiteInput};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn managed_policy_ordering_isolation_expiry_dni_and_release_recovery() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    let project = Uuid::new_v4();
    // BYOC never inherits managed billing policy or release authority.
    other
        .save_signalwire_connection("fixture.signalwire.com", project, "cipher", "cipher", 0)
        .await
        .unwrap();
    assert!(other.managed_forwarding_allowed().await.unwrap());
    store
        .save_managed_signalwire_connection("fixture.signalwire.com", project, "cipher", "cipher")
        .await
        .unwrap();
    assert!(!store.managed_forwarding_allowed().await.unwrap());
    let mut policy = ServicePolicy {
        project_id: project,
        revision: 1,
        state: "active".into(),
        reason: "paid".into(),
        valid_until: Utc::now().timestamp() + 3600,
        release_at: None,
    };
    assert!(other.apply_service_policy(&policy).await.is_err());
    policy.project_id = Uuid::new_v4();
    assert!(store.apply_service_policy(&policy).await.is_err());
    policy.project_id = project;
    store.apply_service_policy(&policy).await.unwrap();
    store.apply_service_policy(&policy).await.unwrap();
    assert!(store.managed_forwarding_allowed().await.unwrap());
    policy.state = "suspended".into();
    assert!(store.apply_service_policy(&policy).await.is_err()); // same revision, changed payload
    policy.revision = 2;
    policy.reason = "payment_required".into();
    policy.release_at = Some(Utc::now().timestamp() - 1);
    store.apply_service_policy(&policy).await.unwrap();
    policy.revision = 1;
    assert!(store.apply_service_policy(&policy).await.is_err());
    assert!(!store.managed_forwarding_allowed().await.unwrap());
    policy.revision = 3;
    policy.state = "active".into();
    policy.reason = "paid".into();
    policy.release_at = None;
    store.apply_service_policy(&policy).await.unwrap();
    let input = NewNumber {
        provider: PhoneProvider::Signalwire,
        label: "Managed".into(),
        phone_number: "+12025550789".into(),
        destination: "+12025550790".into(),
        source: "Website".into(),
        campaign: "Lifecycle".into(),
    };
    store
        .add_number_with_connection(&input, Some(1))
        .await
        .unwrap();
    let number = store.numbers().await.unwrap()[0].id;
    sqlx::query("UPDATE tracking_numbers SET carrier_sid=$2,purchased_at=now() WHERE id=$1")
        .bind(number)
        .bind(Uuid::new_v4().to_string())
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO number_purchases(workspace_id,request_id,phone_number,status,number_id,project_id,managed) VALUES($1,$2,$3,'registered',$4,$5,true)")
        .bind(store.workspace).bind(Uuid::new_v4()).bind(&input.phone_number).bind(number).bind(project).execute(&store.pool).await.unwrap();
    let site = store
        .save_site(
            None,
            &SiteInput {
                name: "Website".into(),
                origin: "https://policy.example".into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    store
        .save_dni_pool(
            site,
            &PoolInput {
                fallback_phone: input.destination.clone(),
                destination_phone: None,
                enabled: true,
                version: 0,
                number_ids: vec![number],
                dedicated_numbers: true,
            },
        )
        .await
        .unwrap();
    let session = Uuid::new_v4();
    store
        .capture_session(
            site,
            "https://policy.example",
            session,
            &json!({"landing_url":"https://policy.example/quote"}),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .allocate_dni(site, "https://policy.example", session)
            .await
            .unwrap()["phone_number"],
        input.phone_number
    );
    policy.revision = 4;
    policy.state = "suspended".into();
    policy.reason = "canceled".into();
    policy.release_at = Some(Utc::now().timestamp() - 1);
    store.apply_service_policy(&policy).await.unwrap();
    let disabled = store
        .allocate_dni(site, "https://policy.example", session)
        .await
        .unwrap();
    assert_eq!(disabled["status"], "disabled");
    assert_eq!(disabled["fallback_phone"], input.destination);
    let due = convert_store::managed::due_releases(&store.pool, 45)
        .await
        .unwrap();
    assert!(due.iter().any(|n| n.number_id == number));
    // Recovering after a worker's snapshot invalidates that release claim.
    policy.revision = 5;
    policy.state = "active".into();
    policy.reason = "paid".into();
    policy.release_at = None;
    store.apply_service_policy(&policy).await.unwrap();
    assert!(!store.claim_release(number, 45).await.unwrap());
    // Expiry blocks forwarding but cannot release numbers, even for an old suspension.
    policy.revision = 6;
    policy.state = "suspended".into();
    policy.reason = "canceled".into();
    policy.release_at = Some(Utc::now().timestamp() - 1);
    store.apply_service_policy(&policy).await.unwrap();
    sqlx::query("UPDATE managed_service_policies SET valid_until=now()-interval '1 second' WHERE workspace_id=$1").bind(store.workspace).execute(&store.pool).await.unwrap();
    assert!(!store.claim_release(number, 45).await.unwrap());
    assert!(!store.managed_forwarding_allowed().await.unwrap());
    policy.revision = 7;
    store.apply_service_policy(&policy).await.unwrap();
    assert!(store.claim_release(number, 45).await.unwrap());
    policy.revision = 8;
    policy.state = "active".into();
    policy.reason = "paid".into();
    policy.release_at = None;
    store.apply_service_policy(&policy).await.unwrap();
    assert!(store.restore_number(number).await.is_err());
    assert!(store.claim_release(number, 45).await.unwrap()); // retries same irreversible carrier action
    let evidence: serde_json::Value =
        sqlx::query_scalar("SELECT release_policy FROM tracking_numbers WHERE id=$1")
            .bind(number)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(evidence["revision"], 7);
    assert_eq!(evidence["reason"], "canceled");
    store.mark_released(number).await.unwrap();
    assert!(!store.claim_release(number, 45).await.unwrap());
    assert!(other.managed_forwarding_allowed().await.unwrap());
}
