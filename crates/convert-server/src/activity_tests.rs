//! Activity log against a disposable PostgreSQL: every source, filters, keyset paging with
//! ties, workspace isolation, scopes, the MCP tool, and no secret material in responses.
use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use std::collections::HashSet;
use tower::ServiceExt;

const OPERATOR: &str = "activity-test-operator-at-least-32-chars";
/// Values planted in columns the log must never read.
const SECRETS: [&str; 9] = [
    "GCLID-PLANTED",
    "MSCLKID-PLANTED",
    "WEBHOOK-PAYLOAD-PLANTED",
    "ENCRYPTED-SECRET-PLANTED",
    "REFRESH-TOKEN-PLANTED",
    "path-token-planted",
    "FORM-FIELD-PLANTED",
    "basic-auth-planted",
    "SESSION-EVIDENCE-PLANTED",
];

fn app(store: Store) -> Shared {
    Arc::new(App {
        store,
        api_key: OPERATOR.into(),
        public_url: "https://api.example".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: None,
        vault: None,
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
        microsoft: Default::default(),
        meta: Default::default(),
    })
}
async fn get(app: Shared, path: &str, key: &str) -> (StatusCode, Value, String) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}
/// Fixtures use readable UUIDs; a per-run prefix keeps reruns on one database independent.
fn fx(text: &str) -> String {
    static PREFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        let hex = Uuid::new_v4().simple().to_string();
        format!(
            "{}-{}-4{}-8{}-",
            &hex[..8],
            &hex[8..12],
            &hex[12..15],
            &hex[15..18]
        )
    });
    text.replace("00000000-0000-4000-8000-", prefix)
}
async fn exec1(store: &Store, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(fx(sql)))
        .bind(store.workspace)
        .execute(&store.pool)
        .await
        .unwrap();
}
async fn exec(store: &Store, sql: &str, at: chrono::DateTime<chrono::Utc>) {
    sqlx::query(sqlx::AssertSqlSafe(fx(sql)))
        .bind(store.workspace)
        .bind(at)
        .execute(&store.pool)
        .await
        .unwrap();
}
fn sources(page: &Value) -> Vec<String> {
    page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["source"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn activity_unions_sources_filters_pages_and_isolates() {
    let db = std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap();
    let store = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    store.migrate().await.unwrap();
    let other = Store::connect(&db, Uuid::new_v4()).await.unwrap();
    other.migrate().await.unwrap();
    let a = app(store.clone());
    let base = chrono::Utc::now() - chrono::Duration::hours(1);
    let t = |minutes: i64| base + chrono::Duration::minutes(minutes);

    // Calls and forms. Five forms share one timestamp to page through ties.
    exec(&store, "INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,tracking_number,destination,call_status,duration_seconds,created_at) VALUES('00000000-0000-4000-8000-00000000ca11',$1,'CA-act','Caller','+12025550111','call','Google Ads','Spring','+12025550100','+12025550199','completed',42,$2)", t(1)).await;
    exec(&store, "INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,submission_payload,created_at) SELECT gen_random_uuid(),$1,'tie-'||n,'Tie '||n,'tie@example.com','form','Website','','{\"note\":\"FORM-FIELD-PLANTED\"}',$2 FROM generate_series(1,5) n", t(2)).await;
    // A website form, joined to its site for the summary.
    exec1(&store, "INSERT INTO sites(id,workspace_id,name,origin,allowed_fields) VALUES('00000000-0000-4000-8000-0000000051e0',$1,'Main site','https://www.example.com',ARRAY['email'])").await;
    sqlx::query(sqlx::AssertSqlSafe(fx("INSERT INTO website_sessions(site_id,id,evidence) VALUES('00000000-0000-4000-8000-0000000051e0','00000000-0000-4000-8000-000000005e55','{\"gclid\":\"SESSION-EVIDENCE-PLANTED\"}')"))).execute(&store.pool).await.unwrap();
    exec(&store, "INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,created_at) VALUES('00000000-0000-4000-8000-00000000f0f0',$1,'web-1','Web Visitor','web@example.com','form','Website','Contact',$2)", t(3)).await;
    exec1(&store, "INSERT INTO website_submissions(site_id,external_id,session_id,workspace_id,lead_id,payload) VALUES('00000000-0000-4000-8000-0000000051e0','web-1','00000000-0000-4000-8000-000000005e55',$1,'00000000-0000-4000-8000-00000000f0f0','{\"message\":\"FORM-FIELD-PLANTED\"}')").await;

    // A pipeline event with a Google and a Microsoft delivery. Snapshots and requests carry
    // click IDs the log must not read.
    exec1(&store, "INSERT INTO conversion_actions(id,workspace_id,key,name,trigger,counting) VALUES(gen_random_uuid(),$1,'booked','Booked job','events','every')").await;
    exec(&store, "INSERT INTO pipeline_events(id,workspace_id,action_key,external_id,lead_id,matched_by,attribution,match_digest,value_minor,occurred_at,sendable,created_at) VALUES('00000000-0000-4000-8000-0000000e7e00',$1,'booked','order-7','00000000-0000-4000-8000-00000000ca11','lead_id','latest_click','digest',25000,$2,true,$2)", t(4)).await;
    exec(&store, "INSERT INTO conversion_outbox(id,workspace_id,kind,entity_key,transaction_id,fingerprint,snapshot,state,created_at) VALUES('00000000-0000-4000-8000-00000000b0b0',$1,'pipeline','e1','tx-1','fp','{\"gclid\":\"GCLID-PLANTED\"}','awaiting_connection',$2)", t(4)).await;
    exec(&store, "INSERT INTO conversion_deliveries(id,workspace_id,outbox_id,outbox_version,connection_revision,snapshot,request,state,attempts,created_at) VALUES(gen_random_uuid(),$1,'00000000-0000-4000-8000-00000000b0b0',1,1,'{\"kind\":\"pipeline\",\"action_key\":\"booked\",\"external_id\":\"order-7\",\"revision\":1,\"value_minor\":25000,\"gclid\":\"GCLID-PLANTED\"}','{\"conversions\":[{\"gclid\":\"GCLID-PLANTED\"}]}','processed',1,$2)", t(5)).await;
    exec(&store, "INSERT INTO microsoft_deliveries(id,workspace_id,event_id,event_revision,kind,connection_revision,snapshot,request,state,diagnostic,created_at) VALUES(gen_random_uuid(),$1,'00000000-0000-4000-8000-0000000e7e00',1,'upload',1,'{\"action_key\":\"booked\",\"external_id\":\"order-7\",\"msclkid\":\"MSCLKID-PLANTED\"}','{\"goal_name\":\"Booked\",\"msclkid\":\"MSCLKID-PLANTED\"}','failed','Goal not found',$2)", t(6)).await;

    // A paused endpoint (so new leads fan out nothing) with one failed delivery.
    exec1(&store, "INSERT INTO webhook_endpoints(id,workspace_id,url,description,event_types,encrypted_secret,state) VALUES('00000000-0000-4000-8000-0000000000e1',$1,'https://user:basic-auth-planted@hooks.example.com/path-token-planted?x=1','CRM',ARRAY['interaction.created'],'ENCRYPTED-SECRET-PLANTED','paused')").await;
    exec(&store, "INSERT INTO webhook_events(id,workspace_id,type,resource_kind,resource_id,occurred_at) VALUES('00000000-0000-4000-8000-0000000000e2',$1,'interaction.created','interaction','x',$2)", t(7)).await;
    exec(&store, "INSERT INTO webhook_deliveries(id,workspace_id,event_id,endpoint_id,payload,state,attempts,last_status,last_error,created_at) VALUES(gen_random_uuid(),$1,'00000000-0000-4000-8000-0000000000e2','00000000-0000-4000-8000-0000000000e1','{\"data\":\"WEBHOOK-PAYLOAD-PLANTED\"}','failed',3,500,'HTTP 500',$2)", t(7)).await;
    exec(&store, "INSERT INTO webhook_endpoint_events(workspace_id,endpoint_id,kind,actor,created_at) VALUES($1,'00000000-0000-4000-8000-0000000000e1','paused','owner',$2)", t(8)).await;

    // Audit: an owner change is shown with only listed settings; worker rows are not.
    exec(&store, "INSERT INTO conversion_events(workspace_id,kind,actor,payload,created_at) VALUES($1,'settings_saved','owner','{\"enabled\":true,\"refresh_token\":\"REFRESH-TOKEN-PLANTED\"}',$2)", t(9)).await;
    exec(&store, "INSERT INTO conversion_events(workspace_id,kind,actor,payload,created_at) VALUES($1,'delivery_status','worker','{\"state\":\"processed\"}',$2)", t(9)).await;
    let (s, key, _) = {
        let response = router(a.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/api-keys")
                    .header("authorization", format!("Bearer {OPERATOR}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name":"Reader","scopes":["read"],"expires_at":null}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (status, v["secret"].as_str().unwrap().to_owned(), v)
    };
    assert_eq!(s, StatusCode::OK);

    // The other workspace has its own call that must never appear here.
    exec(&other, "INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,created_at) VALUES(gen_random_uuid(),$1,'other','Other workspace','+12025550123','call','Other','',$2)", t(3)).await;

    // Everything, newest first, each source present, no worker rows, no secrets.
    let (s, all, text) = get(a.clone(), "/api/activity?limit=100", OPERATOR).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    let found: HashSet<String> = sources(&all).into_iter().collect();
    for source in [
        "interaction",
        "pipeline_event",
        "google_delivery",
        "microsoft_delivery",
        "webhook_delivery",
        "conversion_audit",
        "api_key_audit",
        "webhook_endpoint_audit",
    ] {
        assert!(found.contains(source), "{source} missing: {all}");
    }
    let items = all["data"].as_array().unwrap();
    assert_eq!(items.len(), 14, "{all}");
    assert!(all["next_cursor"].is_null());
    let times: Vec<&str> = items
        .iter()
        .map(|i| i["occurred_at"].as_str().unwrap())
        .collect();
    assert!(
        times.windows(2).all(|w| w[0] >= w[1]),
        "newest first: {times:?}"
    );
    assert!(!text.contains("Other workspace") && !text.contains("delivery_status"));
    for secret in SECRETS.iter().chain([key.as_str()].iter()) {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert!(!text.contains("vc_key_"), "key material leaked: {text}");

    // Summaries, statuses and lead links.
    let by = |source: &str| {
        items
            .iter()
            .find(|i| i["source"] == source)
            .cloned()
            .unwrap()
    };
    let call = items.iter().find(|i| i["kind"] == "call").unwrap();
    assert_eq!(call["summary"], "Call from Caller · Google Ads");
    assert_eq!(call["status"], "completed");
    assert_eq!(call["outcome"], "success");
    assert_eq!(call["detail"]["duration_seconds"], 42);
    assert!(
        call["inquiry_id"].is_string()
            && call["lead_id"] == fx("00000000-0000-4000-8000-00000000ca11")
    );
    let web = items
        .iter()
        .find(|i| i["lead_id"] == fx("00000000-0000-4000-8000-00000000f0f0"))
        .unwrap();
    assert_eq!(
        web["summary"],
        "Form from Web Visitor · Website on Main site"
    );
    let event = by("pipeline_event");
    assert_eq!(
        event["summary"],
        "Pipeline event recorded · Booked job · order-7"
    );
    assert_eq!(event["inquiry_id"], call["inquiry_id"]);
    assert_eq!(by("google_delivery")["status"], "processed");
    assert_eq!(
        by("google_delivery")["summary"],
        "Google Ads upload · Booked job · order-7"
    );
    assert_eq!(
        by("microsoft_delivery")["summary"],
        "Microsoft Ads upload · Booked job · order-7"
    );
    assert_eq!(by("google_delivery")["detail"]["value_minor"], 25000);
    let microsoft = by("microsoft_delivery");
    assert_eq!(
        (microsoft["status"].as_str(), microsoft["outcome"].as_str()),
        (Some("failed"), Some("failure"))
    );
    assert_eq!(microsoft["detail"]["diagnostic"], "Goal not found");
    let hook = by("webhook_delivery");
    assert_eq!(hook["summary"], "interaction.created → hooks.example.com");
    assert_eq!(hook["status"], "500");
    assert_eq!(hook["outcome"], "failure");
    let audit = by("conversion_audit");
    assert_eq!(
        (audit["summary"].as_str(), audit["actor"].as_str()),
        (Some("Settings saved"), Some("owner"))
    );
    assert_eq!(audit["detail"]["enabled"], true);
    assert_eq!(by("api_key_audit")["summary"], "API key “Reader” created");
    assert_eq!(
        by("webhook_endpoint_audit")["summary"],
        "Webhook endpoint hooks.example.com paused"
    );

    // Filters.
    for (kind, expected) in [
        ("call", vec!["interaction"]),
        ("form", vec!["interaction"]),
        (
            "conversion",
            vec!["pipeline_event", "google_delivery", "microsoft_delivery"],
        ),
        ("webhook", vec!["webhook_delivery"]),
        (
            "changes",
            vec![
                "conversion_audit",
                "api_key_audit",
                "webhook_endpoint_audit",
            ],
        ),
    ] {
        let (s, page, _) = get(a.clone(), &format!("/api/activity?kind={kind}"), OPERATOR).await;
        assert_eq!(s, StatusCode::OK);
        assert!(
            page["data"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["kind"] == kind),
            "{kind}: {page}"
        );
        let got: HashSet<String> = sources(&page).into_iter().collect();
        assert_eq!(
            got,
            expected.into_iter().map(String::from).collect(),
            "{kind}"
        );
    }
    // `api` is the earlier name for `changes` and still filters the same rows.
    let (s, alias, _) = get(a.clone(), "/api/activity?kind=api", OPERATOR).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(alias["data"].as_array().unwrap().len(), 3, "{alias}");
    assert!(
        alias["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["kind"] == "changes")
    );
    let (_, forms, _) = get(a.clone(), "/api/activity?kind=form", OPERATOR).await;
    assert_eq!(forms["data"].as_array().unwrap().len(), 6);
    let cutoff = t(6) - chrono::Duration::seconds(30);
    let since = urlencode(&cutoff.to_rfc3339());
    let (_, recent, _) = get(a.clone(), &format!("/api/activity?since={since}"), OPERATOR).await;
    assert!(
        recent["data"].as_array().unwrap().iter().all(|i| {
            chrono::DateTime::parse_from_rfc3339(i["occurred_at"].as_str().unwrap()).unwrap()
                >= cutoff
        }),
        "{recent}"
    );
    assert!(!sources(&recent).contains(&"pipeline_event".to_string()));
    assert!(sources(&recent).contains(&"microsoft_delivery".to_string()));

    // Keyset paging, two at a time, across ties within and between sources.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let path = match &cursor {
            Some(c) => format!("/api/activity?limit=2&cursor={}", urlencode(c)),
            None => "/api/activity?limit=2".into(),
        };
        let (s, page, text) = get(a.clone(), &path, OPERATOR).await;
        assert_eq!(s, StatusCode::OK, "{text}");
        for i in page["data"].as_array().unwrap() {
            seen.push(format!("{}:{}", i["source"], i["id"]));
        }
        cursor = page["next_cursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    let expected: Vec<String> = items
        .iter()
        .map(|i| format!("{}:{}", i["source"], i["id"]))
        .collect();
    assert_eq!(seen, expected);
    let (_, forms_page, _) = get(a.clone(), "/api/activity?kind=form&limit=4", OPERATOR).await;
    let next = forms_page["next_cursor"].as_str().unwrap();
    let (_, rest, _) = get(
        a.clone(),
        &format!("/api/activity?kind=form&limit=4&cursor={}", urlencode(next)),
        OPERATOR,
    )
    .await;
    assert_eq!(rest["data"].as_array().unwrap().len(), 2);
    assert!(rest["next_cursor"].is_null());

    // Validation.
    for bad in [
        "kind=sms",
        "limit=0",
        "limit=101",
        "cursor=garbage",
        "since=yesterday",
        "unknown=1",
    ] {
        let (s, _, text) = get(a.clone(), &format!("/api/activity?{bad}"), OPERATOR).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {text}");
    }

    // Isolation and access: the other workspace sees only its own call; the public API needs
    // a key with read; no credentials is refused.
    let (_, theirs, text) = get(app(other.clone()), "/api/activity", OPERATOR).await;
    assert_eq!(theirs["data"].as_array().unwrap().len(), 1, "{text}");
    assert_eq!(
        theirs["data"][0]["summary"],
        "Call from Other workspace · Other"
    );
    let (s, public, text) = get(a.clone(), "/api/v1/activity?kind=webhook", &key).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(public["data"].as_array().unwrap().len(), 1);
    let (s, _, _) = get(a.clone(), "/api/activity", "").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = get(a.clone(), "/api/v1/activity", "vc_key_not-a-real-key").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // The MCP tool over a read-only key.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let served = router(a.clone());
    let server = tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    let mcp = convert_cli::mcp::Server::scoped(
        Ok(convert_cli::client::Client::new(&origin, &key).unwrap()),
        vec!["read".into()],
    );
    let reply = mcp
        .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_activity","arguments":{"kind":"conversion","since":"7d","limit":2}}}))
        .await
        .unwrap();
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "{reply}");
    let page: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(page["data"].as_array().unwrap().len(), 2);
    assert!(page["next_cursor"].is_string());
    let text = page.to_string();
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} leaked over MCP");
    }
    server.abort();
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
