//! Microsoft Advertising over the real HTTP boundary, a local mock of the Microsoft
//! identity platform and the v13 Customer and Campaign Management SOAP services, and a
//! disposable PostgreSQL database. Nothing here reaches Microsoft.
use super::*;
use axum::{body::Body, extract::OriginalUri};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use pipeline_tests::{ORIGIN, World, consent, post, world};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;

const CLIENT_ID: &str = "0b5c9f6e-1f2a-4c3d-9e8f-123456789abc";
const SECRET: &str = "ms-test-secret-not-for-production";
const DEVELOPER: &str = "MSDEVTOKEN123";
const REDIRECT: &str = "https://engine.example/auth/microsoft/callback";
const SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";

struct Goal {
    id: String,
    name: String,
    status: &'static str,
    kind: &'static str,
    count: &'static str,
    category: &'static str,
}
pub(super) struct Mock {
    challenge: Mutex<String>,
    exchanges: AtomicUsize,
    /// Refresh tokens issued so far; only the latest one is accepted.
    issued: AtomicUsize,
    refreshes: AtomicUsize,
    deny: AtomicBool,
    goals: Mutex<Vec<Goal>>,
    adds: Mutex<Vec<String>>,
    lose_add_response: AtomicBool,
    /// Bodies of ApplyOfflineConversions and ApplyOfflineConversionAdjustments.
    applies: Mutex<Vec<String>>,
    /// Reply for the next apply: ok, garbage, fault:<code>:<name> or partial:<code>:<name>.
    mode: Mutex<String>,
}
impl Default for Mock {
    fn default() -> Self {
        let goal = |id: &str, name: &str, status, kind, count, category| Goal {
            id: id.into(),
            name: name.into(),
            status,
            kind,
            count,
            category,
        };
        Self {
            challenge: Mutex::default(),
            exchanges: AtomicUsize::new(0),
            issued: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            deny: AtomicBool::new(false),
            goals: Mutex::new(vec![
                goal(
                    "11",
                    "Orders",
                    "Active",
                    "OfflineConversion",
                    "All",
                    "Purchase",
                ),
                goal(
                    "12",
                    "Lead",
                    "Active",
                    "OfflineConversion",
                    "Unique",
                    "SubmitLeadForm",
                ),
                goal(
                    "13",
                    "Old import",
                    "Paused",
                    "OfflineConversion",
                    "All",
                    "Purchase",
                ),
                goal(
                    "14",
                    "Website purchase",
                    "Active",
                    "Event",
                    "All",
                    "Purchase",
                ),
            ]),
            adds: Mutex::default(),
            lose_add_response: AtomicBool::new(false),
            applies: Mutex::default(),
            mode: Mutex::new("ok".into()),
        }
    }
}
fn envelope(ns: &str, body: &str) -> Response {
    (
        [("content-type", "text/xml; charset=utf-8")],
        format!(
            "<s:Envelope xmlns:s=\"{SOAP}\"><s:Header><TrackingId xmlns=\"{ns}\">trk-{}</TrackingId></s:Header><s:Body>{body}</s:Body></s:Envelope>",
            Uuid::new_v4().simple()
        ),
    )
        .into_response()
}
fn fault(code: &str, name: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [("content-type", "text/xml; charset=utf-8")],
        format!("<s:Envelope xmlns:s=\"{SOAP}\"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring>PRIVATE_MS_ERROR private-ms-click</faultstring><detail><AdApiFaultDetail xmlns=\"https://adapi.microsoft.com\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"><TrackingId>t</TrackingId><Errors><AdApiError><Code>{code}</Code><Detail>PRIVATE_MS_ERROR</Detail><ErrorCode>{name}</ErrorCode><Message>PRIVATE_MS_ERROR</Message></AdApiError></Errors></AdApiFaultDetail></detail></s:Fault></s:Body></s:Envelope>"),
    )
        .into_response()
}
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    Some(&s[start..start + s[start..].find(close)?])
}
async fn mock(
    State(m): State<Arc<Mock>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if uri.path() == "/oauth2/v2.0/token" {
        let form = url::form_urlencoded::parse(&body).collect::<std::collections::HashMap<_, _>>();
        assert_eq!(form["client_id"], CLIENT_ID);
        assert_eq!(form["client_secret"], SECRET);
        assert_eq!(form["scope"], convert_adapters::microsoft_api::SCOPE);
        if form["grant_type"] == "authorization_code" {
            m.exchanges.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
                *m.challenge.lock().unwrap()
            );
            assert_eq!(form["redirect_uri"], REDIRECT);
            if form["code"] == "missing-scope" {
                return Json(json!({"access_token":"private-ms-access-0","refresh_token":"private-ms-refresh-0","scope":"openid","token_type":"Bearer"})).into_response();
            }
        } else {
            assert_eq!(form["grant_type"], "refresh_token");
            m.refreshes.fetch_add(1, Ordering::SeqCst);
            let latest = format!("private-ms-refresh-{}", m.issued.load(Ordering::SeqCst));
            if m.deny.load(Ordering::SeqCst) || form["refresh_token"] != latest {
                return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid_grant","error_description":"PRIVATE_MS_ERROR private-ms-refresh"}))).into_response();
            }
        }
        // Every token response rotates the refresh token.
        let n = m.issued.fetch_add(1, Ordering::SeqCst) + 1;
        return Json(json!({"access_token":format!("private-ms-access-{n}"),"refresh_token":format!("private-ms-refresh-{n}"),
            "scope":"https://ads.microsoft.com/msads.manage","token_type":"Bearer","expires_in":3600}))
        .into_response();
    }
    let xml = String::from_utf8(body.to_vec()).unwrap();
    let action = headers["soapaction"].to_str().unwrap().to_string();
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/xml")
    );
    let token = between(
        &xml,
        "AuthenticationToken xmlns:h=",
        "</h:AuthenticationToken>",
    )
    .unwrap();
    assert!(token.contains("\">private-ms-access-"), "{token}");
    assert!(xml.contains(&format!(">{DEVELOPER}</h:DeveloperToken>")));
    assert!(!xml.contains("private-ms-refresh"));
    if uri.path() == "/customer" {
        let ns = "https://bingads.microsoft.com/Customer/v13";
        assert!(!xml.contains("CustomerAccountId"));
        return match action.as_str() {
            "GetUser" => envelope(
                ns,
                &format!(
                    "<GetUserResponse xmlns=\"{ns}\"><User xmlns:e=\"{ns}/Entities\"><e:ContactInfo><e:Id>1</e:Id></e:ContactInfo><e:Id>4242</e:Id></User><CustomerRoles xmlns:e=\"{ns}/Entities\"><e:CustomerRole><e:AccountIds i:nil=\"true\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/><e:CustomerId>500</e:CustomerId><e:LinkedAccountIds xmlns:a=\"http://schemas.microsoft.com/2003/10/Serialization/Arrays\"><a:long>7002</a:long></e:LinkedAccountIds><e:RoleId>41</e:RoleId></e:CustomerRole></CustomerRoles></GetUserResponse>"
                ),
            ),
            "SearchAccounts" => {
                let field = between(&xml, "<e:Field>", "</e:Field>").unwrap();
                let value = between(&xml, "<e:Value>", "</e:Value>").unwrap();
                let all = [
                    ("7001", "Quarry Rentals", "500"),
                    ("7002", "Agency client", "900"),
                ];
                let accounts: String = all
                    .iter()
                    .filter(|(id, _, _)| match field {
                        "UserId" => value == "4242",
                        "AccountId" => value == *id,
                        _ => panic!("{field}"),
                    })
                    .map(|(id, name, parent)| format!("<e:AdvertiserAccount><e:CurrencyCode>USD</e:CurrencyCode><e:Id>{id}</e:Id><e:Name>{name}</e:Name><e:Number>F{id}</e:Number><e:ParentCustomerId>{parent}</e:ParentCustomerId><e:AccountLifeCycleStatus>Active</e:AccountLifeCycleStatus></e:AdvertiserAccount>"))
                    .collect();
                envelope(
                    ns,
                    &format!(
                        "<SearchAccountsResponse xmlns=\"{ns}\"><Accounts xmlns:e=\"{ns}/Entities\">{accounts}</Accounts></SearchAccountsResponse>"
                    ),
                )
            }
            _ => panic!("{action}"),
        };
    }
    assert_eq!(uri.path(), "/campaign");
    let ns = "https://bingads.microsoft.com/CampaignManagement/v13";
    assert!(xml.contains(">7001</h:CustomerAccountId>"), "{xml}");
    assert!(xml.contains(">500</h:CustomerId>"), "{xml}");
    match action.as_str() {
        "GetConversionGoalsByIds" => {
            let types = between(&xml, "<ConversionGoalTypes>", "</ConversionGoalTypes>").unwrap();
            let goals: String = m
                .goals
                .lock()
                .unwrap()
                .iter()
                .filter(|g| types.split(' ').any(|t| t == g.kind))
                .map(|g| format!("<ConversionGoal i:type=\"{}Goal\"><ConversionWindowInMinutes>129600</ConversionWindowInMinutes><CountType>{}</CountType><ExcludeFromBidding>false</ExcludeFromBidding><GoalCategory>{}</GoalCategory><Id>{}</Id><Name>{}</Name><Revenue><Type>VariableValue</Type></Revenue><Scope>Account</Scope><Status>{}</Status><Type>{}</Type></ConversionGoal>", g.kind, g.count, g.category, g.id, g.name, g.status, g.kind))
                .collect();
            envelope(
                ns,
                &format!(
                    "<GetConversionGoalsByIdsResponse xmlns=\"{ns}\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"><ConversionGoals>{goals}<ConversionGoal i:nil=\"true\"/></ConversionGoals><PartialErrors i:nil=\"true\"/></GetConversionGoalsByIdsResponse>"
                ),
            )
        }
        "AddConversionGoals" => {
            m.adds.lock().unwrap().push(xml.clone());
            let name = between(&xml, "<Name>", "</Name>").unwrap().to_string();
            let mut goals = m.goals.lock().unwrap();
            if goals.iter().any(|g| g.name == name) {
                return fault("5321", "ConversionGoalNameAlreadyExists");
            }
            let id = (100 + goals.len()).to_string();
            goals.push(Goal {
                id: id.clone(),
                name,
                status: "Active",
                kind: "OfflineConversion",
                count: if xml.contains("<CountType>Unique</CountType>") {
                    "Unique"
                } else {
                    "All"
                },
                category: "SubmitLeadForm",
            });
            if m.lose_add_response.swap(false, Ordering::SeqCst) {
                return (StatusCode::BAD_GATEWAY, "upstream reset").into_response();
            }
            envelope(
                ns,
                &format!(
                    "<AddConversionGoalsResponse xmlns=\"{ns}\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"><ConversionGoalIds xmlns:a=\"http://schemas.microsoft.com/2003/10/Serialization/Arrays\"><a:long>{id}</a:long></ConversionGoalIds><PartialErrors i:nil=\"true\"/></AddConversionGoalsResponse>"
                ),
            )
        }
        "ApplyOfflineConversions" | "ApplyOfflineConversionAdjustments" => {
            m.applies.lock().unwrap().push(xml.clone());
            let mode = std::mem::replace(&mut *m.mode.lock().unwrap(), "ok".into());
            let parts: Vec<&str> = mode.split(':').collect();
            match parts[0] {
                "garbage" => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "PRIVATE_MS_ERROR").into_response()
                }
                "fault" => fault(parts[1], parts[2]),
                "partial" => envelope(
                    ns,
                    &format!(
                        "<{action}Response xmlns=\"{ns}\"><PartialErrors><BatchError><Code>{}</Code><Details>PRIVATE_MS_ERROR</Details><ErrorCode>{}</ErrorCode><FieldPath i:nil=\"true\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/><Index>0</Index><Message>PRIVATE_MS_ERROR private-ms-click</Message><Type>BatchError</Type></BatchError></PartialErrors></{action}Response>",
                        parts[1], parts[2]
                    ),
                ),
                _ => envelope(
                    ns,
                    &format!(
                        "<{action}Response xmlns=\"{ns}\"><PartialErrors i:nil=\"true\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/></{action}Response>"
                    ),
                ),
            }
        }
        _ => panic!("{action}"),
    }
}
async fn request(
    app: Shared,
    method: &str,
    path: &str,
    bearer: &str,
    cookie: &str,
    value: Value,
) -> (StatusCode, HeaderMap, String) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    // Tokens, the developer token and upstream messages never reach a response.
    for private in ["private-ms", "PRIVATE_MS", DEVELOPER, SECRET] {
        assert!(!text.contains(private), "{path}: {text}");
    }
    (code, headers, text)
}
async fn api(app: Shared, method: &str, path: &str, key: &str, v: Value) -> (StatusCode, Value) {
    let (status, _, body) = request(app, method, path, key, "", v).await;
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}
/// The given app, with Microsoft configured against a fresh local mock.
async fn with_microsoft(a: &Shared) -> (Shared, Arc<Mock>, tokio::task::JoinHandle<()>) {
    let m = Arc::new(Mock::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock_app = Router::new().fallback(mock).with_state(m.clone());
    let task = tokio::spawn(async move { axum::serve(listener, mock_app).await.unwrap() });
    // Park Microsoft work left in this disposable database by earlier tests.
    for q in [
        "UPDATE microsoft_deliveries SET due_at=NULL WHERE state IN ('queued','retry')",
        "UPDATE microsoft_event_plans SET state='final' WHERE state='blocked'",
        "UPDATE conversion_actions SET microsoft_goal_name=NULL,microsoft_mapped_at=NULL WHERE microsoft_goal_name IS NOT NULL",
    ] {
        sqlx::query(q).execute(&a.store.pool).await.unwrap();
    }
    let mut app = (**a).clone();
    app.microsoft = microsoft::Microsoft {
        config: Some(
            microsoft::Config::new(
                "https://engine.example",
                CLIENT_ID.into(),
                SECRET.into(),
                DEVELOPER.into(),
            )
            .unwrap(),
        ),
        client: convert_adapters::microsoft_api::Client::for_test(origin),
    };
    (Arc::new(app), m, task)
}
async fn connect(a: &Shared, m: &Mock, code: &str) -> String {
    let revision = a.store.microsoft_connection().await.unwrap().revision;
    let (s, h, b) = request(
        a.clone(),
        "POST",
        "/api/connections/microsoft/start",
        &a.api_key,
        "",
        json!({"revision":revision}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    let u = url::Url::parse(v["authorization_url"].as_str().unwrap()).unwrap();
    assert_eq!(
        u.as_str().split('?').next().unwrap(),
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
    );
    let q = u.query_pairs().collect::<std::collections::HashMap<_, _>>();
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], REDIRECT);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["response_mode"], "query");
    assert_eq!(
        q["scope"],
        "https://ads.microsoft.com/msads.manage offline_access"
    );
    assert_eq!(q["code_challenge_method"], "S256");
    *m.challenge.lock().unwrap() = q["code_challenge"].to_string();
    let cookie = h["set-cookie"].to_str().unwrap();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("Secure")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Path=/auth/microsoft/callback")
    );
    let cookie = cookie.split(';').next().unwrap().to_string();
    let state = q["state"].to_string();
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM microsoft_oauth_attempts t WHERE workspace_id=$1",
    )
    .bind(a.store.workspace)
    .fetch_one(&a.store.pool)
    .await
    .unwrap();
    assert!(!row.to_string().contains(&state));
    assert!(!row.to_string().contains(cookie.split('=').nth(1).unwrap()));
    let (s, h, b) = request(
        a.clone(),
        "GET",
        &format!("/auth/microsoft/callback?state={state}&code={code}"),
        "",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(b.is_empty());
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(h["referrer-policy"], "no-referrer");
    // The same state and cookie cannot be replayed.
    let (_, replay, _) = request(
        a.clone(),
        "GET",
        &format!("/auth/microsoft/callback?state={state}&code={code}"),
        "",
        &cookie,
        Value::Null,
    )
    .await;
    assert!(replay["location"].to_str().unwrap().ends_with("failed"));
    h["location"].to_str().unwrap().into()
}
async fn key(a: &Shared, scopes: &[&str]) -> String {
    a.store
        .issue_api_key(
            &convert_store::api_keys::KeyInput {
                name: format!("ms {}", scopes.join(" ")),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                expires_at: None,
            },
            None,
            None,
        )
        .await
        .unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_string()
}
/// Choose ad account 7001 as the owner and return the new destination version.
async fn choose(a: &Shared) -> i64 {
    let c = a.store.microsoft_connection().await.unwrap();
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/conversion-setup/microsoft-destination",
        &a.api_key,
        json!({"revision":c.revision,"destination_version":c.destination_version,"account_id":"7001"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["account"]["customer_id"], "500");
    v["destination_version"].as_i64().unwrap()
}
async fn check(a: &Shared) -> Value {
    let c = a.store.microsoft_connection().await.unwrap();
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/conversion-setup/microsoft-check",
        &a.api_key,
        json!({"revision":c.revision,"destination_version":c.destination_version}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}
async fn map(a: &Shared, action: &str, goal: Value) -> Value {
    let version = a
        .store
        .conversion_actions()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v["key"] == action)
        .unwrap()["version"]
        .clone();
    let (s, v) = api(
        a.clone(),
        "PUT",
        &format!("/api/conversion-actions/{action}/microsoft-goal"),
        &a.api_key,
        json!({"version":version,"goal_name":goal}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn microsoft_oauth_rotation_discovery_destination_and_scopes() {
    let w = world("not_required_outside_eea", true).await;
    let (a, m, task) = with_microsoft(&w.a).await;
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/connections/microsoft",
        &a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["configured"], true);
    assert_eq!(v["connected"], false);
    assert_eq!(v["redirect_uri"], REDIRECT);
    // Without the configuration the section reports unavailable; Google is unaffected.
    let (_, setup) = api(
        w.a.clone(),
        "GET",
        "/api/conversion-setup",
        &w.a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(setup["microsoft"]["configured"], false);
    assert_eq!(
        setup["microsoft"]["next_steps"][0]["actor"],
        "administrator"
    );
    assert!(
        setup["next_steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| !s["step"].as_str().unwrap().contains("Microsoft"))
    );
    // A token response without Microsoft Advertising access is refused.
    assert!(connect(&a, &m, "missing-scope").await.ends_with("failed"));
    assert!(connect(&a, &m, "good").await.ends_with("connected"));
    assert_eq!(m.exchanges.load(Ordering::SeqCst), 2);
    let stored: Value =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM microsoft_connections c WHERE workspace_id=$1")
            .bind(a.store.workspace)
            .fetch_one(&a.store.pool)
            .await
            .unwrap();
    assert!(!stored.to_string().contains("private-ms"));
    let sealed = a
        .store
        .microsoft_connection()
        .await
        .unwrap()
        .encrypted_token
        .unwrap();
    // Every refresh rotates the refresh token; each replacement is sealed and used next.
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/connections/microsoft/accounts",
        &a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let rotated = a
        .store
        .microsoft_connection()
        .await
        .unwrap()
        .encrypted_token
        .unwrap();
    assert_ne!(sealed, rotated);
    assert_eq!(v["accounts"][0]["id"], "7001");
    assert_eq!(v["accounts"][0]["customer_id"], "500");
    // An account reached through a linked (agency) role operates from that customer.
    assert_eq!(v["accounts"][1]["id"], "7002");
    assert_eq!(v["accounts"][1]["customer_id"], "500");
    assert_eq!(v["accounts"][1]["parent_customer_id"], "900");
    let (s, _) = api(
        a.clone(),
        "GET",
        "/api/connections/microsoft/accounts",
        &a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m.refreshes.load(Ordering::SeqCst), 2);
    // Scopes: discovery is read; the ad account is conversions:send; checks and goals are
    // conversions:write. OAuth is never reachable with a key.
    let read = key(&a, &["read"]).await;
    let write = key(&a, &["read", "conversions:write"]).await;
    let send = key(&a, &["read", "conversions:send"]).await;
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/v1/connections/microsoft/accounts",
        &read,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let c = a.store.microsoft_connection().await.unwrap();
    let destination = json!({"revision":c.revision,"destination_version":0,"account_id":"7001"});
    for k in [&read, &write] {
        let (s, v) = api(
            a.clone(),
            "PUT",
            "/api/v1/conversion-setup/microsoft-destination",
            k,
            destination.clone(),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(v["error"].as_str().unwrap().contains("conversions:send"));
    }
    for (method, path, body) in [
        (
            "POST",
            "/api/v1/conversion-setup/microsoft-check",
            json!({"revision":1,"destination_version":1}),
        ),
        (
            "PUT",
            "/api/v1/conversion-actions/order/microsoft-goal",
            json!({"version":1,"goal_name":"Orders"}),
        ),
        (
            "POST",
            "/api/v1/conversion-actions/order/microsoft-goal",
            json!({"version":1}),
        ),
    ] {
        let (s, v) = api(a.clone(), method, path, &read, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
        assert!(v["error"].as_str().unwrap().contains("conversions:write"));
    }
    for k in [&read, &write, &send] {
        let (s, _) = api(
            a.clone(),
            "POST",
            "/api/v1/connections/microsoft/start",
            k,
            json!({"revision":c.revision}),
        )
        .await;
        assert!(
            s == StatusCode::NOT_FOUND || s == StatusCode::METHOD_NOT_ALLOWED,
            "{s}"
        );
    }
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/microsoft-destination",
        &send,
        json!({"revision":c.revision,"destination_version":0,"account_id":"9999"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("not reachable"));
    let (s, v) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/microsoft-destination",
        &send,
        destination.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["account"]["name"], "Quarry Rentals");
    assert_eq!(v["destination_version"], 1);
    assert_eq!(v["setup"]["microsoft"]["account_id"], "7001");
    let (s, _) = api(
        a.clone(),
        "PUT",
        "/api/v1/conversion-setup/microsoft-destination",
        &send,
        destination,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // A revoked grant surfaces as a reconnect prompt without the provider's text.
    m.deny.store(true, Ordering::SeqCst);
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/v1/connections/microsoft/goals",
        &read,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("Reconnect"));
    m.deny.store(false, Ordering::SeqCst);
    // Disconnect keeps the chosen account but removes the credential.
    let (s, _) = api(
        a.clone(),
        "DELETE",
        "/api/connections/microsoft",
        &a.api_key,
        json!({"revision":c.revision}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let c = a.store.microsoft_connection().await.unwrap();
    assert!(c.encrypted_token.is_none());
    assert_eq!(c.account_id.as_deref(), Some("7001"));
    let (_, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &read,
        Value::Null,
    )
    .await;
    assert_eq!(setup["microsoft"]["connected"], false);
    assert_eq!(setup["microsoft"]["next_steps"][0]["actor"], "owner");
    task.abort();
    w.task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn microsoft_goals_create_recover_map_check_and_review() {
    let w = world("not_required_outside_eea", true).await;
    let (a, m, task) = with_microsoft(&w.a).await;
    assert!(connect(&a, &m, "good").await.ends_with("connected"));
    // Goal creation needs the chosen account first.
    let version = |key: &str| {
        let a = a.clone();
        let key = key.to_string();
        async move {
            a.store
                .conversion_actions()
                .await
                .unwrap()
                .into_iter()
                .find(|v| v["key"] == key.as_str())
                .unwrap()["version"]
                .clone()
        }
    };
    let (s, _) = api(
        a.clone(),
        "POST",
        "/api/conversion-actions/quote/microsoft-goal",
        &a.api_key,
        json!({"version":version("quote").await}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    choose(&a).await;
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/connections/microsoft/goals",
        &a.api_key,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let names: Vec<&str> = v["goals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Orders", "Lead", "Old import"],
        "offline goals only"
    );
    assert_eq!(v["goals"][2]["eligible"], false);
    // Create a VisiConvert goal for quote (once per click → Unique).
    let write = key(&a, &["read", "conversions:write"]).await;
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-actions/quote/microsoft-goal",
        &write,
        json!({"version":version("quote").await}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let name = format!("VisiConvert - quote ({})", a.store.workspace);
    assert_eq!(v["goal"]["name"], name.as_str());
    assert_eq!(v["created"], true);
    assert!(v["accepts_uploads_after"].is_string());
    assert_eq!(v["action"]["microsoft_goal_name"], name.as_str());
    let add = m.adds.lock().unwrap()[0].clone();
    for part in [
        "<ConversionGoal i:type=\"OfflineConversionGoal\">",
        "<ConversionWindowInMinutes>129600</ConversionWindowInMinutes>",
        "<CountType>Unique</CountType>",
        "<GoalCategory>SubmitLeadForm</GoalCategory>",
        "<Revenue><Type>VariableValue</Type></Revenue>",
        "<Scope>Account</Scope>",
        "<Status>Active</Status>",
    ] {
        assert!(add.contains(part), "{part}");
    }
    let (s, _) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-actions/quote/microsoft-goal",
        &write,
        json!({"version":version("quote").await}),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "never replaces a mapping");
    // A lost response is recovered by name, not created twice.
    m.lose_add_response.store(true, Ordering::SeqCst);
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-actions/order/microsoft-goal",
        &write,
        json!({"version":version("order").await,"category":"Purchase"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(m.adds.lock().unwrap().len(), 2);
    assert!(m.adds.lock().unwrap()[1].contains("<CountType>All</CountType>"));
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/v1/conversion-actions/order/microsoft-goal",
        &write,
        json!({"version":version("order").await,"category":"Lead"}),
    )
    .await;
    assert!(
        s == StatusCode::CONFLICT || s == StatusCode::BAD_REQUEST,
        "{v}"
    );
    // Remap order to an existing goal, then check.
    map(&a, "order", json!("Orders")).await;
    let report = check(&a).await;
    assert_eq!(report["valid"], true, "{report}");
    assert_eq!(report["account"]["status"], "Active");
    let quote = report["mapped"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["action"] == "quote")
        .unwrap();
    assert_eq!(quote["eligible"], true);
    assert!(
        quote["accepts_uploads_after"].is_string(),
        "two-hour wait is reported"
    );
    let (_, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &write,
        Value::Null,
    )
    .await;
    let ms = &setup["microsoft"];
    assert_eq!(ms["ready"], true, "{ms}");
    assert_eq!(ms["check_current"], true);
    assert!(
        ms["next_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["advisory"] == true && s["step"].as_str().unwrap().contains("after"))
    );
    // A mapped goal that disappears fails the check with an owner step.
    map(&a, "order", json!("Missing goal")).await;
    let report = check(&a).await;
    assert_eq!(report["valid"], false);
    let (_, setup) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup",
        &write,
        Value::Null,
    )
    .await;
    assert_eq!(
        setup["microsoft"]["next_steps"][0]["actor"], "owner",
        "{setup}"
    );
    map(&a, "order", json!("Orders")).await;
    // Review flags the other active offline goal and the website goal in the same category.
    let (s, v) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-setup/microsoft-review",
        &write,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let flags = |name: &str| {
        v["goals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["name"] == name)
            .unwrap()["flags"]
            .clone()
    };
    assert_eq!(flags("Lead"), json!(["other_offline_goal"]));
    assert_eq!(
        flags("Website purchase"),
        json!(["website_goal_same_category"])
    );
    assert_eq!(flags("Orders"), json!([]));
    assert_eq!(flags("Old import"), json!([]));
    task.abort();
    w.task.abort();
}

/// A form lead whose visit carried the given click IDs.
async fn ms_lead(w: &World, phone: &str, gclid: Option<&str>, msclkid: Option<&str>) -> Uuid {
    let session = Uuid::new_v4();
    let mut evidence =
        json!({"source":"bing","consent":"granted","landing_url":format!("{ORIGIN}/")});
    if let Some(c) = gclid {
        evidence["gclid"] = json!(c);
    }
    if let Some(c) = msclkid {
        evidence["msclkid"] = json!(c);
    }
    w.a.store
        .capture_session(w.site, ORIGIN, session, &evidence)
        .await
        .unwrap();
    let fields = BTreeMap::from([("phone".to_string(), phone.to_string())]);
    w.a.store
        .capture_form(w.site, ORIGIN, session, &session.to_string(), &fields)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT lead_id FROM website_submissions WHERE site_id=$1 AND session_id=$2")
        .bind(w.site)
        .bind(session)
        .fetch_one(&w.a.store.pool)
        .await
        .unwrap()
}
async fn drain(a: &App) -> usize {
    let mut n = 0;
    while microsoft::run_one(a).await.unwrap() {
        n += 1;
    }
    n
}
async fn ms_rows(a: &App, event: &Value) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(d) FROM microsoft_deliveries d WHERE workspace_id=$1 AND event_id=$2 ORDER BY created_at")
        .bind(a.store.workspace).bind(event["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_all(&a.store.pool).await.unwrap()
}
async fn make_due(a: &App) {
    sqlx::query("UPDATE microsoft_deliveries SET due_at=now()-interval '1 second' WHERE workspace_id=$1 AND state IN ('queued','retry') AND due_at IS NOT NULL")
        .bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn microsoft_pipeline_uploads_adjustments_errors_and_eligibility() {
    let w = world("not_required_outside_eea", true).await;
    let (a, m, task) = with_microsoft(&w.a).await;
    let w = World { a: a.clone(), ..w };
    let endpoint = Uuid::new_v4();
    sqlx::query("INSERT INTO webhook_endpoints(id,workspace_id,url,event_types,encrypted_secret) VALUES($1,$2,'https://hooks.example/visiconvert',ARRAY['conversion.delivered','conversion.failed'],'sealed')")
        .bind(endpoint).bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    assert!(connect(&a, &m, "good").await.ends_with("connected"));
    choose(&a).await;
    // An event recorded before the action is mapped is never sent to Microsoft.
    ms_lead(&w, "602-555-0100", None, Some("ms-click-early")).await;
    let (s, early) = post(&w, json!({"action":"order","external_id":"early","match":{"phone":"6025550100"},"value_minor":1000})).await;
    assert_eq!(s, StatusCode::OK, "{early}");
    assert_eq!(
        early["matched_by"], "none",
        "msclkid leads match only actions mapped to Microsoft"
    );
    assert!(early["delivery"]["microsoft"].is_null());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    map(&a, "order", json!("Orders")).await;
    check(&a).await;
    let (_, early) = post(&w, json!({"action":"order","external_id":"early","match":{"phone":"6025550100"},"value_minor":1000})).await;
    assert_eq!(
        early["matched_by"], "phone",
        "rematched once mapped: {early}"
    );
    assert_eq!(
        early["delivery"]["microsoft"]["status"], "blocked",
        "{early}"
    );
    assert!(
        early["delivery"]["microsoft"]["reason"]
            .as_str()
            .unwrap()
            .contains("before this action was mapped")
    );
    assert!(ms_rows(&a, &early).await.is_empty());

    // Both clicks: Google and Microsoft each queue their own request.
    ms_lead(&w, "602-555-0101", Some("g-click-a"), Some("ms-click-a")).await;
    let order = json!({"action":"order","external_id":"order_1","match":{"phone":"6025550101"},"value_minor":41250});
    let (_, v) = post(&w, order.clone()).await;
    assert_eq!(v["delivery"]["google"], "queued", "{v}");
    assert_eq!(v["delivery"]["microsoft"]["status"], "queued", "{v}");
    let both = v.clone();
    // Google click only: the Google send is unchanged and nothing goes to Microsoft.
    ms_lead(&w, "602-555-0102", Some("g-click-b"), None).await;
    let (_, v) = post(&w, json!({"action":"order","external_id":"order_2","match":{"phone":"6025550102"},"value_minor":500})).await;
    assert_eq!(v["delivery"]["google"], "queued", "{v}");
    assert_eq!(v["delivery"]["microsoft"]["status"], "blocked");
    assert!(
        v["delivery"]["microsoft"]["reason"]
            .as_str()
            .unwrap()
            .contains("No Microsoft Advertising click")
    );
    assert!(ms_rows(&a, &v).await.is_empty());
    // An action without a Microsoft goal is never planned for Microsoft.
    let (_, v) = post(
        &w,
        json!({"action":"quote","external_id":"quote_2","match":{"phone":"6025550102"}}),
    )
    .await;
    assert!(v["delivery"]["microsoft"].is_null(), "{v}");
    // Microsoft click only: matched through the Microsoft fallback; Google reports no click.
    ms_lead(&w, "602-555-0103", None, Some("ms-click-c")).await;
    let (_, v) = post(&w, json!({"action":"order","external_id":"order_3","match":{"phone":"6025550103"},"value_minor":900})).await;
    assert_eq!(v["matched_by"], "phone");
    assert_eq!(v["delivery"]["google"], "blocked");
    assert_eq!(v["delivery"]["microsoft"]["status"], "queued", "{v}");

    assert_eq!(drain(&a).await, 2);
    let applies = m.applies.lock().unwrap().clone();
    assert_eq!(applies.len(), 2);
    let first = &applies[0];
    assert!(first.contains("<ConversionCurrencyCode>USD</ConversionCurrencyCode><ConversionName>Orders</ConversionName><ConversionTime>"), "{first}");
    assert!(first.contains(
        "<ConversionValue>412.50</ConversionValue><MicrosoftClickId>ms-click-a</MicrosoftClickId>"
    ));
    let time = between(first, "<ConversionTime>", "</ConversionTime>")
        .unwrap()
        .to_string();
    assert!(time.ends_with(".000Z") || time.ends_with('Z'));
    let rows = ms_rows(&a, &both).await;
    assert_eq!(rows[0]["state"], "processed");
    assert!(rows[0]["tracking_id"].as_str().unwrap().starts_with("trk-"));
    assert!(!rows[0]["request"].to_string().contains("private-ms"));
    let (_, list) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-events?external_id=order_1",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(
        list["data"][0]["delivery"]["microsoft"]["status"], "delivered",
        "{list}"
    );
    // The conversion webhook names provider microsoft.
    let hook = sqlx::query("SELECT d.id,e.id AS event_id,e.type,e.resource_id,e.resource_version,e.occurred_at FROM webhook_deliveries d JOIN webhook_events e ON e.id=d.event_id WHERE d.workspace_id=$1 AND e.resource_id=$2")
        .bind(a.store.workspace).bind(rows[0]["id"].as_str().unwrap()).fetch_one(&a.store.pool).await.unwrap();
    assert_eq!(hook.get::<String, _>("type"), "conversion.delivered");
    let claim = convert_store::webhooks::Claim {
        id: hook.get("id"),
        workspace: a.store.workspace,
        event_id: hook.get("event_id"),
        event_type: hook.get("type"),
        resource_kind: "conversion".into(),
        resource_id: hook.get("resource_id"),
        resource_version: hook.get("resource_version"),
        occurred_at: hook.get("occurred_at"),
        endpoint_id: endpoint,
        url: String::new(),
        include_contact: false,
        secret: String::new(),
        previous_secret: None,
        payload: None,
        attempts: 0,
    };
    let payload = a.store.webhook_payload(&claim).await.unwrap();
    assert_eq!(payload["data"]["provider"], "microsoft", "{payload}");
    assert_eq!(payload["data"]["pipeline_event"]["external_id"], "order_1");

    // A new value restates the accepted conversion; same click and original time.
    let mut revised = order.clone();
    revised["value_minor"] = json!(50000);
    let (_, v) = post(&w, revised).await;
    assert_eq!(v["delivery"]["microsoft"]["status"], "queued", "{v}");
    assert_eq!(v["delivery"]["microsoft"]["kind"], "restate");
    assert_eq!(drain(&a).await, 1);
    let restate = m.applies.lock().unwrap()[2].clone();
    assert!(
        restate.contains("<AdjustmentCurrencyCode>USD</AdjustmentCurrencyCode><AdjustmentTime>"),
        "{restate}"
    );
    assert!(restate.contains("<AdjustmentType>Restate</AdjustmentType><AdjustmentValue>500.00</AdjustmentValue><ConversionName>Orders</ConversionName>"));
    assert!(restate.contains(&format!(
        "<ConversionTime>{time}</ConversionTime><MicrosoftClickId>ms-click-a</MicrosoftClickId>"
    )));
    // Canceling retracts it.
    let mut canceled = order.clone();
    canceled["status"] = json!("canceled");
    let (_, v) = post(&w, canceled).await;
    assert_eq!(v["delivery"]["microsoft"]["kind"], "retract", "{v}");
    assert_eq!(drain(&a).await, 1);
    let retract = m.applies.lock().unwrap()[3].clone();
    assert!(retract.contains(
        "<AdjustmentType>Retract</AdjustmentType><ConversionName>Orders</ConversionName>"
    ));
    assert!(!retract.contains("AdjustmentValue"));
    let (_, list) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-events?external_id=order_1",
        &w.key,
        Value::Null,
    )
    .await;
    assert_eq!(
        list["data"][0]["delivery"]["microsoft"]["status"],
        "retracted"
    );

    // Permanent per-item error: failed, not retried, attempt marker cleared.
    let mut n = 10;
    let mut next = |mode: &str| {
        n += 1;
        *m.mode.lock().unwrap() = mode.into();
        n
    };
    let k = next("partial:5601:OfflineConversionMicrosoftClickIdInvalid");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    let row = &ms_rows(&a, &v).await[0];
    assert_eq!(row["state"], "failed", "{row}");
    assert!(row["attempted_at"].is_null());
    assert!(row["due_at"].is_null());
    assert!(
        row["diagnostic"]
            .as_str()
            .unwrap()
            .contains("OfflineConversionMicrosoftClickIdInvalid")
    );
    assert!(!row.to_string().contains("PRIVATE_MS"));
    make_due(&a).await;
    assert_eq!(drain(&a).await, 0);

    // Retryable fault (rate limit): retried later, then accepted. Duplicates of the same
    // click and time are ignored by Microsoft, so the retry cannot count twice.
    let k = next("fault:117:CallRateExceeded");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    let row = &ms_rows(&a, &v).await[0];
    assert_eq!(row["state"], "retry", "{row}");
    assert!(row["attempted_at"].is_null());
    assert!(row["due_at"].is_string());
    make_due(&a).await;
    assert_eq!(drain(&a).await, 1);
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "processed");

    // Goal not accepting yet (5615): retried after about 30 minutes.
    let k = next("partial:5615:OfflineConversionNotAcceptedForGoal");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    let row = &ms_rows(&a, &v).await[0];
    assert_eq!(row["state"], "retry");
    assert!(row["diagnostic"].as_str().unwrap().contains("2 hours"));
    let wait: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM due_at-now())::float8 FROM microsoft_deliveries WHERE id=$1",
    )
    .bind(row["id"].as_str().unwrap().parse::<Uuid>().unwrap())
    .fetch_one(&a.store.pool)
    .await
    .unwrap();
    assert!(wait > 1700.0, "{wait}");
    sqlx::query("UPDATE microsoft_deliveries SET due_at=NULL WHERE id=$1")
        .bind(row["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .execute(&a.store.pool)
        .await
        .unwrap();

    // Ambiguous response: uncertain, never sent again, and a revision is not sent either.
    let k = next("garbage");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let body = json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100});
    let (_, v) = post(&w, body.clone()).await;
    let before = m.applies.lock().unwrap().len();
    assert_eq!(drain(&a).await, 1);
    let row = &ms_rows(&a, &v).await[0];
    assert_eq!(row["state"], "uncertain", "{row}");
    assert!(row["attempted_at"].is_string());
    make_due(&a).await;
    assert_eq!(drain(&a).await, 0);
    let mut revised = body.clone();
    revised["value_minor"] = json!(200);
    let (_, v) = post(&w, revised).await;
    assert_eq!(v["delivery"]["microsoft"]["status"], "blocked", "{v}");
    assert!(
        v["delivery"]["microsoft"]["reason"]
            .as_str()
            .unwrap()
            .contains("not sent again automatically")
    );
    assert_eq!(drain(&a).await, 0);
    assert_eq!(m.applies.lock().unwrap().len(), before + 1);

    // Authorization refused: needs review, queued again after reconnecting.
    let k = next("fault:105:InvalidCredentials");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    assert_eq!(drain(&a).await, 1);
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "needs_review");
    assert!(connect(&a, &m, "good").await.ends_with("connected"));
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "queued");
    check(&a).await;
    assert_eq!(drain(&a).await, 1);
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "processed");

    // A worker that stopped after arming leaves the request uncertain, never replayed.
    let k = next("ok");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    let id: Uuid = ms_rows(&a, &v).await[0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE microsoft_deliveries SET state='sending',attempted_at=now(),due_at=now()-interval '1 minute' WHERE id=$1").bind(id).execute(&a.store.pool).await.unwrap();
    let before = m.applies.lock().unwrap().len();
    assert_eq!(drain(&a).await, 0);
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "uncertain");
    assert_eq!(m.applies.lock().unwrap().len(), before);

    // Paused sending holds queued requests; nothing leaves.
    let k = next("ok");
    ms_lead(
        &w,
        &format!("602-555-01{k}"),
        None,
        Some(&format!("ms-{k}")),
    )
    .await;
    let (_, v) = post(&w, json!({"action":"order","external_id":format!("o{k}"),"match":{"phone":format!("60255501{k}")},"value_minor":100})).await;
    assert_eq!(v["delivery"]["microsoft"]["status"], "queued");
    let policy = a.store.pipeline_policy().await.unwrap();
    a.store
        .set_pipeline_policy(
            &convert_store::pipeline::PolicyInput {
                auto_send: false,
                consent_policy: "not_required_outside_eea".into(),
                attest: true,
                version: policy["version"].as_i64().unwrap() as i32,
            },
            "test",
        )
        .await
        .unwrap();
    assert_eq!(drain(&a).await, 0);
    assert_eq!(ms_rows(&a, &v).await[0]["state"], "queued");
    task.abort();
    w.task.abort();
}

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn microsoft_consent_new_goal_wait_and_inferred_calls() {
    let w = world("per_lead", true).await;
    let (a, m, task) = with_microsoft(&w.a).await;
    let w = World { a: a.clone(), ..w };
    assert!(connect(&a, &m, "good").await.ends_with("connected"));
    choose(&a).await;
    let version = a
        .store
        .conversion_actions()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v["key"] == "order")
        .unwrap()["version"]
        .clone();
    let (s, v) = api(
        a.clone(),
        "POST",
        "/api/conversion-actions/order/microsoft-goal",
        &a.api_key,
        json!({"version":version}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    check(&a).await;
    let lead = ms_lead(&w, "602-555-0300", None, Some("ms-consent")).await;
    let body = json!({"action":"order","external_id":"c1","match":{"phone":"6025550300"},"value_minor":100});
    let (_, v) = post(&w, body.clone()).await;
    assert_eq!(v["delivery"]["microsoft"]["status"], "blocked");
    assert!(
        v["delivery"]["microsoft"]["reason"]
            .as_str()
            .unwrap()
            .contains("consent"),
        "{v}"
    );
    consent(&a, lead, chrono::Utc::now() - chrono::Duration::days(1)).await;
    sqlx::query("UPDATE microsoft_event_plans SET plan_after=now() WHERE workspace_id=$1")
        .bind(a.store.workspace)
        .execute(&a.store.pool)
        .await
        .unwrap();
    a.store.plan_microsoft(None).await.unwrap();
    let (_, list) = api(
        a.clone(),
        "GET",
        "/api/v1/conversion-events?external_id=c1",
        &w.key,
        Value::Null,
    )
    .await;
    let ms = &list["data"][0]["delivery"]["microsoft"];
    assert_eq!(ms["status"], "blocked");
    assert!(
        ms["reason"]
            .as_str()
            .unwrap()
            .contains("2 hours after it is created"),
        "{ms}"
    );
    // Once the wait is over, it is sent.
    sqlx::query("UPDATE microsoft_created_goals SET created_at=now()-interval '3 hours' WHERE workspace_id=$1").bind(a.store.workspace).execute(&a.store.pool).await.unwrap();
    sqlx::query("UPDATE microsoft_event_plans SET plan_after=now() WHERE workspace_id=$1")
        .bind(a.store.workspace)
        .execute(&a.store.pool)
        .await
        .unwrap();
    a.store.plan_microsoft(None).await.unwrap();
    assert_eq!(drain(&a).await, 1);
    assert!(m.applies.lock().unwrap()[0].contains("ms-consent"));
    // Calls attributed by DNI timing follow the shared allow_inferred_calls setting.
    sqlx::query("UPDATE leads SET channel='call' WHERE id=$1")
        .bind(lead)
        .execute(&a.store.pool)
        .await
        .unwrap();
    let mut revised = body.clone();
    revised["external_id"] = json!("c2");
    let (_, v) = post(&w, revised).await;
    assert_eq!(v["delivery"]["microsoft"]["status"], "blocked", "{v}");
    assert!(
        v["delivery"]["microsoft"]["reason"]
            .as_str()
            .unwrap()
            .contains("allow_inferred_calls")
    );
    task.abort();
    w.task.abort();
}
