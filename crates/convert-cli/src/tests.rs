//! Exercises the CLI operations and the MCP server against an in-memory stand-in for the API,
//! which mirrors the server's version and revision rules.
use crate::{client::Client, mcp, ops};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use chrono::{DateTime, Utc};
use convert_domain::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct Data {
    leads: Vec<Lead>,
    outcomes: Vec<(Uuid, Outcome)>,
    numbers: Vec<TrackingNumber>,
}

type Shared = Arc<Mutex<Data>>;
type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message})))
}

fn authorized(headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    match headers.get("authorization").and_then(|h| h.to_str().ok()) {
        Some("Bearer test-key") => Ok(()),
        _ => Err(fail(StatusCode::UNAUTHORIZED, "Unauthorized")),
    }
}

fn lead(id: &str, name: &str, minutes_ago: i64) -> Lead {
    let now: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
    Lead {
        inquiry_id: None,
        id: id.parse().unwrap(),
        name: name.into(),
        contact: "+15125550190".into(),
        channel: "call".into(),
        source: "Google Ads".into(),
        campaign: "Spring HVAC".into(),
        qualification: "unreviewed".into(),
        notes: "Asked about a new furnace".into(),
        version: 3,
        created_at: now - chrono::Duration::minutes(minutes_ago),
        is_demo: false,
        call_status: Some("completed".into()),
        duration_seconds: Some(184),
        tracking_number: Some("+15125550142".into()),
        destination: Some("+15125550100".into()),
        booked_minor: 0,
    }
}

async fn mock() -> (Client, Shared) {
    let data: Shared = Arc::default();
    {
        let mut d = data.lock().unwrap();
        d.leads.push(lead(
            "aaaaaaaa-1111-4000-8000-000000000001",
            "Dana Ruiz",
            30,
        ));
        d.leads.push(lead(
            "aaaaaaaa-2222-4000-8000-000000000002",
            "Sam Okafor",
            5,
        ));
        d.leads.push(lead(
            "bbbbbbbb-3333-4000-8000-000000000003",
            "Lee Park",
            60 * 24 * 10,
        ));
    }
    let app = Router::new()
        .route("/api/connections/signalwire", get(|| async { Json(json!({"connected":true,"voice_url":"https://convert.test/webhooks/signalwire/workspaces/w/voice","status_url":"https://convert.test/webhooks/signalwire/workspaces/w/status"})) }))
        .route("/api/connections/signalwire/numbers", get(|| async { Json(json!({"has_more":false,"numbers":[{"sid":"fixture-id","phone_number":"+12025550311","friendly_name":"SignalWire fixture"}]})) }))
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route(
            "/api/bootstrap",
            get(|headers: HeaderMap| async move {
                authorized(&headers)?;
                Ok::<_, (StatusCode, Json<Value>)>(Json(json!({"name": "Test HVAC", "hosted": false})))
            }),
        )
        .route(
            "/api/connections/twilio",
            get(|headers: HeaderMap| async move {
                authorized(&headers)?;
                Ok::<_, (StatusCode, Json<Value>)>(Json(json!({
                    "mode": "workspace", "connected": true, "account_sid": "AC00000000000000000000000000000000",
                    "revision": 1, "voice_url": "https://convert.test/webhooks/twilio/workspaces/w/voice",
                    "status_url": "https://convert.test/webhooks/twilio/workspaces/w/status"
                })))
            }),
        )
        .route(
            "/api/connections/twilio/numbers",
            get(|headers: HeaderMap| async move {
                authorized(&headers)?;
                Ok::<_, (StatusCode, Json<Value>)>(Json(json!({"has_more": false, "numbers": [
                    {"sid": "PN1", "phone_number": "+15125550142", "friendly_name": "Main ad line"},
                    {"sid": "PN2", "phone_number": "+15125550143", "friendly_name": "(512) 555-0143"}
                ]})))
            }),
        )
        .route("/api/numbers", get(list_numbers).post(add_number))
        .route("/api/leads", get(list_leads))
        .route("/api/leads/{id}", axum::routing::patch(update_lead))
        .route("/api/leads/{id}/outcomes", get(list_outcomes).put(put_outcome))
        .with_state(data.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        Client::new(&format!("http://{address}"), "test-key").unwrap(),
        data,
    )
}

async fn list_numbers(State(data): State<Shared>, headers: HeaderMap) -> Reply {
    authorized(&headers)?;
    Ok(Json(json!(data.lock().unwrap().numbers)))
}

async fn add_number(
    State(data): State<Shared>,
    headers: HeaderMap,
    Json(n): Json<NewNumber>,
) -> Reply {
    authorized(&headers)?;
    n.validate()
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e.0))?;
    let mut d = data.lock().unwrap();
    if d.numbers.iter().any(|x| x.phone_number == n.phone_number) {
        return Err(fail(StatusCode::CONFLICT, "Already tracked"));
    }
    d.numbers.push(TrackingNumber {
        provider: n.provider,
        id: Uuid::new_v4(),
        label: n.label,
        phone_number: n.phone_number,
        destination: n.destination,
        source: n.source,
        campaign: n.campaign,
        is_demo: false,
    });
    Ok(Json(json!({"ok": true})))
}

async fn list_leads(State(data): State<Shared>, headers: HeaderMap) -> Reply {
    authorized(&headers)?;
    Ok(Json(json!(data.lock().unwrap().leads)))
}

async fn update_lead(
    State(data): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(u): Json<LeadUpdate>,
) -> Reply {
    authorized(&headers)?;
    let mut d = data.lock().unwrap();
    let lead = d
        .leads
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "Not found"))?;
    if lead.version != u.version {
        return Err(fail(StatusCode::CONFLICT, "Conflict"));
    }
    lead.qualification = u.qualification.as_str().into();
    lead.notes = u.notes;
    lead.version += 1;
    Ok(Json(json!({"ok": true})))
}

async fn list_outcomes(
    State(data): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Reply {
    authorized(&headers)?;
    let d = data.lock().unwrap();
    Ok(Json(json!(
        d.outcomes
            .iter()
            .filter(|(l, _)| *l == id)
            .map(|(_, o)| o)
            .collect::<Vec<_>>()
    )))
}

async fn put_outcome(
    State(data): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(n): Json<OutcomeInput>,
) -> Reply {
    authorized(&headers)?;
    n.validate()
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e.0))?;
    let mut d = data.lock().unwrap();
    match d
        .outcomes
        .iter()
        .position(|(_, o)| o.external_id == n.external_id)
    {
        Some(i) => {
            let (lead, previous) = &d.outcomes[i];
            if *lead != id || n.revision != previous.revision + 1 {
                return Err(fail(StatusCode::CONFLICT, "Conflict"));
            }
            d.outcomes.remove(i);
        }
        None if n.revision != 1 => return Err(fail(StatusCode::CONFLICT, "Conflict")),
        None => {}
    }
    d.outcomes.push((
        id,
        Outcome {
            external_id: n.external_id,
            amount_minor: n.amount_minor,
            currency: n.currency,
            status: n.status,
            revision: n.revision,
        },
    ));
    Ok(Json(json!({"ok": true})))
}

#[tokio::test]
async fn qualify_uses_the_current_version_and_keeps_notes() {
    let (client, data) = mock().await;
    let result = ops::qualify(&client, "aaaaaaaa-2222", Qualification::Qualified, None)
        .await
        .unwrap();
    assert_eq!(result["qualification"], "qualified");
    let d = data.lock().unwrap();
    let lead = d.leads.iter().find(|l| l.name == "Sam Okafor").unwrap();
    assert_eq!(
        (lead.qualification.as_str(), lead.version),
        ("qualified", 4)
    );
    assert_eq!(lead.notes, "Asked about a new furnace");
}

#[tokio::test]
async fn short_ids_must_be_unique() {
    let (client, _) = mock().await;
    let error = ops::find_lead(&client, "aaaaaaaa")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("2 leads start with"), "{error}");
    assert!(ops::find_lead(&client, "bbbbbbbb").await.is_ok());
    assert!(ops::find_lead(&client, "abc").await.is_err());
}

#[tokio::test]
async fn bookings_revise_by_reference() {
    let (client, data) = mock().await;
    let lead = "bbbbbbbb-3333-4000-8000-000000000003";
    let first = ops::record_outcome(
        &client,
        lead,
        "JOB-1042",
        Some(125_000),
        ops::OutcomeStatus::Booked,
    )
    .await
    .unwrap();
    assert_eq!(
        (first["revision"].as_i64(), first["amount"].as_str()),
        (Some(1), Some("$1,250.00"))
    );
    let raised = ops::record_outcome(
        &client,
        lead,
        "JOB-1042",
        Some(140_000),
        ops::OutcomeStatus::Booked,
    )
    .await
    .unwrap();
    assert_eq!(raised["revision"], 2);
    let canceled = ops::record_outcome(
        &client,
        lead,
        "JOB-1042",
        None,
        ops::OutcomeStatus::Canceled,
    )
    .await
    .unwrap();
    assert_eq!(
        (canceled["status"].as_str(), canceled["amount"].as_str()),
        (Some("canceled"), Some("$1,400.00"))
    );
    assert_eq!(data.lock().unwrap().outcomes.len(), 1);
    let missing = ops::record_outcome(&client, lead, "JOB-9", None, ops::OutcomeStatus::Booked)
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("booked amount"));
    let other_lead = ops::record_outcome(
        &client,
        "aaaaaaaa-1111",
        "JOB-1042",
        Some(100),
        ops::OutcomeStatus::Booked,
    )
    .await;
    assert!(
        other_lead
            .unwrap_err()
            .to_string()
            .contains("different lead")
    );
}

#[tokio::test]
async fn adding_numbers_reports_the_manual_twilio_step() {
    let (client, _) = mock().await;
    let before = ops::available_numbers(&client).await.unwrap();
    assert_eq!(before["untracked"].as_array().unwrap().len(), 2);
    let added = ops::add_number(
        &client,
        ops::AddNumber {
            provider: Default::default(),
            phone_number: "(512) 555-0142".into(),
            forward_to: "512.555.0100".into(),
            source: "Google Ads".into(),
            campaign: "Spring HVAC".into(),
            label: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(added["tracking_number"], "+15125550142");
    assert_eq!(added["label"], "Spring HVAC");
    assert!(added["voice_url"].as_str().unwrap().ends_with("/voice"));
    let after = ops::available_numbers(&client).await.unwrap();
    assert_eq!(after["untracked"][0]["phone_number"], "+15125550143");
}

#[tokio::test]
async fn lead_filters_and_bad_keys() {
    let (client, _) = mock().await;
    let now: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
    let filter = ops::LeadFilter {
        since: Some(ops::parse_since("1d", now).unwrap()),
        ..Default::default()
    };
    let leads = ops::list_leads(&client, &filter).await.unwrap();
    assert_eq!(
        leads.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["Sam Okafor", "Dana Ruiz"]
    );
    let wrong = Client::new(client.base(), "wrong").unwrap();
    let error = ops::status(&wrong).await.unwrap_err().to_string();
    assert!(error.contains("visiconvert login"), "{error}");
}

fn rpc(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[tokio::test]
async fn mcp_is_read_only_unless_allowed() {
    let (client, data) = mock().await;
    let server = mcp::Server::new(Ok(client.clone()), false);
    let init = server
        .handle(rpc(
            1,
            "initialize",
            json!({"protocolVersion": "2025-03-26"}),
        ))
        .await
        .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert!(
        server
            .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await
            .is_none()
    );
    let tools = server
        .handle(rpc(2, "tools/list", json!({})))
        .await
        .unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"list_leads") && !names.contains(&"qualify_lead"));
    let blocked = server
        .handle(rpc(3, "tools/call", json!({"name": "qualify_lead", "arguments": {"lead_id": "bbbbbbbb", "qualification": "spam"}})))
        .await
        .unwrap();
    assert_eq!(blocked["result"]["isError"], true);
    assert!(
        blocked["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("--allow-changes")
    );
    assert_eq!(data.lock().unwrap().leads[2].qualification, "unreviewed");

    let writer = mcp::Server::new(Ok(client), true);
    let done = writer
        .handle(rpc(4, "tools/call", json!({"name": "qualify_lead", "arguments": {"lead_id": "bbbbbbbb", "qualification": "spam"}})))
        .await
        .unwrap();
    assert_eq!(done["result"]["isError"], false, "{done}");
    assert_eq!(data.lock().unwrap().leads[2].qualification, "spam");
    let listed = writer
        .handle(rpc(
            5,
            "tools/call",
            json!({"name": "list_leads", "arguments": {"limit": 1}}),
        ))
        .await
        .unwrap();
    let body: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["count"], 1);
    let unknown = writer
        .handle(rpc(6, "resources/list", json!({})))
        .await
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    let prompt = writer
        .handle(rpc(
            7,
            "prompts/get",
            json!({"name": "set_up_call_tracking"}),
        ))
        .await
        .unwrap();
    assert!(
        prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("Never ask them to paste credentials into this conversation")
    );
}

#[tokio::test]
async fn mcp_explains_a_missing_login() {
    let server = mcp::Server::new(Err("Not logged in. Run `visiconvert login`".into()), false);
    let reply = server
        .handle(rpc(1, "tools/call", json!({"name": "get_status"})))
        .await
        .unwrap();
    assert_eq!(reply["result"]["isError"], true);
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("visiconvert login")
    );
}

#[test]
fn docs_reference_pages_are_current() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs-site/src/content/docs/reference");
    for (page, name) in [(crate::Page::Cli, "cli"), (crate::Page::Mcp, "mcp")] {
        let saved = std::fs::read_to_string(root.join(format!("{name}.mdx"))).unwrap();
        assert!(
            saved == crate::reference_page(page),
            "docs-site/src/content/docs/reference/{name}.mdx is stale; run `npm run reference` in docs-site"
        );
    }
}

#[test]
fn client_enforces_secure_origins_for_all_credentials() {
    for url in [
        "http://localhost.evil.example",
        "http://127.0.0.1.example",
        "http://example.com",
        "https://user:password@example.com",
        "https://example.com/path",
        "https://example.com?key=x",
        "https://example.com#x",
    ] {
        assert!(Client::new(url, "test-key").is_err(), "{url}");
    }
    for url in [
        "https://example.com",
        "http://localhost:3100",
        "http://127.0.0.1:3100",
        "http://[::1]:3100",
    ] {
        assert!(Client::new(url, "test-key").is_ok(), "{url}");
    }
}

#[tokio::test]
async fn signalwire_cli_and_mcp_select_provider_and_report_callbacks() {
    let (client, data) = mock().await;
    let status = ops::status(&client).await.unwrap();
    assert_eq!(status.signalwire["connected"], true);
    let reader = mcp::Server::new(Ok(client.clone()), false);
    let listed = reader
        .handle(rpc(
            1,
            "tools/call",
            json!({"name":"list_signalwire_numbers","arguments":{}}),
        ))
        .await
        .unwrap();
    assert_eq!(listed["result"]["isError"], false);
    assert!(listed.to_string().contains("+12025550311"));
    let writer = mcp::Server::new(Ok(client), true);
    let args = json!({"provider":"signalwire","phone_number":"+12025550311","forward_to":"+12025550398","source":"Yard signs","campaign":"Pilot"});
    let blocked = reader
        .handle(rpc(
            2,
            "tools/call",
            json!({"name":"add_tracking_number","arguments":args}),
        ))
        .await
        .unwrap();
    assert_eq!(blocked["result"]["isError"], true);
    let added = writer
        .handle(rpc(
            3,
            "tools/call",
            json!({"name":"add_tracking_number","arguments":args}),
        ))
        .await
        .unwrap();
    assert_eq!(added["result"]["isError"], false, "{added}");
    assert!(added.to_string().contains("/signalwire/workspaces/w/voice"));
    assert_eq!(
        data.lock().unwrap().numbers[0].provider,
        PhoneProvider::Signalwire
    );
}
