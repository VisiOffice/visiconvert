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
    for migration in [
        include_str!("../migrations/0007_dynamic_numbers.sql"),
        include_str!("../migrations/0008_conversion_preparation.sql"),
        include_str!("../migrations/0009_public_api.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
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
