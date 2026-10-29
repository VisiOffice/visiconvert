use convert_domain::NewLead;
use convert_store::Store;
use sqlx::{Executor, Row, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
#[ignore = "Requires a disposable VISICONVERT_TEST_DATABASE_URL"]
async fn migration_preserves_existing_reviews_ids_and_booking_values() {
    let database = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let admin = PgPoolOptions::new().connect(&database).await.unwrap();
    admin
        .execute("CREATE SCHEMA inquiry_upgrade_fixture")
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .after_connect(|conn, _| {
            Box::pin(async move {
                conn.execute("SET search_path TO inquiry_upgrade_fixture")
                    .await?;
                Ok(())
            })
        })
        .connect(&database)
        .await
        .unwrap();
    for migration in [
        include_str!("../migrations/0001_pilot.sql"),
        include_str!("../migrations/0002_hosted_access.sql"),
        include_str!("../migrations/0003_hosted_credential_versions.sql"),
        include_str!("../migrations/0004_workspace_twilio.sql"),
        include_str!("../migrations/0005_website_capture.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    let workspace = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces(id,name) VALUES($1,'Upgrade fixture')")
        .bind(workspace)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,qualification,notes,version) VALUES($1,$2,'old-id','Existing customer','existing@example.com','form','Google','Existing campaign','qualified','Existing notes',7)").bind(id).bind(workspace).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO outcomes(workspace_id,lead_id,external_id,amount_minor,currency,status,revision) VALUES($1,$2,'old-booking',12005,'USD','booked',3)").bind(workspace).bind(id).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0006_inquiry_grouping.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let store = Store {
        pool: pool.clone(),
        workspace,
    };
    let inquiry = store.inquiry(id).await.unwrap();
    assert_eq!(inquiry.lead.id, id);
    assert_eq!(inquiry.primary_lead_id, id);
    assert_eq!(inquiry.lead.qualification, "qualified");
    assert_eq!(inquiry.lead.notes, "Existing notes");
    assert_eq!(inquiry.lead.version, 7);
    assert_eq!(inquiry.lead.booked_minor, 12005);
    let new = NewLead {
        external_id: "retry".into(),
        name: "New customer".into(),
        contact: "new@example.com".into(),
        source: "Referral".into(),
        campaign: "".into(),
        notes: "".into(),
    };
    let first = store.add_lead(&new).await.unwrap();
    assert_eq!(store.add_lead(&new).await.unwrap(), first);
    let counts=sqlx::query("SELECT (SELECT count(*) FROM leads) AS interactions,(SELECT count(*) FROM inquiries) AS inquiries").fetch_one(&pool).await.unwrap();
    assert_eq!(counts.get::<i64, _>("interactions"), 2);
    assert_eq!(counts.get::<i64, _>("inquiries"), 2);
    let old_call = Uuid::new_v4();
    sqlx::query("INSERT INTO tracking_numbers(id,workspace_id,label,phone_number,destination,source,campaign) VALUES($1,$2,'Legacy number','+12025550411','+12025550412','Legacy source','Legacy campaign')").bind(Uuid::new_v4()).bind(workspace).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,tracking_number,destination) VALUES($1,$2,'legacy-call','Legacy caller','+12025550413','call','Legacy source','Legacy campaign','+12025550411','+12025550412')").bind(old_call).bind(workspace).execute(&pool).await.unwrap();
    for migration in [
        include_str!("../migrations/0007_dynamic_numbers.sql"),
        include_str!("../migrations/0008_conversion_preparation.sql"),
        include_str!("../migrations/0009_public_api.sql"),
        include_str!("../migrations/0010_google_connection.sql"),
        include_str!("../migrations/0011_signalwire.sql"),
        include_str!("../migrations/0012_setup_scope.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    let site = store
        .save_site(
            None,
            &convert_store::sites::SiteInput {
                name: "Legacy pool".into(),
                origin: "https://legacy.example".into(),
                allowed_fields: vec![],
                enabled: true,
                version: 0,
            },
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO dni_pools(site_id,workspace_id,fallback_phone,enabled,version) VALUES($1,$2,'+12025550412',false,7)").bind(site).bind(workspace).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0013_dni_destination.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let migrated_pool = store.dni_pool(site).await.unwrap();
    assert_eq!(migrated_pool["pool"]["fallback_phone"], "+12025550412");
    assert_eq!(migrated_pool["pool"]["destination_phone"], "+12025550412");
    assert_eq!(migrated_pool["pool"]["version"], 7);
    assert_eq!(migrated_pool["pool"]["enabled"], false);
    // A restored older image can still insert a pool without the new column.
    let legacy_site = store
        .save_site(
            None,
            &convert_store::sites::SiteInput {
                name: "Rollback pool".into(),
                origin: "https://rollback.example".into(),
                allowed_fields: vec![],
                enabled: false,
                version: 0,
            },
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO dni_pools(site_id,workspace_id,fallback_phone,enabled) VALUES($1,$2,'+12025550414',false)").bind(legacy_site).bind(workspace).execute(&pool).await.unwrap();
    assert_eq!(
        store.dni_pool(legacy_site).await.unwrap()["pool"]["destination_phone"],
        "+12025550414"
    );
    // Existing numbers stay live and unpurchased; the phone number is still unique while live.
    sqlx::raw_sql(include_str!("../migrations/0014_managed_numbers.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let numbers = store.numbers().await.unwrap();
    assert_eq!(numbers[0].provider, convert_domain::PhoneProvider::Twilio);
    assert!(!numbers[0].purchased);
    assert!(numbers[0].release_requested_at.is_none());
    assert!(
        sqlx::query("INSERT INTO tracking_numbers(id,workspace_id,label,phone_number,destination,source,campaign) VALUES($1,$2,'Duplicate','+12025550411','+12025550415','Legacy source','Legacy campaign')")
            .bind(Uuid::new_v4())
            .bind(workspace)
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .call_event(&convert_store::CallEvent {
                sid: "legacy-call".into(),
                to: "+12025550411".into(),
                from: "+12025550413".into(),
                fingerprint: "migration-retry".into(),
                kind: convert_store::CallEventKind::Voice
            })
            .await
            .unwrap(),
        "+12025550412"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT provider FROM leads WHERE id=$1")
            .bind(old_call)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "twilio"
    );
    let after = store.inquiry(id).await.unwrap();
    assert_eq!(after.lead.notes, "Existing notes");
    assert_eq!(after.lead.version, 7);
    assert_eq!(after.lead.booked_minor, 12005);
    let keys = store.api_keys().await.unwrap();
    assert!(keys["keys"].as_array().unwrap().is_empty());
    assert!(keys["history"].as_array().unwrap().is_empty());
    let conversions = store.conversion_preview().await.unwrap();
    assert_eq!(conversions["settings"]["version"], 0);
    assert!(conversions["jobs"].as_array().unwrap().is_empty());
    assert_eq!(conversions["candidates"].as_array().unwrap().len(), 2);
    for candidate in conversions["candidates"].as_array().unwrap() {
        assert_eq!(candidate["eligible"], false);
        assert!(
            candidate["reasons"]
                .to_string()
                .contains("Original event time")
        );
    }
    pool.close().await;
    admin
        .execute("DROP SCHEMA inquiry_upgrade_fixture CASCADE")
        .await
        .unwrap();
    admin.close().await;
}
