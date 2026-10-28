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
        provider: Default::default(),
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
        destination_phone: None,
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
    // A published static number may differ from the forwarding destination.
    input.destination_phone = Some(num.destination.clone());
    let static_number = NewNumber {
        phone_number: input.fallback_phone.clone(),
        label: "Published website line".into(),
        ..num.clone()
    };
    store.add_number(&static_number).await.unwrap();
    store.save_dni_pool(site, &input).await.unwrap();
    assert!(matches!(
        store.save_dni_pool(site, &input).await,
        Err(Error::Conflict)
    ));
    assert!(store.save_dni_pool(second_site, &input).await.is_err());
    // A published fallback cannot also be pooled, even for another site.
    let static_id = store
        .numbers()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.phone_number == static_number.phone_number)
        .unwrap()
        .id;
    let mut invalid = PoolInput {
        fallback_phone: num.destination.clone(),
        destination_phone: Some(num.destination.clone()),
        enabled: true,
        version: 0,
        number_ids: vec![static_id],
        dedicated_numbers: true,
    };
    assert!(store.save_dni_pool(second_site, &invalid).await.is_err());
    invalid.fallback_phone = num.phone_number.clone();
    invalid.number_ids.clear();
    invalid.enabled = false;
    assert!(store.save_dni_pool(second_site, &invalid).await.is_err());
    invalid.fallback_phone = num.destination.clone();
    invalid.destination_phone = Some(num.phone_number.clone());
    assert!(store.save_dni_pool(second_site, &invalid).await.is_err());
    assert!(
        other
            .save_dni_pool(
                other.save_site(None, &site_input).await.unwrap(),
                &PoolInput {
                    fallback_phone: static_number.phone_number.clone(),
                    destination_phone: Some(num.destination.clone()),
                    enabled: false,
                    version: 0,
                    number_ids: vec![],
                    dedicated_numbers: false
                }
            )
            .await
            .is_err()
    );
    // Number registration must respect pools configured before numbers arrive.
    store
        .save_dni_pool(
            second_site,
            &PoolInput {
                fallback_phone: "+12025550188".into(),
                destination_phone: Some("+12025550187".into()),
                enabled: false,
                version: 0,
                number_ids: vec![],
                dedicated_numbers: false,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .add_number(&NewNumber {
                phone_number: "+12025550187".into(),
                ..num.clone()
            })
            .await
            .is_err()
    );
    assert!(
        store
            .add_number(&NewNumber {
                phone_number: "+12025550188".into(),
                ..num.clone()
            })
            .await
            .is_err()
    );
    let reserved_fallback = NewNumber {
        phone_number: "+12025550188".into(),
        destination: "+12025550187".into(),
        ..num.clone()
    };
    assert!(other.add_number(&reserved_fallback).await.is_err());
    store.add_number(&reserved_fallback).await.unwrap();
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
    assert_eq!(lease["fallback_phone"], static_number.phone_number);
    assert!(lease.get("destination_phone").is_none());
    // An older client omitting the new field must not reset the saved destination.
    input.version = 1;
    input.destination_phone = None;
    store.save_dni_pool(site, &input).await.unwrap();
    assert_eq!(
        store.dni_pool(site).await.unwrap()["pool"]["destination_phone"],
        num.destination
    );
    let before_change = store.dni_pool(site).await.unwrap()["history"][0].clone();
    input.version = 2;
    input.fallback_phone = "+12025550185".into();
    store.save_dni_pool(site, &input).await.unwrap();
    let after_change = store.dni_pool(site).await.unwrap()["history"][0].clone();
    assert!(after_change["revoked_at"].is_string());
    assert_eq!(before_change["reusable_at"], after_change["reusable_at"]);
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, winner)
            .await
            .unwrap()["status"],
        "expired"
    );
    // Restore the test lease for the existing call evidence scenarios below.
    sqlx::query("UPDATE dni_assignments SET revoked_at=NULL WHERE site_id=$1")
        .bind(site)
        .execute(&store.pool)
        .await
        .unwrap();
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
    input.version = 3;
    input.enabled = false;
    store.save_dni_pool(site, &input).await.unwrap();
    assert_eq!(
        store
            .allocate_dni(site, &site_input.origin, loser)
            .await
            .unwrap()["status"],
        "disabled"
    );
    input.version = 4;
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
    // Listing calls retain static source/campaign and never invent a visitor click ID.
    let listing = NewNumber {
        phone_number: "+12025550186".into(),
        label: "LA listing".into(),
        source: "Google Business Profile".into(),
        campaign: "Los Angeles".into(),
        ..num.clone()
    };
    store.add_number(&listing).await.unwrap();
    let listing_sid = format!("CA{}", Uuid::new_v4().simple());
    assert_eq!(
        store
            .call_event(&CallEvent {
                sid: listing_sid.clone(),
                to: listing.phone_number,
                from: "+12025550183".into(),
                fingerprint: "listing-call".into(),
                kind: CallEventKind::Voice
            })
            .await
            .unwrap(),
        num.destination
    );
    let listing_lead = store
        .leads()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.source == "Google Business Profile")
        .unwrap();
    assert_eq!(listing_lead.campaign, "Los Angeles");
    assert!(store.dni_evidence(listing_lead.id).await.unwrap().is_none());
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
