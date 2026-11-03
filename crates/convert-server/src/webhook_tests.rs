//! Webhooks end to end: triggers, signing, retries, disabling and scopes, against a
//! disposable PostgreSQL and a local receiver.
use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use std::sync::{
    Mutex,
    atomic::{AtomicU16, Ordering},
};
use tower::ServiceExt;

#[derive(Default)]
struct Receiver {
    status: AtomicU16,
    requests: Mutex<Vec<(HeaderMap, String)>>,
}
async fn receive(State(r): State<Arc<Receiver>>, headers: HeaderMap, body: String) -> StatusCode {
    r.requests.lock().unwrap().push((headers, body));
    match r.status.load(Ordering::SeqCst) {
        0 => StatusCode::NO_CONTENT,
        code => StatusCode::from_u16(code).unwrap(),
    }
}
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
fn app(store: Store) -> Shared {
    hosted_app(store, None)
}
fn hosted_app(store: Store, cloud: Option<hosted::Config>) -> Shared {
    Arc::new(App {
        store,
        api_key: "webhook-test-operator-at-least-32-chars".into(),
        public_url: "https://api.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud,
        vault: Some(secrets::Vault::from_hex(&"c".repeat(64)).unwrap()),
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    })
}
/// Drain everything currently due.
async fn drain(a: &App) {
    while webhooks::run_batch(a).await.unwrap() {}
}
fn verify(headers: &HeaderMap, body: &str, secret: &str) -> bool {
    let id = headers["webhook-id"].to_str().unwrap();
    let ts: i64 = headers["webhook-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let expected = webhooks::sign(secret, id, ts, body).unwrap();
    headers["webhook-signature"]
        .to_str()
        .unwrap()
        .split(' ')
        .any(|s| s == expected)
}
async fn state(store: &Store, delivery: Uuid) -> (String, i32) {
    let r = sqlx::query("SELECT state,attempts FROM webhook_deliveries WHERE id=$1")
        .bind(delivery)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    use sqlx::Row;
    (r.get("state"), r.get("attempts"))
}
async fn due_now(store: &Store) {
    sqlx::query(
        "UPDATE webhook_deliveries SET due_at=now() WHERE workspace_id=$1 AND state='retry'",
    )
    .bind(store.workspace)
    .execute(&store.pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn events_are_signed_retried_disabled_and_scoped() {
    webhooks::ALLOW_PRIVATE.store(true, Ordering::SeqCst);
    let receiver = Arc::new(Receiver::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let r = receiver.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/hook", post(receive)).with_state(r),
        )
        .await
        .unwrap()
    });
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let a = app(store.clone());
    let op = a.api_key.clone();

    // Nothing is recorded before an endpoint exists.
    store
        .add_lead(&NewLead {
            external_id: "before".into(),
            name: "Before".into(),
            contact: "before@example.com".into(),
            source: "Google".into(),
            campaign: "".into(),
            notes: "".into(),
        })
        .await
        .unwrap();
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_events WHERE workspace_id=$1")
            .bind(store.workspace)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(events, 0);

    // Scopes: read-only keys can list but not create; webhooks:write can create.
    let (s, read) = call(
        a.clone(),
        "POST",
        "/api/api-keys",
        &op,
        json!({"name":"Read","scopes":["read"],"expires_at":null}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{read}");
    let read = read["secret"].as_str().unwrap().to_owned();
    let (s, writer) = call(
        a.clone(),
        "POST",
        "/api/api-keys",
        &op,
        json!({"name":"Hooks","scopes":["read","webhooks:write"],"expires_at":null}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{writer}");
    let writer = writer["secret"].as_str().unwrap().to_owned();
    let all = json!([
        "interaction.created",
        "call.completed",
        "inquiry.reviewed",
        "booking.recorded",
        "booking.revised",
        "booking.canceled"
    ]);
    let (s, _) = call(
        a.clone(),
        "POST",
        "/api/v1/webhooks",
        &read,
        json!({"url":url,"event_types":all}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        a.clone(),
        "POST",
        "/api/v1/webhooks",
        &writer,
        json!({"url":url,"event_types":["lead.exploded"]}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, endpoint) = call(
        a.clone(),
        "POST",
        "/api/v1/webhooks",
        &writer,
        json!({"url":url,"description":"CRM","event_types":all}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{endpoint}");
    let secret = endpoint["secret"].as_str().unwrap().to_owned();
    let id = endpoint["id"].as_str().unwrap().to_owned();
    let (s, listed) = call(a.clone(), "GET", "/api/v1/webhooks", &read, json!({})).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        !listed.to_string().contains(&secret),
        "secrets are shown once"
    );
    assert!(!listed.to_string().contains("encrypted"));
    assert_eq!(listed["history"][0]["kind"], "created");

    // A form lead, its review and a booking lifecycle each emit one event.
    let lead = store
        .add_lead(&NewLead {
            external_id: "quote-1".into(),
            name: "Pat Lee".into(),
            contact: "pat@example.com".into(),
            source: "Google".into(),
            campaign: "Spring".into(),
            notes: "".into(),
        })
        .await
        .unwrap();
    store
        .update_inquiry(
            lead,
            &LeadUpdate {
                qualification: Qualification::Qualified,
                notes: "Good fit".into(),
                version: 1,
            },
            "test",
        )
        .await
        .unwrap();
    for (revision, amount, status) in [
        (1, 50000, "booked"),
        (2, 65000, "booked"),
        (3, 65000, "canceled"),
    ] {
        store
            .put_outcome(
                lead,
                &OutcomeInput {
                    external_id: "job-1".into(),
                    amount_minor: amount,
                    currency: "USD".into(),
                    status: status.into(),
                    revision,
                },
            )
            .await
            .unwrap();
    }
    store.seed_demo().await.unwrap();
    drain(&a).await;
    let requests = receiver.requests.lock().unwrap().clone();
    let types: Vec<String> = requests
        .iter()
        .map(|(_, b)| {
            serde_json::from_str::<Value>(b).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let mut sorted = types.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "booking.canceled",
            "booking.recorded",
            "booking.revised",
            "inquiry.reviewed",
            "interaction.created"
        ],
        "demo data emits nothing"
    );
    for (headers, body) in &requests {
        assert!(verify(headers, body, &secret));
        assert_eq!(headers["content-type"], "application/json");
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            headers["webhook-id"].to_str().unwrap(),
            v["id"].as_str().unwrap()
        );
        assert_eq!(v["workspace_id"], json!(store.workspace));
    }
    let created: Value = serde_json::from_str(
        &requests[types
            .iter()
            .position(|t| t == "interaction.created")
            .unwrap()]
        .1,
    )
    .unwrap();
    assert_eq!(created["data"]["object"], "interaction");
    assert_eq!(created["data"]["contact"], "pat@example.com");
    assert_eq!(created["data"]["fields"]["campaign"], "Spring");
    let revised: Value = serde_json::from_str(
        &requests[types.iter().position(|t| t == "booking.revised").unwrap()].1,
    )
    .unwrap();
    assert_eq!(revised["resource_version"], 2);
    assert_eq!(revised["data"]["external_id"], "job-1");
    receiver.requests.lock().unwrap().clear();

    // A failed attempt is retried later with identical bytes and the same event id.
    receiver.status.store(503, Ordering::SeqCst);
    let (s, test) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{test}");
    let delivery: Uuid = test["delivery_id"].as_str().unwrap().parse().unwrap();
    drain(&a).await;
    assert_eq!(state(&store, delivery).await, ("retry".into(), 1));
    drain(&a).await;
    assert_eq!(receiver.requests.lock().unwrap().len(), 1, "not due yet");
    receiver.status.store(0, Ordering::SeqCst);
    due_now(&store).await;
    drain(&a).await;
    assert_eq!(state(&store, delivery).await, ("succeeded".into(), 2));
    {
        let requests = receiver.requests.lock().unwrap();
        assert_eq!(requests[0].1, requests[1].1);
        assert_eq!(requests[0].0["webhook-id"], requests[1].0["webhook-id"]);
        assert_eq!(
            serde_json::from_str::<Value>(&requests[0].1).unwrap()["type"],
            "webhook.test"
        );
    }

    // Client errors fail without retry; a finished delivery can be redelivered.
    receiver.status.store(400, Ordering::SeqCst);
    let (_, test) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    let delivery: Uuid = test["delivery_id"].as_str().unwrap().parse().unwrap();
    drain(&a).await;
    assert_eq!(state(&store, delivery).await, ("failed".into(), 1));
    let (s, page) = call(
        a.clone(),
        "GET",
        &format!("/api/v1/webhooks/{id}/deliveries?limit=1"),
        &read,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page["data"][0]["last_status"], 400);
    assert!(page["next_cursor"].is_string());
    receiver.status.store(0, Ordering::SeqCst);
    let (s, _) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhook-deliveries/{delivery}/redeliver"),
        &read,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhook-deliveries/{delivery}/redeliver"),
        &writer,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    drain(&a).await;
    assert_eq!(state(&store, delivery).await.0, "succeeded");

    // Rotation signs with both secrets for the overlap period.
    let version = call(
        a.clone(),
        "GET",
        &format!("/api/v1/webhooks/{id}"),
        &read,
        json!({}),
    )
    .await
    .1["version"]
        .clone();
    let (s, rotated) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/rotate-secret"),
        &writer,
        json!({"version":version}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{rotated}");
    let new_secret = rotated["secret"].as_str().unwrap().to_owned();
    assert_ne!(new_secret, secret);
    receiver.requests.lock().unwrap().clear();
    call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    drain(&a).await;
    {
        let requests = receiver.requests.lock().unwrap();
        let (headers, body) = &requests[0];
        assert!(verify(headers, body, &secret) && verify(headers, body, &new_secret));
        assert_eq!(
            headers["webhook-signature"]
                .to_str()
                .unwrap()
                .split(' ')
                .count(),
            2
        );
    }

    // Without contact details, personal fields are left out.
    let (s, updated) = call(
        a.clone(),
        "PATCH",
        &format!("/api/v1/webhooks/{id}"),
        &writer,
        json!({"url":url,"event_types":all,"include_contact":false,"version":rotated["version"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{updated}");
    let (s, _) = call(
        a.clone(),
        "PATCH",
        &format!("/api/v1/webhooks/{id}"),
        &writer,
        json!({"url":url,"event_types":all,"version":rotated["version"]}),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "stale versions are rejected");
    receiver.requests.lock().unwrap().clear();
    store
        .add_lead(&NewLead {
            external_id: "quote-2".into(),
            name: "Sam Roe".into(),
            contact: "+12025550123".into(),
            source: "Bing".into(),
            campaign: "".into(),
            notes: "".into(),
        })
        .await
        .unwrap();
    drain(&a).await;
    {
        let requests = receiver.requests.lock().unwrap();
        let body = &requests[0].1;
        assert!(
            !body.contains("Sam Roe") && !body.contains("+12025550123"),
            "{body}"
        );
        assert!(body.contains("Bing"));
    }

    // Send-time checks still apply if private destinations are later disallowed.
    webhooks::ALLOW_PRIVATE.store(false, Ordering::SeqCst);
    let (_, test) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    drain(&a).await;
    assert_eq!(
        state(
            &store,
            test["delivery_id"].as_str().unwrap().parse().unwrap()
        )
        .await
        .0,
        "failed"
    );
    webhooks::ALLOW_PRIVATE.store(true, Ordering::SeqCst);

    // An interrupted attempt is recovered and retried.
    let (_, test) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    let delivery: Uuid = test["delivery_id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE webhook_deliveries SET state='sending',attempts=1,due_at=now()-interval '1 second' WHERE id=$1")
        .bind(delivery)
        .execute(&store.pool)
        .await
        .unwrap();
    drain(&a).await;
    assert_eq!(state(&store, delivery).await, ("succeeded".into(), 2));

    // Five days of failures disable the endpoint and cancel queued work.
    sqlx::query(
        "UPDATE webhook_endpoints SET failing_since=now()-interval '6 days' WHERE id=$1::uuid",
    )
    .bind(&id)
    .execute(&store.pool)
    .await
    .unwrap();
    receiver.status.store(500, Ordering::SeqCst);
    call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    drain(&a).await;
    let (_, shown) = call(
        a.clone(),
        "GET",
        &format!("/api/v1/webhooks/{id}"),
        &read,
        json!({}),
    )
    .await;
    assert_eq!(shown["state"], "disabled");
    assert_eq!(shown["disabled_reason"], "Every delivery failed for 5 days");
    let (s, _) = call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Re-enabling resets the failure clock; 410 Gone disables immediately.
    let (s, enabled) = call(
        a.clone(),
        "PATCH",
        &format!("/api/v1/webhooks/{id}"),
        &writer,
        json!({"url":url,"event_types":all,"state":"enabled","version":shown["version"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{enabled}");
    assert!(enabled["failing_since"].is_null());
    receiver.status.store(410, Ordering::SeqCst);
    call(
        a.clone(),
        "POST",
        &format!("/api/v1/webhooks/{id}/test"),
        &writer,
        json!({}),
    )
    .await;
    drain(&a).await;
    let (_, shown) = call(
        a.clone(),
        "GET",
        &format!("/api/v1/webhooks/{id}"),
        &read,
        json!({}),
    )
    .await;
    assert_eq!(shown["state"], "disabled");

    // Other workspaces cannot see or change this endpoint.
    let other = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    other.migrate().await.unwrap();
    let b = app(other.clone());
    let (_, other_key) = call(
        b.clone(),
        "POST",
        "/api/api-keys",
        &b.api_key,
        json!({"name":"Other","scopes":["read","webhooks:write"],"expires_at":null}),
    )
    .await;
    let other_key = other_key["secret"].as_str().unwrap().to_owned();
    let (s, _) = call(
        b.clone(),
        "GET",
        &format!("/api/v1/webhooks/{id}"),
        &other_key,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = call(
        b.clone(),
        "DELETE",
        &format!("/api/v1/webhooks/{id}"),
        &other_key,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Deleting removes the endpoint and its deliveries but keeps the history.
    let (s, _) = call(
        a.clone(),
        "DELETE",
        &format!("/api/v1/webhooks/{id}"),
        &writer,
        json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, listed) = call(a.clone(), "GET", "/api/v1/webhooks", &read, json!({})).await;
    assert_eq!(listed["endpoints"], json!([]));
    assert_eq!(listed["history"][0]["kind"], "deleted");
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn endpoint_limits_and_call_completion() {
    webhooks::ALLOW_PRIVATE.store(true, Ordering::SeqCst);
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let a = app(store.clone());
    let op = a.api_key.clone();
    for n in 0..10 {
        let (s, v) = call(
            a.clone(),
            "POST",
            "/api/webhooks",
            &op,
            json!({"url":format!("http://127.0.0.1:9/{n}"),"event_types":["call.completed"]}),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let (s, _) = call(
        a.clone(),
        "POST",
        "/api/webhooks",
        &op,
        json!({"url":"http://127.0.0.1:9/x","event_types":["call.completed"]}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // call.completed fires once, when the call first becomes terminal.
    let lead = Uuid::new_v4();
    sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign) VALUES($1,$2,'CA1','Phone inquiry','+12025550100','call','Direct','')")
        .bind(lead)
        .bind(store.workspace)
        .execute(&store.pool)
        .await
        .unwrap();
    for _ in 0..2 {
        sqlx::query("UPDATE leads SET dial_terminal=true,call_status='completed' WHERE id=$1")
            .bind(lead)
            .execute(&store.pool)
            .await
            .unwrap();
    }
    let completed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE workspace_id=$1 AND type='call.completed'",
    )
    .bind(store.workspace)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(completed, 1);
    let fanned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE workspace_id=$1")
            .bind(store.workspace)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(fanned, 10, "one delivery per subscribed endpoint");
    // Keep the shared worker from spending time on unreachable test endpoints.
    sqlx::query("UPDATE webhook_endpoints SET state='paused' WHERE workspace_id=$1")
        .bind(store.workspace)
        .execute(&store.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn hosted_delivery_is_held_while_billing_is_inactive() {
    webhooks::ALLOW_PRIVATE.store(true, Ordering::SeqCst);
    let receiver = Arc::new(Receiver::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let r = receiver.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/hook", post(receive)).with_state(r),
        )
        .await
        .unwrap()
    });
    // Rails stand-in: answers the workspace-only delivery question with a settable status.
    let billing = Arc::new(AtomicU16::new(403));
    let asked = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (b, q) = (billing.clone(), asked.clone());
    let cloud_router = Router::new().route(
        "/internal/api-access",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let (b, q) = (b.clone(), q.clone());
            async move {
                assert_eq!(
                    headers["authorization"].to_str().unwrap(),
                    format!("Bearer {}", "a".repeat(64))
                );
                q.lock().unwrap().push(body);
                StatusCode::from_u16(b.load(Ordering::SeqCst)).unwrap()
            }
        }),
    );
    let cloud_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cloud_origin = format!("http://{}", cloud_listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(cloud_listener, cloud_router).await.unwrap() });

    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let operator = app(store.clone());
    let (s, endpoint) = call(
        operator.clone(),
        "POST",
        "/api/webhooks",
        &operator.api_key.clone(),
        json!({"url":url,"event_types":["booking.recorded"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{endpoint}");
    let endpoint: Uuid = endpoint["id"].as_str().unwrap().parse().unwrap();
    let hosted = hosted_app(
        store.clone(),
        Some(hosted::Config {
            service_key: "a".repeat(64),
            cloud_origin,
            app_origin: None,
        }),
    );
    let last = |delivery: Uuid| {
        let pool = store.pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT last_error FROM webhook_deliveries WHERE id=$1",
            )
            .bind(delivery)
            .fetch_one(&pool)
            .await
            .unwrap()
            .unwrap_or_default()
        }
    };

    // Billing inactive: held, no attempt used, endpoint not marked failing, nothing sent.
    webhooks::forget_billing();
    let delivery = store.queue_webhook_test(endpoint).await.unwrap();
    drain(&hosted).await;
    assert_eq!(state(&store, delivery).await, ("retry".into(), 0));
    assert!(last(delivery).await.contains("billing is inactive"));
    assert!(receiver.requests.lock().unwrap().is_empty());
    let failing: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT failing_since FROM webhook_endpoints WHERE id=$1")
            .bind(endpoint)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(failing.is_none());
    let held_until: bool = sqlx::query_scalar(
        "SELECT due_at > now() + interval '14 minutes' FROM webhook_deliveries WHERE id=$1",
    )
    .bind(delivery)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert!(held_until);
    let question = asked.lock().unwrap()[0].clone();
    assert_eq!(question["purpose"], "webhook_delivery");
    assert_eq!(question["workspace_id"], store.workspace.to_string());
    assert!(question.get("user_id").is_none());

    // Rails unreachable or erroring: also held (fail closed), and not cached.
    billing.store(500, Ordering::SeqCst);
    webhooks::forget_billing();
    due_now(&store).await;
    drain(&hosted).await;
    assert_eq!(state(&store, delivery).await, ("retry".into(), 0));
    assert!(last(delivery).await.contains("could not be checked"));

    // Billing restored: the held delivery goes out on its next turn.
    billing.store(200, Ordering::SeqCst);
    due_now(&store).await;
    drain(&hosted).await;
    assert_eq!(state(&store, delivery).await, ("succeeded".into(), 1));
    assert_eq!(receiver.requests.lock().unwrap().len(), 1);

    // The answer is cached: a lapse is noticed within the cache window, not per delivery.
    let before = asked.lock().unwrap().len();
    let second = store.queue_webhook_test(endpoint).await.unwrap();
    drain(&hosted).await;
    assert_eq!(state(&store, second).await.0, "succeeded");
    assert_eq!(asked.lock().unwrap().len(), before);

    // A lapse longer than retention drops the old event instead of sending a backlog.
    billing.store(403, Ordering::SeqCst);
    webhooks::forget_billing();
    let stale = store.queue_webhook_test(endpoint).await.unwrap();
    sqlx::query("UPDATE webhook_events SET occurred_at=now()-interval '31 days' WHERE id=(SELECT event_id FROM webhook_deliveries WHERE id=$1)")
        .bind(stale)
        .execute(&store.pool)
        .await
        .unwrap();
    drain(&hosted).await;
    assert_eq!(state(&store, stale).await, ("canceled".into(), 0));
    assert!(last(stale).await.contains("not sent after 30 days"));
    assert_eq!(receiver.requests.lock().unwrap().len(), 2);
    webhooks::forget_billing();
}
