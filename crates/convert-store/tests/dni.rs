use convert_domain::{NewNumber, OutcomeInput};
use convert_store::{CallEvent, CallEventKind, Error, Store, dni::PoolInput, sites::SiteInput};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn allocation_concurrency_cooldown_evidence_and_isolation() {
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    let mut site_input = SiteInput {
        name: "DNI test".into(),
        origin: "https://dni.example".into(),
        allowed_fields: vec![],
        enabled: true,
        version: 0,
    };
    let site = store.save_site(None, &site_input).await.unwrap();
    let second_site = store.save_site(None, &site_input).await.unwrap();
    let num = NewNumber {
        label: "DNI".into(),
        phone_number: "+12025550180".into(),
        destination: "+12025550181".into(),
        source: "Static must not leak".into(),
        campaign: "Old campaign".into(),
    };
    store.add_number(&num).await.unwrap();
    let number_id = store.numbers().await.unwrap()[0].id;
    let mut input = PoolInput {
        fallback_phone: num.destination.clone(),
        enabled: true,
        version: 0,
        number_ids: vec![number_id],
        dedicated_numbers: true,
    };
    assert!(other.save_dni_pool(site, &input).await.is_err());
    assert!(other.dni_pool(site).await.is_err());
    input.dedicated_numbers = false;
    assert!(store.save_dni_pool(site, &input).await.is_err());
    input.dedicated_numbers = true;
    input.fallback_phone = "+12025550182".into();
    assert!(store.save_dni_pool(site, &input).await.is_err());
    input.fallback_phone = num.destination.clone();
    store.save_dni_pool(site, &input).await.unwrap();
    assert!(matches!(
        store.save_dni_pool(site, &input).await,
        Err(Error::Conflict)
    ));
    assert!(store.save_dni_pool(second_site, &input).await.is_err());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    for (id, source) in [(a, "google"), (b, "bing")] {
        store.capture_session(site, &site_input.origin, id, &json!({"source":source,"utm_campaign":"DNI campaign","landing_url":"https://dni.example/quote"})).await.unwrap();
    }
    assert!(
        store
            .allocate_dni(site, "https://wrong.example", a)
            .await
            .is_err()
    );
    assert!(
        store
            .allocate_dni(site, &site_input.origin, Uuid::new_v4())
            .await
            .is_err()
    );
    let (first, second) = tokio::join!(
        store.allocate_dni(site, &site_input.origin, a),
        store.allocate_dni(site, &site_input.origin, b)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        [&first, &second]
            .iter()
            .filter(|x| x["status"] == "assigned")
            .count(),
        1
    );
    assert_eq!(
        [&first, &second]
            .iter()
            .filter(|x| x["status"] == "exhausted")
            .count(),
        1
    );
    let (winner, loser, source) = if first["status"] == "assigned" {
        (a, b, "google")
    } else {
        (b, a, "bing")
    };
    let lease = store
        .allocate_dni(site, &site_input.origin, winner)
        .await
        .unwrap();
    assert_eq!(lease, if winner == a { first } else { second });
    let sid = format!("CA{}", Uuid::new_v4().simple());
    let event = |sid: &str, fp: &str, kind| CallEvent {
        sid: sid.into(),
        to: num.phone_number.clone(),
        from: "+12025550183".into(),
        fingerprint: fp.into(),
        kind,
    };
    assert_eq!(
        store
            .call_event(&event(&sid, "voice", CallEventKind::Voice))
            .await
            .unwrap(),
        num.destination
    );
    let lead = store.leads().await.unwrap().pop().unwrap();
    assert_eq!(lead.source, source);
    assert_eq!(lead.campaign, "DNI campaign");
    let evidence = store.dni_evidence(lead.id).await.unwrap().unwrap();
    assert_eq!(evidence["status"], "inferred_window");
    assert_eq!(evidence["evidence"]["session_id"], winner.to_string());
    assert_eq!(evidence["evidence"]["number_reused"], false);
    assert!(other.dni_evidence(lead.id).await.unwrap().is_none());
    store
        .put_outcome(
            lead.id,
            &OutcomeInput {
                external_id: "dni-booking".into(),
                amount_minor: 12345,
                currency: "USD".into(),
                status: "booked".into(),
                revision: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .summary()
            .await
            .unwrap()
            .iter()
            .map(|s| s.booked_minor)
            .sum::<i64>(),
        12345
    );
    // Expired but still cooling down: no second lease and no invented source.
    sqlx::query("UPDATE dni_assignments SET assigned_at=clock_timestamp()-interval '31 minutes',expires_at=clock_timestamp()-interval '1 minute',reusable_at=clock_timestamp()+interval '29 minutes' WHERE site_id=$1").bind(site).execute(&store.pool).await.unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, loser)
            .await
            .unwrap()["status"],
        "exhausted"
    );
    let late_sid = format!("CA{}", Uuid::new_v4().simple());
    store
        .call_event(&event(&late_sid, "late", CallEventKind::Voice))
        .await
        .unwrap();
    let late = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.id != lead.id)
        .unwrap();
    assert_eq!(late.source, "Unknown / pooled call");
    assert_eq!(
        store.dni_evidence(late.id).await.unwrap().unwrap()["status"],
        "no_active_assignment"
    );
    // Once the cooldown finishes, the number can be reused; old evidence is immutable.
    sqlx::query("UPDATE dni_assignments SET assigned_at=clock_timestamp()-interval '61 minutes',expires_at=clock_timestamp()-interval '31 minutes',reusable_at=clock_timestamp()-interval '1 minute' WHERE site_id=$1").bind(site).execute(&store.pool).await.unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, loser)
            .await
            .unwrap()["status"],
        "assigned"
    );
    store
        .call_event(&event(&sid, "voice", CallEventKind::Voice))
        .await
        .unwrap();
    assert_eq!(
        store.dni_evidence(lead.id).await.unwrap().unwrap(),
        evidence
    );
    let reused_sid = format!("CA{}", Uuid::new_v4().simple());
    store
        .call_event(&event(&reused_sid, "reused", CallEventKind::Voice))
        .await
        .unwrap();
    let reused = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.id != lead.id && l.id != late.id)
        .unwrap();
    assert_eq!(
        store.dni_evidence(reused.id).await.unwrap().unwrap()["evidence"]["number_reused"],
        true
    );
    let out_sid = format!("CA{}", Uuid::new_v4().simple());
    store
        .call_event(&event(
            &out_sid,
            "status-first",
            CallEventKind::Status {
                status: "completed".into(),
                sequence: 1,
            },
        ))
        .await
        .unwrap();
    store
        .call_event(&event(&out_sid, "voice-after-status", CallEventKind::Voice))
        .await
        .unwrap();
    let unordered = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| ![lead.id, late.id, reused.id].contains(&l.id))
        .unwrap();
    assert_eq!(
        store.dni_evidence(unordered.id).await.unwrap().unwrap()["status"],
        "late_or_out_of_order"
    );
    assert_eq!(unordered.source, "Unknown / pooled call");
    // A site origin change revokes live assignments too; old-site calls cannot leak source.
    site_input.version = 1;
    site_input.origin = "https://new.example".into();
    store.save_site(Some(site), &site_input).await.unwrap();
    assert!(
        store
            .allocate_dni(site, "https://dni.example", loser)
            .await
            .is_err()
    );
    site_input.version = 2;
    site_input.origin = "https://dni.example".into();
    store.save_site(Some(site), &site_input).await.unwrap();
    input.version = 1;
    input.enabled = false;
    store.save_dni_pool(site, &input).await.unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, loser)
            .await
            .unwrap()["status"],
        "disabled"
    );
    input.version = 2;
    input.enabled = true;
    store.save_dni_pool(site, &input).await.unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, loser)
            .await
            .unwrap()["status"],
        "expired"
    );
    let fresh = Uuid::new_v4();
    store
        .capture_session(
            site,
            &site_input.origin,
            fresh,
            &json!({"source":"fresh","landing_url":"https://dni.example/"}),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, fresh)
            .await
            .unwrap()["status"],
        "exhausted"
    );
    let mut tx = store.pool.begin().await.unwrap();
    for ws in [store.workspace, other.workspace] {
        for sql in [
            "DELETE FROM dni_call_evidence WHERE workspace_id=$1",
            "DELETE FROM dni_assignments WHERE workspace_id=$1",
            "DELETE FROM dni_pool_numbers WHERE workspace_id=$1",
            "DELETE FROM dni_pools WHERE workspace_id=$1",
            "DELETE FROM website_sessions WHERE site_id IN (SELECT id FROM sites WHERE workspace_id=$1)",
            "DELETE FROM sites WHERE workspace_id=$1",
            "DELETE FROM outcomes WHERE workspace_id=$1",
            "DELETE FROM audit_events WHERE workspace_id=$1",
            "DELETE FROM webhook_receipts WHERE workspace_id=$1",
            "DELETE FROM inquiry_events WHERE workspace_id=$1",
            "DELETE FROM inquiries WHERE workspace_id=$1",
            "DELETE FROM leads WHERE workspace_id=$1",
            "DELETE FROM tracking_numbers WHERE workspace_id=$1",
            "DELETE FROM workspaces WHERE id=$1",
        ] {
            sqlx::query(sql).bind(ws).execute(&mut *tx).await.unwrap();
        }
    }
    tx.commit().await.unwrap();
}
