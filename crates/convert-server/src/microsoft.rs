//! Microsoft Advertising as a second destination for pipeline conversions: owner-only
//! OAuth (Microsoft identity platform v2 with PKCE), ad account and goal discovery, the
//! read-only destination check and review, goal creation and mapping, and the worker that
//! sends offline conversions and their adjustments.
//!
//! The developer token is an installation secret. Customers only sign in with Microsoft;
//! they never see or enter it.
use super::*;
use axum::{extract::Query, http::header, response::Redirect};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use convert_adapters::microsoft_api::{self, DeliveryError, GoalSpec};
use convert_store::{api_keys::KeyAccess, google::Attempt, microsoft::Connection};
use serde::Deserialize;
use sha2::{Digest, Sha256};
const CALLBACK: &str = "/auth/microsoft/callback";
const COOKIE: &str = "vc_microsoft_browser";

#[derive(Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub developer_token: String,
    pub redirect: String,
}
impl Config {
    /// All three variables, or none. Server-side only; never sent to a browser or agent.
    pub fn from_env(origin: &str, vault: bool) -> anyhow::Result<Option<Self>> {
        let get = |name| std::env::var(name).ok().filter(|s: &String| !s.is_empty());
        match (
            get("VISICONVERT_MICROSOFT_CLIENT_ID"),
            get("VISICONVERT_MICROSOFT_CLIENT_SECRET"),
            get("VISICONVERT_MICROSOFT_DEVELOPER_TOKEN"),
        ) {
            (None, None, None) => Ok(None),
            (Some(id), Some(secret), Some(developer)) => {
                anyhow::ensure!(vault, "Microsoft OAuth requires VISICONVERT_ENCRYPTION_KEY");
                Self::new(&hosted::origin(origin)?, id, secret, developer).map(Some)
            }
            _ => anyhow::bail!(
                "Set all of VISICONVERT_MICROSOFT_CLIENT_ID, VISICONVERT_MICROSOFT_CLIENT_SECRET and VISICONVERT_MICROSOFT_DEVELOPER_TOKEN, or none"
            ),
        }
    }
    pub fn new(
        origin: &str,
        client_id: String,
        client_secret: String,
        developer_token: String,
    ) -> anyhow::Result<Self> {
        let guid = client_id.len() == 36
            && client_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            });
        anyhow::ensure!(
            guid,
            "Invalid Microsoft OAuth client ID (expected the application GUID)"
        );
        anyhow::ensure!(
            (16..=256).contains(&client_secret.len())
                && client_secret.bytes().all(|b| b.is_ascii_graphic()),
            "Invalid Microsoft OAuth client secret"
        );
        anyhow::ensure!(
            (6..=64).contains(&developer_token.len())
                && developer_token.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid Microsoft Advertising developer token"
        );
        Ok(Self {
            client_id,
            client_secret,
            developer_token,
            redirect: format!("{origin}{CALLBACK}"),
        })
    }
    fn fingerprint(&self) -> String {
        hosted::digest(&format!("{}\n{}", self.client_id, self.redirect))
    }
    fn url(&self, state: &str, verifier: &str) -> String {
        let mut u = url::Url::parse(microsoft_api::AUTHORIZE).unwrap();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        u.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", self.redirect.as_str()),
            ("response_mode", "query"),
            ("scope", microsoft_api::SCOPE),
            ("prompt", "select_account"),
            ("state", state),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]);
        u.into()
    }
}
/// Optional configuration plus the provider client, kept together on the App.
#[derive(Clone)]
pub struct Microsoft {
    pub config: Option<Config>,
    pub client: microsoft_api::Client,
}
impl Default for Microsoft {
    fn default() -> Self {
        Self {
            config: None,
            client: microsoft_api::Client::new().expect("HTTP client"),
        }
    }
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn config(a: &App) -> Result<&Config, ApiError> {
    a.microsoft.config.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Microsoft Advertising is not available on this installation. The administrator must configure it first.".into(),
        )
    })
}
async fn owner(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(h) = &access.session_hash
        && !access.store.manages_connection(h).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can manage the Microsoft Advertising connection".into(),
        ));
    }
    Ok(())
}
fn bad(m: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, m.into())
}
impl From<microsoft_api::Error> for ApiError {
    fn from(e: microsoft_api::Error) -> Self {
        use microsoft_api::Error::*;
        Self(
            match e {
                GoalNameExists => StatusCode::CONFLICT,
                Credentials | Scope | Access => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            },
            e.to_string(),
        )
    }
}
/// Owner routes under /api. The destination, check, review and goal routes are shared
/// with keys and MCP (see `shared_routes`).
pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/connections/microsoft", get(show).delete(disconnect))
        .route("/connections/microsoft/start", post(start))
        .route("/connections/microsoft/accounts", get(accounts))
        .route("/connections/microsoft/goals", get(goals))
        .layer(middleware::from_fn(
            |request: Request, next: Next| async move {
                let mut response = next.run(request).await;
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
                response
            },
        ))
}
/// Read-only discovery under /api/v1 (scope read).
pub fn public_routes() -> Router<Shared> {
    Router::new()
        .route("/connections/microsoft/accounts", get(accounts))
        .route("/connections/microsoft/goals", get(goals))
}
/// Mounted for owners (/api) and for keys and MCP (/api/v1).
pub fn shared_routes() -> Router<Shared> {
    Router::new()
        .route(
            "/conversion-setup/microsoft-destination",
            axum::routing::put(destination_route),
        )
        .route("/conversion-setup/microsoft-check", post(check_route))
        .route("/conversion-setup/microsoft-review", get(review_route))
        .route(
            "/conversion-setup/microsoft-goal",
            axum::routing::put(goal_settings_route),
        )
        .route(
            "/conversion-actions/{key}/microsoft-goal",
            post(create_goal_route).put(map_goal_route),
        )
}
/// Connected with the OAuth client this installation is configured with now.
pub(crate) fn connected(a: &App, c: &Connection) -> bool {
    c.encrypted_token.is_some()
        && a.vault.is_some()
        && a.microsoft
            .config
            .as_ref()
            .is_some_and(|g| c.client_fingerprint.as_deref() == Some(g.fingerprint().as_str()))
}
async fn show(State(a): State<Shared>, Extension(access): Extension<hosted::Access>) -> Api<Value> {
    owner(&access).await?;
    let c = access.store.microsoft_connection().await?;
    let configured = a.microsoft.config.is_some() && a.vault.is_some();
    let matching = connected(&a, &c);
    Ok(Json(json!({"configured":configured,"connected":matching,
        "reconnect_required":c.encrypted_token.is_some() && !matching,"revision":c.revision,"connected_at":c.connected_at,
        "redirect_uri":format!("{}{CALLBACK}",hosted::browser_origin(&a)),
        "customer_id":c.customer_id,"account_id":c.account_id,"account_name":c.account_name,"destination_version":c.destination_version,
        "validation":if matching {c.validation.clone()} else {None},"checked_at":if matching {c.checked_at} else {None},
        "check_current":matching && c.check_current()})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: i64,
}
fn browser_cookie(a: &App, value: &str, age: u32) -> String {
    format!(
        "{COOKIE}={value}; Path={CALLBACK}; HttpOnly; SameSite=Lax; Max-Age={age}{}",
        if hosted::browser_origin(a).starts_with("https://") {
            "; Secure"
        } else {
            ""
        }
    )
}
async fn start(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Revision>,
) -> Result<Response, ApiError> {
    owner(&access).await?;
    let g = config(&a)?;
    let vault = a.vault.as_ref().ok_or_else(connections::vault_error)?;
    let state = random();
    let browser = random();
    let verifier = random();
    let state_hash = hosted::digest(&state);
    let encrypted = vault
        .seal_microsoft(
            access.store.workspace,
            &format!("pkce:{state_hash}"),
            &verifier,
        )
        .map_err(|_| connections::vault_error())?;
    let attempt = Attempt {
        workspace: access.store.workspace,
        state_hash,
        browser_hash: hosted::digest(&browser),
        encrypted_verifier: encrypted,
        client_fingerprint: g.fingerprint(),
        session_hash: access.session_hash.clone(),
        operator_hash: access
            .session_hash
            .is_none()
            .then(|| hosted::digest(&a.api_key)),
        actor: access
            .store
            .inquiry_actor(access.session_hash.as_deref())
            .await?,
        expected_revision: input.revision,
    };
    access.store.begin_microsoft_oauth(&attempt).await?;
    Ok((
        [(header::SET_COOKIE, browser_cookie(&a, &browser, 600))],
        Json(json!({"authorization_url":g.url(&state,&verifier)})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}
pub async fn callback(
    State(a): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<Callback>,
) -> Response {
    let result = complete(&a, &headers, query).await;
    // Never echo codes, upstream descriptions or tokens into HTML/URLs/logs.
    let status = match result {
        Ok(()) => "connected",
        Err(_) => "failed",
    };
    let mut response =
        Redirect::to(&format!("/auth/microsoft/result?status={status}")).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        browser_cookie(&a, "", 0).parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}
pub async fn result(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Html<&'static str> {
    axum::response::Html(
        if q.get("status").map(String::as_str) == Some("connected") {
            "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Microsoft Advertising connected — VisiConvert</title><h1>Microsoft Advertising connected</h1><p>Return to VisiConvert to choose the ad account and map your actions to goals. No conversions were uploaded. You can close this window.</p><script src='/auth/microsoft/result.js' defer></script></html>"
        } else {
            "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Microsoft Advertising connection — VisiConvert</title><h1>Microsoft Advertising could not be connected</h1><p>The request expired, was canceled, or could not be verified. Return to VisiConvert and try again, allowing access to your Microsoft Advertising accounts. You can close this window.</p><script src='/auth/microsoft/result.js' defer></script></html>"
        },
    )
}
pub async fn result_script() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        "if (window.opener) { window.opener.postMessage({type:'visiconvert-microsoft-result',status:new URLSearchParams(location.search).get('status')},location.origin); window.close(); }",
    )
}
async fn complete(a: &App, headers: &HeaderMap, q: Callback) -> Result<(), ApiError> {
    let invalid = || {
        bad("Microsoft connection expired, was canceled, or could not be verified. Start again.")
    };
    let state = q.state.filter(|s| hash64(s)).ok_or_else(invalid)?;
    let cookies: Vec<_> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|s| s.split(';'))
        .filter_map(|s| s.trim().strip_prefix(&format!("{COOKIE}=")))
        .collect();
    if cookies.len() != 1 || !hash64(cookies[0]) {
        return Err(invalid());
    }
    let attempt = a
        .store
        .consume_microsoft_oauth(&hosted::digest(&state), &hosted::digest(cookies[0]))
        .await?
        .ok_or_else(invalid)?;
    let g = config(a)?;
    if q.error.is_some() || attempt.client_fingerprint != g.fingerprint() {
        return Err(invalid());
    }
    let code = q
        .code
        .filter(|s| !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control))
        .ok_or_else(invalid)?;
    let store = Store {
        pool: a.store.pool.clone(),
        workspace: attempt.workspace,
    };
    if let Some(hash) = &attempt.session_hash {
        if a.cloud.is_none() || !store.manages_connection(hash).await? {
            return Err(invalid());
        }
    } else if attempt.workspace != a.store.workspace
        || attempt.operator_hash.as_deref() != Some(hosted::digest(&a.api_key).as_str())
    {
        return Err(invalid());
    }
    let vault = a.vault.as_ref().ok_or_else(connections::vault_error)?;
    let verifier = vault
        .open_microsoft(
            store.workspace,
            &format!("pkce:{}", attempt.state_hash),
            &attempt.encrypted_verifier,
        )
        .map_err(|_| connections::vault_error())?;
    let tokens = a
        .microsoft
        .client
        .exchange(
            &g.client_id,
            &g.client_secret,
            &g.redirect,
            &code,
            &verifier,
        )
        .await?;
    let refresh = tokens
        .refresh_token
        .ok_or(microsoft_api::Error::Credentials)?;
    let encrypted = vault
        .seal_microsoft(store.workspace, "refresh", &refresh)
        .map_err(|_| connections::vault_error())?;
    store.finish_microsoft_oauth(&attempt, &encrypted).await?;
    Ok(())
}
/// A fresh access token. Microsoft rotates refresh tokens, so every replacement is sealed
/// and saved atomically before the access token is used.
pub(crate) async fn worker_token(a: &App, store: &Store) -> Result<(Connection, String), ApiError> {
    let g = config(a)?;
    let c = store.microsoft_connection().await?;
    if c.client_fingerprint.as_deref() != Some(g.fingerprint().as_str()) {
        return Err(microsoft_api::Error::Credentials.into());
    }
    let vault = a.vault.as_ref().ok_or_else(connections::vault_error)?;
    let encrypted = c
        .encrypted_token
        .as_deref()
        .ok_or(microsoft_api::Error::Credentials)?;
    let refresh = vault
        .open_microsoft(store.workspace, "refresh", encrypted)
        .map_err(|_| connections::vault_error())?;
    let t = a
        .microsoft
        .client
        .refresh(&g.client_id, &g.client_secret, &refresh)
        .await?;
    if let Some(next) = t.refresh_token
        && next != refresh
    {
        let sealed = vault
            .seal_microsoft(store.workspace, "refresh", &next)
            .map_err(|_| connections::vault_error())?;
        store
            .rotate_microsoft_refresh(c.revision, encrypted, &sealed)
            .await?;
    }
    Ok((c, t.access_token))
}
async fn token(a: &App, access: &hosted::Access) -> Result<(Connection, String), ApiError> {
    owner(access).await?;
    worker_token(a, &access.store).await
}
fn developer(a: &App) -> Result<&str, ApiError> {
    Ok(config(a)?.developer_token.as_str())
}
async fn accounts(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    let (_, token) = token(&a, &access).await?;
    let (accounts, more) = a.microsoft.client.accounts(&token, developer(&a)?).await?;
    Ok(Json(json!({"accounts":accounts,"has_more":more})))
}
#[derive(Deserialize)]
struct GoalQuery {
    account_id: Option<String>,
}
/// Offline conversion goals in the saved ad account, or in another reachable one.
async fn goals(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Query(q): Query<GoalQuery>,
) -> Api<Value> {
    let (c, token) = token(&a, &access).await?;
    let dev = developer(&a)?;
    let account_id = q
        .account_id
        .or(c.account_id.clone())
        .ok_or_else(|| bad("Choose the Microsoft Advertising account first, or pass account_id from list_microsoft_accounts."))?;
    if !microsoft_api::numeric_id(&account_id) {
        return Err(bad(
            "account_id must be a numeric Microsoft Advertising account ID",
        ));
    }
    let account = a
        .microsoft
        .client
        .account(&token, dev, &account_id)
        .await?
        .ok_or(microsoft_api::Error::Access)?;
    let goals = a
        .microsoft
        .client
        .goals(&token, dev, &account.customer_id, &account.id, false)
        .await?;
    Ok(Json(json!({"account":account,"goals":goals})))
}
async fn disconnect(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Revision>,
) -> Api<Value> {
    owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access
        .store
        .disconnect_microsoft(input.revision, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn actor(
    access: &hosted::Access,
    key: &Option<Extension<KeyAccess>>,
) -> Result<String, ApiError> {
    Ok(match key {
        Some(k) => k.actor.clone(),
        None => {
            access
                .store
                .inquiry_actor(access.session_hash.as_deref())
                .await?
        }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Destination {
    /// connection_revision from the checklist.
    revision: i64,
    /// destination_version from the checklist (0 before the first choice).
    destination_version: i32,
    account_id: String,
}
/// conversions:send. The ad account that receives Microsoft conversions. It must be
/// reachable by the connected Microsoft user; the CustomerId to operate from is derived.
async fn destination_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Destination>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    if !microsoft_api::numeric_id(&input.account_id) {
        return Err(bad(
            "account_id must be a numeric Microsoft Advertising account ID",
        ));
    }
    let (c, token) = token(&a, &access).await?;
    if c.revision != input.revision || c.destination_version != input.destination_version {
        return Err(convert_store::Error::Conflict.into());
    }
    let account = a
        .microsoft
        .client
        .account(&token, developer(&a)?, &input.account_id)
        .await?
        .ok_or_else(|| bad("This ad account is not reachable from the connected Microsoft user. Use an account from list_microsoft_accounts."))?;
    let version = access
        .store
        .save_microsoft_destination(
            input.revision,
            input.destination_version,
            &account.customer_id,
            &account.id,
            &account.name,
            &actor,
        )
        .await?;
    Ok(Json(
        json!({"account":account,"destination_version":version,
        "setup":pipeline::setup_status(&a, &access.store).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    revision: i64,
    destination_version: i32,
}
/// conversions:write. Read-only at Microsoft: the saved account and every offline goal,
/// recorded against the connection revision and destination version it covered.
async fn check_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Check>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let (c, token) = token(&a, &access).await?;
    if c.revision != input.revision || c.destination_version != input.destination_version {
        return Err(convert_store::Error::Conflict.into());
    }
    if c.account_id.is_none() {
        return Err(bad(
            "Choose the Microsoft Advertising account first (set_microsoft_destination).",
        ));
    }
    let report = destination_report(&a, &access.store, &token, &c).await?;
    access
        .store
        .save_microsoft_validation(c.revision, c.destination_version, &report, &actor)
        .await?;
    Ok(Json(report))
}
/// Enabled actions mapped to a Microsoft goal: (key, goal name).
async fn mapped(store: &Store) -> Result<Vec<(String, String)>, ApiError> {
    Ok(store
        .conversion_actions()
        .await?
        .into_iter()
        .filter(|v| v["enabled"] == true)
        .filter_map(|v| {
            Some((
                v["key"].as_str()?.to_string(),
                v["microsoft_goal_name"].as_str()?.to_string(),
            ))
        })
        .collect())
}
pub(crate) async fn destination_report(
    a: &App,
    store: &Store,
    token: &str,
    c: &Connection,
) -> Result<Value, ApiError> {
    let dev = developer(a)?;
    let (customer, account_id) = (
        c.customer_id.clone().unwrap_or_default(),
        c.account_id.clone().unwrap_or_default(),
    );
    let mapped = mapped(store).await?;
    let result = async {
        let account = a
            .microsoft
            .client
            .account(token, dev, &account_id)
            .await?
            .ok_or(microsoft_api::Error::Access)?;
        let goals = a
            .microsoft
            .client
            .goals(token, dev, &customer, &account_id, false)
            .await?;
        Ok::<_, microsoft_api::Error>((account, goals))
    }
    .await;
    let waits = store.microsoft_goal_waits(&account_id).await?;
    Ok(match result {
        Ok((account, goals)) => {
            let rows: Vec<Value> = mapped
                .iter()
                .map(|(key, name)| {
                    let goal = goals.iter().find(|g| &g.name == name);
                    json!({"action":key,"goal_name":name,"found":goal.is_some(),"eligible":goal.is_some_and(|g| g.eligible),
                        "accepts_uploads_after":waits.iter().find(|(n, _)| n == name).map(|(_, t)| t)})
                })
                .collect();
            let all = !mapped.is_empty() && rows.iter().all(|r| r["eligible"] == true);
            let valid = account.status == "Active" && all;
            json!({"valid":valid,"account":{"id":account.id,"name":account.name,"number":account.number,"status":account.status,"currency":account.currency,"customer_id":customer},
                "goals":goals,"mapped":rows,"destination_version":c.destination_version,
                "message":if valid {"Account is readable and active, and every mapped goal is an active offline conversion goal in it. Upload acceptance and attribution are not checked."}
                    else if account.status != "Active" {"The ad account is not active in Microsoft Advertising."}
                    else if mapped.is_empty() {"Map at least one action to a Microsoft goal before checking the destination."}
                    else {"A mapped goal is missing, paused or not an offline conversion goal in this account. Check the exact goal names."}})
        }
        Err(e) => {
            json!({"valid":false,"destination_version":c.destination_version,"message":e.to_string()})
        }
    })
}
/// Worker refresh of a missing or expired check.
async fn refresh_destination(a: &App, store: &Store) -> Result<(), ApiError> {
    let (c, token) = worker_token(a, store).await?;
    if c.account_id.is_none() {
        return Ok(());
    }
    let report = destination_report(a, store, &token, &c).await?;
    store
        .save_microsoft_validation(c.revision, c.destination_version, &report, "worker")
        .await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MapGoal {
    /// Current version of the VisiConvert action.
    version: i32,
    /// Exact name of a Microsoft offline conversion goal, or null to clear.
    goal_name: Option<String>,
    /// Owner's choice: also send events and Microsoft-click leads since this time (at
    /// most 90 days ago). Only for clicks no other uploader sent, or they count twice.
    #[serde(default)]
    include_since: Option<chrono::DateTime<chrono::Utc>>,
}
/// conversions:write. Uploads reference goals by name.
async fn map_goal_route(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(path): Path<String>,
    Json(input): Json<MapGoal>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let name = input.goal_name.as_deref().filter(|s| !s.is_empty());
    Ok(Json(
        access
            .store
            .set_microsoft_action_goal(&path, name, input.include_since, input.version, &actor)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateGoal {
    version: i32,
    /// Optional offline goal category; defaults from the action's counting.
    category: Option<String>,
}
/// conversions:write. Create, or recover after an interrupted attempt, a VisiConvert
/// offline conversion goal in the saved ad account and map the action to it. Never
/// replaces a mapping.
async fn create_goal_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(path): Path<String>,
    Json(input): Json<CreateGoal>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    Ok(Json(
        create_goal(&a, &access.store, &path, &input, &actor).await?,
    ))
}
pub(crate) async fn create_goal(
    a: &App,
    store: &Store,
    key: &str,
    input: &CreateGoal,
    actor: &str,
) -> Result<Value, ApiError> {
    let current = store
        .conversion_actions()
        .await?
        .into_iter()
        .find(|v| v["key"] == key)
        .ok_or(convert_store::Error::NotFound)?;
    if current["version"].as_i64() != Some(input.version.into()) {
        return Err(convert_store::Error::Conflict.into());
    }
    if current["microsoft_goal_name"].is_string() {
        return Err(ApiError(StatusCode::CONFLICT, "This action is already mapped to a Microsoft goal. Clear the mapping first to create a new one.".into()));
    }
    let c = store.microsoft_connection().await?;
    let (Some(customer), Some(account)) = (c.customer_id.clone(), c.account_id.clone()) else {
        return Err(bad(
            "The owner must choose the Microsoft Advertising account first.",
        ));
    };
    let name = format!("VisiConvert - {key} ({})", store.workspace);
    let spec = GoalSpec::pipeline(
        &name,
        current["counting"].as_str().unwrap_or(""),
        input.category.as_deref(),
    )
    .ok_or_else(|| {
        bad(&format!(
            "category must be one of {}",
            microsoft_api::GOAL_CATEGORIES.join(", ")
        ))
    })?;
    let (_, token) = worker_token(a, store).await?;
    let dev = developer(a)?;
    let client = &a.microsoft.client;
    let (customer, account) = (&customer, &account);
    let list = || client.goals(&token, dev, customer, account, false);
    let goals = list().await?;
    let known = store
        .created_microsoft_goal(c.account_id.as_deref().unwrap(), key)
        .await?;
    let existing = known
        .and_then(|(id, _, _)| goals.iter().find(|g| g.id == id))
        .or_else(|| goals.iter().find(|g| g.name == name))
        .cloned();
    let (goal_id, goal_name, created) = match existing {
        Some(g) => (g.id, g.name, false),
        None => {
            let result = a
                .microsoft
                .client
                .add_goal(
                    &token,
                    dev,
                    c.customer_id.as_deref().unwrap(),
                    c.account_id.as_deref().unwrap(),
                    &spec,
                )
                .await;
            match result {
                Ok(id) => (id, name.clone(), true),
                Err(
                    microsoft_api::Error::GoalNameExists | microsoft_api::Error::CreationUncertain,
                ) => {
                    // An earlier or lost attempt may have created it; recover it by name.
                    let g = list()
                        .await?
                        .into_iter()
                        .find(|g| g.name == name)
                        .ok_or(microsoft_api::Error::CreationUncertain)?;
                    (g.id, g.name, true)
                }
                Err(e) => return Err(e.into()),
            }
        }
    };
    store
        .remember_microsoft_goal(
            c.account_id.as_deref().unwrap(),
            key,
            &goal_id,
            &goal_name,
            actor,
        )
        .await
        .or_else(|e| match e {
            // Already recorded for this goal (a recovered attempt).
            convert_store::Error::Conflict if !created => Ok(()),
            e => Err(e),
        })?;
    let action = store
        .set_microsoft_action_goal(key, Some(&goal_name), None, input.version, actor)
        .await?;
    let ready = store
        .created_microsoft_goal(c.account_id.as_deref().unwrap(), key)
        .await?
        .map(|(_, _, t)| t + convert_store::microsoft::GOAL_READY);
    Ok(
        json!({"action":action,"goal":{"id":goal_id,"name":goal_name},"created":created,
        "accepts_uploads_after":ready,
        "notes":["Microsoft accepts offline conversions for a new goal about 2 hours after it is created; VisiConvert waits and sends after that.",
            "Creating an offline conversion goal turns on Microsoft click ID (msclkid) auto-tagging for the account.",
            "New goals count toward conversions used for bidding. Change that in Microsoft Advertising if this goal should only be reported."]}),
    )
}

/// Read-only review of the saved ad account's conversion goals: which ones VisiConvert
/// sends to, and which others could count the same leads (another offline uploader, or a
/// website goal for the same outcome).
async fn review_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let (c, token) = token(&a, &access).await?;
    let (Some(customer), Some(account)) = (c.customer_id.clone(), c.account_id.clone()) else {
        return Err(bad(
            "Choose the Microsoft Advertising account first (set_microsoft_destination).",
        ));
    };
    let goals = a
        .microsoft
        .client
        .goals(&token, developer(&a)?, &customer, &account, true)
        .await?;
    let mapped = mapped(&access.store).await?;
    let categories: Vec<String> = goals
        .iter()
        .filter(|g| mapped.iter().any(|(_, n)| *n == g.name))
        .map(|g| g.category.clone())
        .collect();
    let actions = access.store.conversion_actions().await?;
    let mut warnings = Vec::new();
    let rows: Vec<Value> = goals
        .iter()
        .map(|g| {
            let used: Vec<&str> = mapped
                .iter()
                .filter(|(_, n)| *n == g.name)
                .map(|(k, _)| k.as_str())
                .collect();
            let mut flags: Vec<&str> = Vec::new();
            let active = g.status == "Active";
            if !used.is_empty() {
                if !g.eligible {
                    flags.push("mapped_goal_not_accepting");
                }
                for key in &used {
                    let counting = actions
                        .iter()
                        .find(|v| v["key"] == *key)
                        .and_then(|v| v["counting"].as_str())
                        .unwrap_or("");
                    let expected = if counting == "once_per_click" { "Unique" } else { "All" };
                    if !g.count_type.is_empty() && g.count_type != expected {
                        flags.push("count_type_differs");
                    }
                }
                if g.window_minutes > 0 && g.window_minutes < 129_600 {
                    flags.push("short_window");
                }
            } else if active && g.kind == "OfflineConversion" {
                flags.push("other_offline_goal");
            } else if active
                && !g.exclude_from_bidding
                && !g.category.is_empty()
                && categories.contains(&g.category)
            {
                flags.push("website_goal_same_category");
            }
            if flags.iter().any(|f| *f != "short_window") {
                warnings.push(json!({"id":g.id,"name":g.name,"flags":flags}));
            }
            json!({"id":g.id,"name":g.name,"type":g.kind,"status":g.status,"category":g.category,"count_type":g.count_type,
                "window_minutes":g.window_minutes,"exclude_from_bidding":g.exclude_from_bidding,"used_by":used,"flags":flags})
        })
        .collect();
    let summary = if warnings.is_empty() {
        "No other active goal in this account looks like it counts the same leads.".to_string()
    } else {
        format!(
            "{} goal(s) need a look: another offline goal may be fed by a different uploader, or a website goal may count the same outcome. After the owner confirms each one, pause it or exclude it from bidding with set_microsoft_goal (conversions:send).",
            warnings.len()
        )
    };
    Ok(Json(
        json!({"account_id":account,"goals":rows,"warnings":warnings,"summary":summary,
        "flags":{"other_offline_goal":"An active offline goal VisiConvert does not send to; another uploader (CRM, script, older integration) may send the same leads to it.",
            "website_goal_same_category":"An active website (UET) goal in the same category as a mapped goal, counted for bidding; it may count the same leads.",
            "mapped_goal_not_accepting":"A mapped goal is paused, deleted or not an offline conversion goal.",
            "count_type_differs":"The goal's count type differs from the action's counting rule (Unique for once_per_click, All for every).",
            "short_window":"The goal's conversion window is shorter than 90 days; later events are not sent."}}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalSettings {
    goal_id: String,
    /// Active or Paused. Deleting is not offered: it cannot be undone.
    status: Option<String>,
    /// true excludes the goal from the Conversions columns and automated bidding.
    exclude_from_bidding: Option<bool>,
}
/// conversions:send. Pause or reactivate a conversion goal in the saved ad account, or
/// include or exclude it from bidding. Goals VisiConvert sends to cannot be paused.
async fn goal_settings_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<GoalSettings>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    if !microsoft_api::numeric_id(&input.goal_id) {
        return Err(bad(
            "goal_id must be a goal id from review_microsoft_conversion_setup.",
        ));
    }
    if input
        .status
        .as_deref()
        .is_some_and(|s| !matches!(s, "Active" | "Paused"))
    {
        return Err(bad("status must be Active or Paused."));
    }
    if input.status.is_none() && input.exclude_from_bidding.is_none() {
        return Err(bad("Pass status, exclude_from_bidding, or both."));
    }
    let actor = actor(&access, &key).await?;
    let (c, token) = token(&a, &access).await?;
    let (Some(customer), Some(account)) = (c.customer_id.clone(), c.account_id.clone()) else {
        return Err(bad(
            "Choose the Microsoft Advertising account first (set_microsoft_destination).",
        ));
    };
    let dev = developer(&a)?;
    let client = &a.microsoft.client;
    let find = |goals: Vec<microsoft_api::Goal>| goals.into_iter().find(|g| g.id == input.goal_id);
    let goal = find(client.goals(&token, dev, &customer, &account, true).await?).ok_or(
        ApiError(
            StatusCode::NOT_FOUND,
            "No conversion goal with this ID in the saved ad account. Use an id from review_microsoft_conversion_setup.".into(),
        ),
    )?;
    if !matches!(goal.status.as_str(), "Active" | "Paused") {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("This goal is {} and cannot be changed.", goal.status),
        ));
    }
    let used: Vec<String> = access
        .store
        .conversion_actions()
        .await?
        .into_iter()
        .filter(|v| v["microsoft_goal_name"] == goal.name.as_str())
        .filter_map(|v| v["key"].as_str().map(str::to_string))
        .collect();
    let status = input.status.as_deref().filter(|s| *s != goal.status);
    if status == Some("Paused") && !used.is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "VisiConvert sends {} to this goal, and a paused goal rejects uploads. Clear the mapping with set_microsoft_action_goal first.",
                used.join(", ")
            ),
        ));
    }
    let exclude = input
        .exclude_from_bidding
        .filter(|e| *e != goal.exclude_from_bidding);
    let changed = status.is_some() || exclude.is_some();
    let after = if changed {
        client
            .update_goal(
                &token,
                dev,
                &customer,
                &account,
                &goal,
                microsoft_api::GoalChange {
                    status,
                    exclude_from_bidding: exclude,
                },
            )
            .await?;
        let after = find(client.goals(&token, dev, &customer, &account, true).await?)
            .ok_or(microsoft_api::Error::Response)?;
        access
            .store
            .log_microsoft_goal_change(
                &actor,
                &json!({"account_id":account,"goal_id":goal.id,"name":goal.name,"type":goal.kind,
                    "status_before":goal.status,"status":after.status,
                    "exclude_from_bidding_before":goal.exclude_from_bidding,"exclude_from_bidding":after.exclude_from_bidding}),
            )
            .await?;
        after
    } else {
        goal.clone()
    };
    Ok(Json(
        json!({"goal":{"id":after.id,"name":after.name,"type":after.kind,"status":after.status,
            "category":after.category,"exclude_from_bidding":after.exclude_from_bidding,"used_by":used},
        "before":{"status":goal.status,"exclude_from_bidding":goal.exclude_from_bidding},
        "changed":changed,
        "notes":["Pausing stops the goal recording new conversions; conversions already recorded stay in reports.",
            "Excluding a goal from bidding removes it from the Conversions columns and automated bidding; All conversions still include it."]}),
    ))
}

/// The Microsoft section of the setup checklist. Additive: Google's `ready`,
/// `next_steps` and `summary` are unchanged.
pub(crate) async fn setup_status(a: &App, store: &Store) -> Result<Value, ApiError> {
    let mut f = store.microsoft_setup().await?;
    let c = store.microsoft_connection().await?;
    let configured = a.microsoft.config.is_some() && a.vault.is_some();
    let is_connected = connected(a, &c);
    let policy = store.pipeline_policy().await?;
    let n = |k: &str| f[k].as_i64().unwrap_or(0);
    let report = c.validation.clone().unwrap_or_default();
    let mut steps: Vec<(&str, String)> = Vec::new();
    if !configured {
        steps.push(("administrator", "Microsoft Advertising is not available on this installation; the administrator must configure it (see the Microsoft connection guide).".into()));
    } else if !is_connected {
        steps.push(("owner", "Owner must connect Microsoft Advertising in Conversions. Keys and agents cannot start a Microsoft connection.".into()));
    } else if c.account_id.is_none() {
        steps.push(("owner_approval", "Choose the Microsoft Advertising account: the owner in Conversions, or an agent with conversions:send (set_microsoft_destination) after the owner confirms the account name and ID.".into()));
    }
    if n("mapped_actions") == 0 {
        steps.push(("agent", "Map at least one enabled action to a Microsoft offline conversion goal (set_microsoft_action_goal), or create a VisiConvert goal for it (create_microsoft_goal).".into()));
    }
    if is_connected && c.account_id.is_some() && n("mapped_actions") > 0 {
        if c.checked_at.is_none() {
            steps.push(("agent", "Run the Microsoft destination check once (check_microsoft_destination, or POST /api/v1/conversion-setup/microsoft-check) with revision and destination_version from this checklist; VisiConvert repeats it automatically after that.".into()));
        } else if report["valid"] != true && c.check_current() {
            steps.push((
                "owner",
                format!(
                    "Microsoft destination check needs attention: {}",
                    report["message"]
                        .as_str()
                        .unwrap_or("check the account and goals")
                ),
            ));
        }
    }
    if policy["enabled"] != true || policy["auto_send"] != true {
        steps.push(("owner_approval", "Turn on delivery and automatic sending (shared with Google): the owner in Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner confirms that no other uploader sends the same conversions.".into()));
    }
    let mut advisory: Vec<(&str, String)> = Vec::new();
    if let Some(account) = &c.account_id {
        for (name, ready) in store.microsoft_goal_waits(account).await? {
            advisory.push(("agent", format!("Microsoft accepts uploads for the new goal \"{name}\" after {}; eligible events wait until then.", ready.to_rfc3339())));
        }
    }
    if f["dni_calls"] == true && f["allow_inferred_calls"] != true && n("mapped_actions") > 0 {
        advisory.push(("owner_approval", "Calls matched to a website visit by timing on a DNI number (inferred calls) are not sent to Microsoft either until the owner allows them (set_google_destination allow_inferred_calls).".into()));
    }
    if is_connected && c.account_id.is_some() {
        advisory.push(("agent", "Run review_microsoft_conversion_setup (GET /api/v1/conversion-setup/microsoft-review) to find other goals in the Microsoft Advertising account that could count the same leads.".into()));
    }
    let ready = steps.is_empty();
    f["configured"] = json!(configured);
    f["connected"] = json!(is_connected);
    f["reconnect_required"] = json!(c.encrypted_token.is_some() && !is_connected);
    f["connection_revision"] = json!(c.revision);
    f["destination_version"] = json!(c.destination_version);
    f["customer_id"] = json!(c.customer_id);
    f["account_id"] = json!(c.account_id);
    f["account_name"] = json!(c.account_name);
    f["checked_at"] = json!(c.checked_at);
    f["check_current"] = json!(is_connected && c.check_current());
    f["check_valid"] = json!(report["valid"] == true);
    f["check_message"] = report["message"].clone();
    f["ready"] = json!(ready);
    f["next_steps"] = json!(
        steps
            .iter()
            .map(|(who, text)| json!({"actor":who,"step":text}))
            .chain(
                advisory
                    .iter()
                    .map(|(who, text)| json!({"actor":who,"step":text,"advisory":true}))
            )
            .collect::<Vec<_>>()
    );
    f["summary"] = json!(if ready {
        "Ready: eligible pipeline events with a Microsoft click are sent to Microsoft Advertising automatically.".to_string()
    } else {
        steps[0].1.clone()
    });
    Ok(f)
}

/// At most one automatic destination check per workspace every 10 minutes.
static REFRESHED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, std::time::Instant>>,
> = std::sync::LazyLock::new(Default::default);
async fn refresh(a: &App, store: &Store) -> Result<(), convert_store::Error> {
    if !store.microsoft_refresh_due().await? {
        return Ok(());
    }
    {
        let mut seen = REFRESHED.lock().unwrap();
        if seen
            .get(&store.workspace)
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(600))
        {
            return Ok(());
        }
        seen.insert(store.workspace, std::time::Instant::now());
    }
    if refresh_destination(a, store).await.is_err() {
        tracing::warn!(workspace = %store.workspace, "Automatic Microsoft destination check failed");
    }
    Ok(())
}
/// Record new_lead events, refresh expired checks, then plan due Microsoft revisions.
pub(crate) async fn plan(a: &App) -> Result<(), convert_store::Error> {
    for workspace in a.store.microsoft_workspaces().await? {
        let store = Store {
            pool: a.store.pool.clone(),
            workspace,
        };
        store.create_lead_events().await?;
        refresh(a, &store).await?;
        store.plan_microsoft(None).await?;
    }
    Ok(())
}
fn backoff(attempt: i64) -> i64 {
    (30_i64.saturating_mul(1_i64 << attempt.clamp(0, 7))).min(3600)
}
/// One durable Microsoft request. Armed before it leaves; anything that does not prove
/// Microsoft refused it is uncertain and never sent again automatically.
pub(crate) async fn run_one(a: &App) -> Result<bool, convert_store::Error> {
    let Some((workspace, id)) = a.store.claim_microsoft_delivery().await? else {
        return Ok(false);
    };
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    // Refresh (and persist a rotated refresh token) before taking the workspace lock.
    let credentials = worker_token(a, &store).await;
    let mut guard = store.microsoft_guard(id).await?;
    let count = guard.delivery["attempts"].as_i64().unwrap_or(1);
    let Ok((_, token)) = credentials else {
        let retry = count < 5;
        guard
            .finish(
                if retry { "retry" } else { "needs_review" },
                None,
                "Microsoft authorization unavailable. Reconnect Microsoft Advertising if this persists; bounded retries are scheduled.",
                retry.then(|| backoff(count)),
                true,
            )
            .await?;
        return Ok(true);
    };
    if let Err(reason) = guard.fresh().await? {
        guard.stop(reason).await?;
        return Ok(true);
    }
    let request = guard.delivery["request"].clone();
    guard.arm().await?;
    // Reacquire and recheck after the durable attempt commit; edits serialize on this
    // lock throughout the bounded request.
    let mut guard = store.microsoft_guard(id).await?;
    if let Err(reason) = guard.fresh().await? {
        guard.stop(reason).await?;
        return Ok(true);
    }
    let Some(dev) = a
        .microsoft
        .config
        .as_ref()
        .map(|c| c.developer_token.as_str())
    else {
        guard
            .finish(
                "retry",
                None,
                "Microsoft Advertising is not configured.",
                Some(3600),
                true,
            )
            .await?;
        return Ok(true);
    };
    match a.microsoft.client.apply(&token, dev, &request).await {
        Ok(v) => {
            guard
                .finish(
                    "processed",
                    v["tracking_id"].as_str().filter(|s| !s.is_empty()),
                    "Microsoft accepted the request. Offline conversions can take up to 6 hours to appear in reports.",
                    None,
                    false,
                )
                .await?
        }
        Err(e) => {
            // Every error but Uncertain is Microsoft refusing the request, or a request
            // that never connected: nothing was applied, so the attempt marker is cleared.
            let (state, delay) = match e {
                DeliveryError::GoalNotReady if count < 10 => ("retry", Some(1800)),
                DeliveryError::Retry if count < 5 => ("retry", Some(backoff(count))),
                DeliveryError::Uncertain => ("uncertain", None),
                DeliveryError::Authorization => ("needs_review", None),
                _ => ("failed", None),
            };
            guard
                .finish(state, None, &e.message(), delay, state != "uncertain")
                .await?;
        }
    }
    Ok(true)
}
