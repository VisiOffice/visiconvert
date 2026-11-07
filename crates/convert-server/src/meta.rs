//! Meta (Facebook and Instagram ads) as a third destination for pipeline conversions,
//! through the Conversions API: saving the owner's dataset (Pixel) ID and Events Manager
//! access token, the matching and Limited Data Use settings, event mappings, the
//! destination check (optionally with a Test events code), and the worker that sends
//! events.
//!
//! The access token is accepted only by the connection route, from the owner in
//! Conversions or a conversions:send key (the CLI reads it from an environment variable).
//! No MCP tool takes it, and it is never returned: only its fingerprint.
use super::*;
use convert_adapters::meta_api::{self, DeliveryError};
use convert_store::{api_keys::KeyAccess, meta::Connection};
use serde::Deserialize;

#[derive(Clone)]
pub struct Meta {
    pub client: meta_api::Client,
}
impl Default for Meta {
    fn default() -> Self {
        Self {
            client: meta_api::Client::new().expect("HTTP client"),
        }
    }
}
fn bad(m: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, m.into())
}
impl From<meta_api::Error> for ApiError {
    fn from(e: meta_api::Error) -> Self {
        use meta_api::Error::*;
        Self(
            match e {
                Token | Access => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            },
            e.to_string(),
        )
    }
}
/// Mounted for owners (/api) and for keys and MCP (/api/v1).
pub fn shared_routes() -> Router<Shared> {
    Router::new()
        .route(
            "/connections/meta",
            get(show).put(connect).delete(disconnect),
        )
        .route(
            "/conversion-setup/meta-settings",
            axum::routing::put(settings_route),
        )
        .route("/conversion-setup/meta-check", post(check_route))
        .route(
            "/conversion-actions/{key}/meta-event",
            axum::routing::put(map_event_route),
        )
}
fn vault(a: &App) -> Result<&secrets::Vault, ApiError> {
    a.vault.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Meta needs the installation's encryption key (VISICONVERT_ENCRYPTION_KEY) to store the access token. Ask the administrator to configure it.".into(),
        )
    })
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
async fn show(Extension(access): Extension<hosted::Access>) -> Api<Value> {
    conversions::owner(&access).await?;
    Ok(Json(access.store.meta_connection().await?.status()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connect {
    /// `revision` from the Meta checklist (0 before the first save).
    revision: i64,
    /// The dataset (Pixel) ID from Events Manager.
    dataset_id: String,
    /// A Conversions API access token generated in that dataset's settings.
    access_token: String,
}
/// conversions:send. Save or replace the dataset and token after reading the dataset with
/// the token. The token is sealed and never returned.
async fn connect(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Connect>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let vault = vault(&a)?;
    let dataset_id = input.dataset_id.trim();
    let token = input.access_token.trim();
    if !meta_api::numeric_id(dataset_id) {
        return Err(bad(
            "dataset_id must be the numeric dataset (Pixel) ID from Events Manager",
        ));
    }
    if !meta_api::token_shape(token) {
        return Err(bad(
            "access_token does not look like a Meta access token. Generate one in Events Manager: the dataset's Settings, Conversions API, Generate access token.",
        ));
    }
    let current = access.store.meta_connection().await?;
    if current.revision != input.revision {
        return Err(convert_store::Error::Conflict.into());
    }
    let dataset = a.meta.client.dataset(token, dataset_id).await?;
    let sealed = vault
        .seal_meta(access.store.workspace, "token", token)
        .map_err(|_| connections::vault_error())?;
    let fingerprint = meta_api::sha256_hex(token)[..12].to_string();
    let revision = access
        .store
        .save_meta_connection(
            input.revision,
            (&dataset.id, &dataset.name),
            &sealed,
            &fingerprint,
            &actor,
        )
        .await?;
    Ok(Json(
        json!({"revision":revision,"dataset":dataset,"token_fingerprint":fingerprint,
        "setup":pipeline::setup_status(&a, &access.store).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: i64,
}
/// conversions:send. Removes the token and dataset, and clears kept browser details.
async fn disconnect(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Revision>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    access.store.disconnect_meta(input.revision, &actor).await?;
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    /// `settings_version` from the Meta checklist (0 before the first save).
    settings_version: i32,
    /// meta_clicks (only leads whose visit had a Meta ad click) or all_leads.
    #[serde(rename = "match")]
    match_mode: String,
    /// off, or geolocate (send Limited Data Use and let Meta apply it by location).
    ldu: String,
}
/// conversions:send. Which leads Meta receives changes what is shared with Meta, so it
/// needs the owner's confirmation.
async fn settings_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Settings>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let version = access
        .store
        .save_meta_settings(
            input.settings_version,
            &input.match_mode,
            &input.ldu,
            &actor,
        )
        .await?;
    Ok(Json(json!({"settings_version":version,
        "setup":pipeline::setup_status(&a, &access.store).await?})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MapEvent {
    /// Current version of the VisiConvert action.
    version: i32,
    /// A Meta standard event (Lead, Purchase, Schedule…) or custom event name; null clears.
    event_name: Option<String>,
    /// Owner's choice: also send events since this time (at most 7 days ago). Only when no
    /// other integration sent them to Meta, or they count twice.
    #[serde(default)]
    include_since: Option<chrono::DateTime<chrono::Utc>>,
}
/// conversions:write.
async fn map_event_route(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(path): Path<String>,
    Json(input): Json<MapEvent>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let name = input
        .event_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok(Json(
        access
            .store
            .set_meta_action_event(&path, name, input.include_since, input.version, &actor)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    revision: i64,
    /// From Events Manager's Test events tab. Sends one test event there; test events
    /// never count as conversions.
    #[serde(default)]
    test_event_code: Option<String>,
}
fn test_code(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
/// conversions:write. Reads the dataset with the saved token, and with a test code sends
/// one test event, recording the report against the connection revision.
async fn check_route(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Check>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let code = input
        .test_event_code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if code.is_some_and(|c| !test_code(c)) {
        return Err(bad(
            "test_event_code is the code shown in Events Manager's Test events tab, such as TEST12345",
        ));
    }
    let c = access.store.meta_connection().await?;
    if c.revision != input.revision {
        return Err(convert_store::Error::Conflict.into());
    }
    let report = destination_report(&a, &access.store, &c, code).await?;
    access
        .store
        .save_meta_validation(c.revision, &report, &actor)
        .await?;
    Ok(Json(report))
}
fn open_token(a: &App, store: &Store, encrypted: &str) -> Result<String, ApiError> {
    vault(a)?
        .open_meta(store.workspace, "token", encrypted)
        .map_err(|_| connections::vault_error())
}
/// Read the dataset (and optionally send a test event). A rejected token or dataset is a
/// failed check; Meta being unavailable is an error and records nothing.
pub(crate) async fn destination_report(
    a: &App,
    store: &Store,
    c: &Connection,
    test_event_code: Option<&str>,
) -> Result<Value, ApiError> {
    let (Some(encrypted), Some(dataset_id)) = (&c.encrypted_token, &c.dataset_id) else {
        return Err(bad("Save the Meta dataset and access token first."));
    };
    let token = open_token(a, store, encrypted)?;
    let dataset = match a.meta.client.dataset(&token, dataset_id).await {
        Ok(d) => d,
        Err(e @ (meta_api::Error::Token | meta_api::Error::Access)) => {
            return Ok(
                json!({"valid":false,"dataset":{"id":dataset_id},"message":e.to_string(),"test_event":null}),
            );
        }
        Err(e) => return Err(e.into()),
    };
    let mut report = json!({"valid":true,"dataset":dataset,"test_event":null,
        "message":"The token can use this dataset. Send a test event (test_event_code) to confirm events arrive in Events Manager."});
    if let Some(code) = test_event_code {
        let event = json!({"event_name":"Lead","event_time":chrono::Utc::now().timestamp(),"event_id":format!("vc-test-{}",Uuid::new_v4()),
            "action_source":"system_generated","user_data":{"external_id":[meta_api::sha256_hex(&format!("visiconvert-test-{}",store.workspace))]}});
        match a
            .meta
            .client
            .send(&token, dataset_id, &[event], Some(code))
            .await
        {
            Ok(acc) => {
                report["test_event"] =
                    json!({"sent":true,"trace_id":acc.trace_id,"test_event_code":code});
                report["message"] = json!(
                    "The token can use this dataset and Meta accepted a test Lead event. Look for it in Events Manager's Test events tab; test events never count as conversions."
                );
            }
            Err(DeliveryError::Retry | DeliveryError::Uncertain) => {
                return Err(meta_api::Error::Unavailable.into());
            }
            Err(e) => {
                report["valid"] = json!(false);
                report["test_event"] = json!({"sent":false,"test_event_code":code});
                report["message"] = json!(e.message());
            }
        }
    }
    Ok(report)
}
async fn refresh_destination(a: &App, store: &Store) -> Result<(), ApiError> {
    let c = store.meta_connection().await?;
    let report = destination_report(a, store, &c, None).await?;
    store
        .save_meta_validation(c.revision, &report, "worker")
        .await?;
    Ok(())
}

pub(crate) async fn setup_status(a: &App, store: &Store) -> Result<Value, ApiError> {
    let mut f = store.meta_setup().await?;
    let c = store.meta_connection().await?;
    let configured = a.vault.is_some();
    let connected = configured && c.encrypted_token.is_some();
    let policy = store.pipeline_policy().await?;
    let n = |k: &str| f[k].as_i64().unwrap_or(0);
    let report = c.validation.clone().unwrap_or_default();
    let mut steps: Vec<(&str, String)> = Vec::new();
    if !configured {
        steps.push(("administrator", "Meta needs the installation's encryption key to store the access token; the administrator must configure it.".into()));
    } else if !connected {
        steps.push(("owner_approval", "Save the Meta dataset (Pixel) ID and a Conversions API access token from Events Manager: the owner in Conversions, or with the owner's token through the CLI (visiconvert connect meta --dataset <id> --token-env <VAR>, conversions:send). MCP tools never accept the token.".into()));
    }
    if n("mapped_actions") == 0 {
        steps.push(("agent", "Map at least one enabled action to a Meta event (set_meta_action_event), such as Lead for new leads or Purchase for won jobs.".into()));
    }
    if connected && n("mapped_actions") > 0 && report["valid"] != true && c.check_current() {
        steps.push((
            "owner",
            format!(
                "Meta destination check needs attention: {}",
                report["message"]
                    .as_str()
                    .unwrap_or("check the dataset and token")
            ),
        ));
    }
    if policy["enabled"] != true || policy["auto_send"] != true {
        steps.push(("owner_approval", "Turn on delivery and automatic sending (shared with Google and Microsoft): the owner in Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner confirms that no other integration (such as a CRM's Meta connection) sends the same conversions.".into()));
    }
    let mut advisory: Vec<(&str, String)> = Vec::new();
    if connected && report["test_event"].is_null() {
        advisory.push(("agent", "Send one test event (check_meta_destination with the test_event_code from Events Manager's Test events tab) to confirm events arrive.".into()));
    }
    if c.match_mode == "meta_clicks" && n("mapped_actions") > 0 {
        advisory.push(("owner_approval", "Only leads whose website visit had a Meta ad click are sent (match meta_clicks). To send every lead and let Meta match by hashed phone and email, the owner can choose all_leads (set_meta_settings, conversions:send).".into()));
    }
    if f["dni_calls"] == true && f["allow_inferred_calls"] != true && n("mapped_actions") > 0 {
        advisory.push(("owner_approval", "Calls matched to a website visit by timing on a DNI number (inferred calls) are not sent to Meta as ad clicks until the owner allows them (set_google_destination allow_inferred_calls).".into()));
    }
    if n("mapped_actions") > 0 {
        advisory.push(("agent", "Meta cannot restate or withdraw a sent event: the first version stands, and Meta only accepts events from the last 7 days.".into()));
    }
    let ready = steps.is_empty();
    f["configured"] = json!(configured);
    f["connected"] = json!(connected);
    f["revision"] = json!(c.revision);
    f["dataset_id"] = json!(c.dataset_id);
    f["dataset_name"] = json!(c.dataset_name);
    f["token_fingerprint"] = json!(c.token_fingerprint);
    f["token_saved_at"] = json!(c.token_saved_at);
    f["match"] = json!(c.match_mode);
    f["ldu"] = json!(c.ldu);
    f["settings_version"] = json!(c.settings_version);
    f["checked_at"] = json!(c.checked_at);
    f["check_current"] = json!(connected && c.check_current());
    f["check_valid"] = json!(report["valid"] == true);
    f["check_message"] = report["message"].clone();
    f["test_event"] = report["test_event"].clone();
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
        "Ready: eligible pipeline events are sent to Meta's Conversions API automatically."
            .to_string()
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
    if !store.meta_refresh_due().await? {
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
        tracing::warn!(workspace = %store.workspace, "Automatic Meta destination check failed");
    }
    Ok(())
}
static CLEARED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// Record new_lead events, refresh expired checks, plan due Meta revisions, and (hourly)
/// clear browser details older than 30 days.
pub(crate) async fn plan(a: &App) -> Result<(), convert_store::Error> {
    let due = {
        let mut last = CLEARED.lock().unwrap();
        let due = last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3600));
        if due {
            *last = Some(std::time::Instant::now());
        }
        due
    };
    if due {
        a.store.clear_meta_visit_details().await?;
    }
    for workspace in a.store.meta_workspaces().await? {
        let store = Store {
            pool: a.store.pool.clone(),
            workspace,
        };
        store.create_lead_events().await?;
        refresh(a, &store).await?;
        store.plan_meta(None).await?;
    }
    Ok(())
}
fn backoff(attempt: i64) -> i64 {
    (30_i64.saturating_mul(1_i64 << attempt.clamp(0, 7))).min(3600)
}
/// One durable Meta request. Armed before it leaves; anything that does not prove Meta
/// refused it is uncertain and never sent again automatically.
pub(crate) async fn run_one(a: &App) -> Result<bool, convert_store::Error> {
    let Some((workspace, id)) = a.store.claim_meta_delivery().await? else {
        return Ok(false);
    };
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    let mut guard = store.meta_guard(id).await?;
    let count = guard.delivery["attempts"].as_i64().unwrap_or(1);
    if let Err(reason) = guard.fresh().await? {
        guard.stop(reason).await?;
        return Ok(true);
    }
    let token = match guard.token().await? {
        Some(encrypted) => open_token(a, &store, &encrypted).ok(),
        None => None,
    };
    let Some(token) = token else {
        let retry = count < 5;
        guard
            .finish(
                if retry { "retry" } else { "needs_review" },
                None,
                "The Meta access token is unavailable. Save it again in Conversions if this persists; bounded retries are scheduled.",
                retry.then(|| backoff(count)),
                true,
            )
            .await?;
        return Ok(true);
    };
    let request = guard.delivery["request"].clone();
    guard.arm().await?;
    // Reacquire and recheck after the durable attempt commit; edits serialize on this
    // lock throughout the bounded request.
    let mut guard = store.meta_guard(id).await?;
    if let Err(reason) = guard.fresh().await? {
        guard.stop(reason).await?;
        return Ok(true);
    }
    let dataset = request["dataset_id"].as_str().unwrap_or_default();
    match a
        .meta
        .client
        .send(&token, dataset, &[request["event"].clone()], None)
        .await
    {
        Ok(accepted) => guard
            .finish(
                "processed",
                Some(accepted.trace_id.as_str()).filter(|s| !s.is_empty()),
                "Meta accepted the event. It appears in Events Manager within about 20 minutes.",
                None,
                false,
            )
            .await?,
        Err(e) => {
            // Every error but Uncertain is Meta refusing the request, or a request that
            // never connected: nothing was applied, so the attempt marker is cleared.
            let (state, delay) = match e {
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
