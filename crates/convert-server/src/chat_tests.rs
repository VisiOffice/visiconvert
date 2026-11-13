//! POST /api/v1/chat-leads over the real HTTP boundary and a disposable PostgreSQL
//! database, through to pipeline matching and new_lead events. Nothing reaches Google.
use super::*;
use chrono::{Duration, Utc};
use google_tests::api;
use pipeline_tests::{ORIGIN, action, post, world};

async fn chat(w: &pipeline_tests::World, key: &str, body: Value) -> (StatusCode, Value) {
    api(w.a.clone(), "POST", "/api/v1/chat-leads", key, body).await
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn chat_leads_link_clicks_update_contacts_and_match_events() {
    let w = world("not_required_outside_eea", false).await;
    let s = &w.a.store;
    let key = s
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: "VisiComms".into(),
                scopes: vec![
                    "read".into(),
                    "leads:write".into(),
                    "conversions:write".into(),
                ],
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
    let now = Utc::now();
    let ok = json!({"external_id":"visicomms-chat:1","started_at":now,"landing_url":format!("{ORIGIN}/?gclid=G-chat-1")});

    // Permission: the pipeline key has no leads:write.
    let (status, _) = chat(&w, &w.key, ok.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Validation.
    let with = |k: &str, v: Value| {
        let mut b = ok.clone();
        b[k] = v;
        b
    };
    for (body, needle) in [
        (with("external_id", json!("has space")), "external_id"),
        (
            with("started_at", json!(now + Duration::hours(1))),
            "future",
        ),
        (
            with("started_at", json!(now - Duration::days(91))),
            "90 days",
        ),
        (
            with("landing_url", json!("https://elsewhere.example/?gclid=x")),
            "enabled sites",
        ),
        (
            with("landing_url", json!("ftp://pipeline.example/")),
            "http(s)",
        ),
        (with("phone", json!("12345")), "phone"),
        (with("email", json!("not-an-email")), "email"),
        (with("click", json!({"dclid":"x"})), "Unknown"),
        (with("click", json!({"gclid":"different"})), "differs"),
    ] {
        let (status, v) = chat(&w, &key, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{needle}: {v}");
        assert!(
            v["error"].as_str().unwrap().contains(needle),
            "{needle}: {v}"
        );
    }

    // 1. The tracker recorded this click: the chat links to that visit.
    let tracked = Uuid::new_v4();
    s.capture_session(
        w.site,
        ORIGIN,
        tracked,
        &json!({"source":"google_ads","consent":"granted","landing_url":format!("{ORIGIN}/"),"gclid":"G-chat-1"}),
    )
    .await
    .unwrap();
    let (status, v) = chat(&w, &key, with("name", json!("Nicole"))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["created"], true);
    assert_eq!(v["visit"], "matched");
    assert_eq!(v["click"], true);
    let first: Uuid = v["lead_id"].as_str().unwrap().parse().unwrap();
    let session: Uuid =
        sqlx::query_scalar("SELECT session_id FROM website_submissions WHERE lead_id=$1")
            .bind(first)
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(session, tracked);
    let (channel, contact): (String, String) =
        sqlx::query_as("SELECT channel,contact FROM leads WHERE id=$1")
            .bind(first)
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(
        (channel.as_str(), contact.as_str()),
        ("chat", "Not supplied")
    );

    // A quote for this visitor's phone has nothing to match yet.
    let (status, ev) = post(
        &w,
        json!({"action":"quote","external_id":"chat-quote-1","match":{"phone":"(602) 555-0144"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ev}");
    assert_eq!(ev["matched_by"], "none");

    // 2. The phone arrives later in the chat: same external_id, contact added, nothing else.
    let mut later = with("phone", json!("+1 602-555-0144"));
    later["landing_url"] = json!(format!("{ORIGIN}/other?gclid=ignored"));
    let (status, v) = chat(&w, &key, later).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["created"], false);
    assert_eq!(v["lead_id"], first.to_string());
    assert_eq!(v["click"], true);
    let (name, contact, phone): (String, String, Option<String>) =
        sqlx::query_as("SELECT name,contact,match_phone FROM leads WHERE id=$1")
            .bind(first)
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(name, "Nicole", "a repost without name keeps it");
    assert_eq!(contact, "+16025550144");
    assert_eq!(phone.as_deref(), Some("6025550144"));
    // Identical repost is a no-op.
    let (status, v) = chat(&w, &key, with("phone", json!("6025550144"))).await;
    assert_eq!(
        (status, v["created"].clone()),
        (StatusCode::OK, json!(false))
    );

    // Reposting the unmatched quote now matches the chat lead by phone.
    let (status, ev) = post(
        &w,
        json!({"action":"quote","external_id":"chat-quote-1","match":{"phone":"(602) 555-0144"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ev}");
    assert_eq!(ev["matched_by"], "phone", "{ev}");
    assert_eq!(ev["lead_id"], first.to_string());

    // 3. No tracker visit (blocked or purged), www spelling of the site, click supplied
    // separately, email only, two days ago: a visit is built from the landing URL.
    let started = now - Duration::days(2);
    let (status, v) = chat(
        &w,
        &key,
        json!({"external_id":"varda-chat:27703","started_at":started,
            "landing_url":"https://www.pipeline.example/?utm_source=bing","click":{"gclid":"G-chat-2"},
            "email":"  Pat@Example.COM "}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["visit"], "created");
    let second: Uuid = v["lead_id"].as_str().unwrap().parse().unwrap();
    let (evidence, visit_at, lead_at, email): (Value, chrono::DateTime<Utc>, chrono::DateTime<Utc>, Option<String>) =
        sqlx::query_as("SELECT s.evidence,s.created_at,l.created_at,l.match_email FROM leads l JOIN website_submissions f ON f.lead_id=l.id JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id WHERE l.id=$1")
            .bind(second).fetch_one(&s.pool).await.unwrap();
    assert_eq!(evidence["gclid"], "G-chat-2");
    assert_eq!(evidence["utm_source"], "bing");
    assert_eq!(evidence["captured_by"], "chat_api");
    assert_eq!(evidence["landing_url"], "https://www.pipeline.example/");
    assert!((visit_at - started).num_seconds().abs() < 1);
    assert!((lead_at - started).num_seconds().abs() < 1);
    assert_eq!(email.as_deref(), Some("pat@example.com"));
    // An email-only chat lead matches by email.
    let (status, ev) = post(
        &w,
        json!({"action":"order","external_id":"chat-order-2","value_minor":19109,"match":{"email":"pat@example.com"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ev}");
    assert_eq!(ev["matched_by"], "email", "{ev}");
    assert_eq!(ev["lead_id"], second.to_string());

    // 4. new_lead actions pick up chat leads with a Google click at the chat's time.
    action(
        &w.a,
        "chat_lead",
        "new_lead",
        "once_per_click",
        json!(700),
        "789",
        0,
    )
    .await;
    let (status, v) = chat(
        &w,
        &key,
        json!({"external_id":"visicomms-chat:3","started_at":Utc::now(),"landing_url":format!("{ORIGIN}/?gclid=G-chat-3")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let third = v["lead_id"].as_str().unwrap().to_string();
    s.create_lead_events().await.unwrap();
    let recorded: Vec<String> = sqlx::query_scalar(
        "SELECT external_id FROM pipeline_events WHERE workspace_id=$1 AND action_key='chat_lead'",
    )
    .bind(s.workspace)
    .fetch_all(&s.pool)
    .await
    .unwrap();
    assert!(recorded.contains(&third), "{recorded:?}");
    assert!(
        !recorded.contains(&second.to_string()),
        "older than the action"
    );

    // 5. Activity lists chats under their own kind.
    let (status, v) = api(
        w.a.clone(),
        "GET",
        "/api/v1/activity?kind=chat",
        &key,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let kinds: Vec<&str> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.len() >= 3 && kinds.iter().all(|k| *k == "chat"),
        "{kinds:?}"
    );
    assert!(
        v["data"][0]["summary"]
            .as_str()
            .unwrap()
            .starts_with("Chat from")
    );
    w.task.abort();
}

#[test]
fn phone_and_email_normalize() {
    assert_eq!(chat::phone("+1 (602) 555-0144").ok().unwrap(), "6025550144");
    assert_eq!(chat::phone("313106218419").ok().unwrap(), "3106218419");
    assert!(chat::phone("555-0144").is_err());
    assert_eq!(chat::email(" A@B.Co ").ok().unwrap(), "a@b.co");
    assert!(chat::email("a@b").is_err());
    assert!(chat::email("a b@c.d").is_err());
}
