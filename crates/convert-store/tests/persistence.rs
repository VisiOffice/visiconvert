use convert_domain::*;
use convert_store::{CallEvent, CallEventKind, Error, Store};
use uuid::Uuid;

// Explicit opt-in: tests migrate a dedicated database and clean only their own workspaces.
#[tokio::test]
#[ignore = "requires VISICONVERT_TEST_DATABASE_URL pointing to a dedicated Postgres database"]
async fn attribution_isolation_idempotency_and_ordering() {
    let url = std::env::var("VISICONVERT_TEST_DATABASE_URL")
        .expect("Set the dedicated test database URL");
    let workspace = Uuid::new_v4();
    let store = Store::connect(&url, workspace).await.unwrap();
    store.migrate().await.unwrap();
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    let input = NewLead {
        external_id: "submission-1".into(),
        name: "Avery".into(),
        contact: "avery@example.com".into(),
        source: "Google Ads".into(),
        campaign: "Spring".into(),
        notes: "".into(),
    };
    let (a, b) = tokio::join!(store.add_lead(&input), store.add_lead(&input));
    let id = a.unwrap();
    assert_eq!(id, b.unwrap());
    assert_eq!(store.leads().await.unwrap().len(), 1);
    let mut changed = input.clone();
    changed.source = "Different".into();
    assert!(matches!(
        store.add_lead(&changed).await,
        Err(Error::Conflict)
    ));
    let update = LeadUpdate {
        version: 1,
        qualification: Qualification::Qualified,
        notes: "Booked service".into(),
    };
    assert!(matches!(
        other.update_lead(id, &update).await,
        Err(Error::NotFound)
    ));
    store.update_lead(id, &update).await.unwrap();
    assert!(matches!(
        store.update_lead(id, &update).await,
        Err(Error::Conflict)
    ));
    assert!(other.leads().await.unwrap().is_empty());
    assert!(other.outcomes(id).await.is_err());
    let mut outcome = OutcomeInput {
        external_id: "booking-1".into(),
        amount_minor: 25000,
        currency: "USD".into(),
        status: "booked".into(),
        revision: 1,
    };
    let (a, b) = tokio::join!(
        store.put_outcome(id, &outcome),
        store.put_outcome(id, &outcome)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(store.summary().await.unwrap()[0].booked_minor, 25000);
    outcome.amount_minor = 30000;
    assert!(matches!(
        store.put_outcome(id, &outcome).await,
        Err(Error::Conflict)
    ));
    outcome.revision = 2;
    store.put_outcome(id, &outcome).await.unwrap();
    assert_eq!(store.leads().await.unwrap()[0].booked_minor, 30000);
    outcome.status = "canceled".into();
    outcome.revision = 3;
    store.put_outcome(id, &outcome).await.unwrap();
    assert_eq!(store.summary().await.unwrap()[0].booked_minor, 0);
    let number = NewNumber {
        provider: Default::default(),
        label: "Search".into(),
        phone_number: "+12025550101".into(),
        destination: "+12025550102".into(),
        source: "Google Ads".into(),
        campaign: "Static number".into(),
    };
    store.add_number(&number).await.unwrap();
    let loop_number = NewNumber {
        provider: Default::default(),
        phone_number: number.destination.clone(),
        destination: "+12025550103".into(),
        ..number.clone()
    };
    assert!(matches!(
        store.add_number(&loop_number).await,
        Err(Error::Conflict)
    ));
    let sid = format!("CA{}", Uuid::new_v4().simple());
    let event = |fp: &str, kind| CallEvent {
        sid: sid.clone(),
        to: number.phone_number.clone(),
        from: "+12025550104".into(),
        fingerprint: fp.into(),
        kind,
    };
    // A final parent event can arrive before the original voice request.
    store
        .call_event(&event(
            "parent-final",
            CallEventKind::Status {
                status: "completed".into(),
                sequence: 3,
            },
        ))
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            store
                .call_event(&event("voice", CallEventKind::Voice))
                .await
                .unwrap(),
            number.destination
        );
    }
    store
        .call_event(&event(
            "parent-late",
            CallEventKind::Status {
                status: "ringing".into(),
                sequence: 1,
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .leads()
            .await
            .unwrap()
            .iter()
            .find(|l| l.channel == "call")
            .unwrap()
            .call_status
            .as_deref(),
        Some("ended_unknown")
    );
    for _ in 0..2 {
        store
            .call_event(&event(
                "dial-final",
                CallEventKind::Dial {
                    status: "completed".into(),
                    duration: Some(42),
                },
            ))
            .await
            .unwrap();
    }
    store
        .call_event(&event(
            "parent-later",
            CallEventKind::Status {
                status: "completed".into(),
                sequence: 4,
            },
        ))
        .await
        .unwrap();
    let leads = store.leads().await.unwrap();
    assert_eq!(leads.len(), 2);
    let call = leads.iter().find(|l| l.channel == "call").unwrap();
    assert_eq!(call.call_status.as_deref(), Some("destination_answered"));
    assert_eq!(call.duration_seconds, Some(42));
    assert_eq!(call.source, "Google Ads");
    assert_eq!(call.qualification, "unreviewed");
    assert!(
        other
            .call_event(&event("other", CallEventKind::Voice))
            .await
            .is_err()
    );
    let mut cleanup = store.pool.begin().await.unwrap();
    for ws in [store.workspace, other.workspace] {
        for statement in [
            "DELETE FROM audit_events WHERE workspace_id=$1",
            "DELETE FROM outcomes WHERE workspace_id=$1",
            "DELETE FROM webhook_receipts WHERE workspace_id=$1",
            "DELETE FROM inquiry_events WHERE workspace_id=$1",
            "DELETE FROM inquiries WHERE workspace_id=$1",
            "DELETE FROM leads WHERE workspace_id=$1",
            "DELETE FROM tracking_numbers WHERE workspace_id=$1",
        ] {
            sqlx::query(statement)
                .bind(ws)
                .execute(&mut *cleanup)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM workspaces WHERE id=$1")
            .bind(ws)
            .execute(&mut *cleanup)
            .await
            .unwrap();
    }
    cleanup.commit().await.unwrap();
}
