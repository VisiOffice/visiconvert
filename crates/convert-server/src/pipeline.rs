//! Pipeline conversions: owner-controlled sending and consent policy, and the actions/events API
//! shared by the UI, workspace API keys and MCP. Planning runs immediately after each
//! event and again in the worker.
//!
//! Keys and MCP with conversions:write may define and map actions and post events. Turning
//! Google delivery or automatic sending on or off and attesting the consent policy need the
//! separate conversions:send scope, which only an owner can grant; those changes go through
//! the same store helpers as the owner routes. conversions:write may also run the read-only
//! destination check. Starting a Google connection and choosing the
//! advertising account stay in the private, owner-checked API (OAuth needs the owner).
use super::*;
use axum::extract::Query;
use convert_store::{
    api_keys::KeyAccess,
    pipeline::{ActionInput, DeliveryInput, EventInput, EventPage, PolicyInput},
};

pub fn private_routes() -> Router<Shared> {
    Router::new()
        .route("/conversions/pipeline", get(overview))
        .route("/conversions/pipeline/policy", axum::routing::put(policy))
        .route(
            "/conversions/actions/{key}",
            axum::routing::put(save_action),
        )
        .merge(shared_routes())
}
/// Mounted under /api/v1 for keys and MCP. Google discovery is read-only.
pub fn public_routes() -> Router<Shared> {
    shared_routes()
        .route("/connections/google/accounts", get(google::accounts))
        .route("/connections/google/actions", get(google::actions))
        .merge(microsoft::public_routes())
}
fn shared_routes() -> Router<Shared> {
    Router::new()
        .route("/conversion-setup", get(setup))
        .route("/conversion-setup/check", post(check_destination))
        .route(
            "/conversion-setup/destination",
            axum::routing::put(save_destination),
        )
        .route(
            "/conversion-delivery",
            get(delivery_state).put(set_delivery),
        )
        .route("/conversion-actions", get(list_actions))
        .route("/conversion-actions/{key}", axum::routing::put(save_action))
        .route(
            "/conversion-actions/{key}/google-action",
            post(create_google_action),
        )
        .route("/conversion-events", get(list).post(record))
        .merge(super::google_review::routes())
        .merge(microsoft::shared_routes())
        .merge(meta::shared_routes())
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
/// The owner a credential acts for: attestations name this person, not the credential.
async fn attester(
    access: &hosted::Access,
    key: &Option<Extension<KeyAccess>>,
) -> Result<String, ApiError> {
    Ok(match key {
        Some(k) => k.owner.map_or_else(
            || "workspace operator".into(),
            |u| format!("hosted user {u}"),
        ),
        None => {
            access
                .store
                .inquiry_actor(access.session_hash.as_deref())
                .await?
        }
    })
}
const DOUBLE_COUNT_WARNING: &str = "Automatic sending is on. Turn off every other uploader for the same Google conversion actions (Google Ads offline imports, Zapier or CRM integrations, another tracking tool), or Google will count these conversions twice. Conversions already sent to Google cannot be retracted.";
async fn delivery_state(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    Ok(Json(delivery_value(&a, &access.store).await?))
}
async fn delivery_value(a: &App, store: &Store) -> Result<Value, ApiError> {
    let mut value = store.pipeline_policy().await?;
    value["setup"] = setup_status(a, store).await?;
    if value["auto_send"] == true {
        let mut warning = DOUBLE_COUNT_WARNING.to_string();
        if let Some(r) = store.google_review().await? {
            let uploading: Vec<String> = r["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| {
                    w["flags"]
                        .as_array()
                        .is_some_and(|f| f.iter().any(|c| c == "active_other_uploader"))
                })
                .map(|w| {
                    format!(
                        "{} ({})",
                        w["name"].as_str().unwrap_or(""),
                        w["id"].as_str().unwrap_or("")
                    )
                })
                .collect();
            if !uploading.is_empty() {
                warning.push_str(&format!(" The last Google Ads review found another tool still uploading to: {}. Turn those uploaders off or make the actions secondary.", uploading.join(", ")));
            }
        }
        let microsoft = &value["setup"]["microsoft"];
        if microsoft["account_id"].is_string() && microsoft["mapped_actions"].as_i64() > Some(0) {
            warning.push_str(" The same applies to Microsoft Advertising: turn off any other offline conversion upload to the mapped Microsoft goals. Microsoft conversions can be retracted when an event is canceled.");
        }
        let meta = &value["setup"]["meta"];
        if meta["connected"] == true && meta["mapped_actions"].as_i64() > Some(0) {
            warning.push_str(" For Meta, turn off any other integration (such as a CRM or form tool's Meta connection) sending the same events to the dataset. Meta cannot withdraw a sent event.");
        }
        value["warning"] = json!(warning);
    }
    Ok(value)
}
/// conversions:send. Delivery, automatic sending and the consent policy, using the owner
/// routes' rules and the shared delivery settings version.
async fn set_delivery(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<DeliveryInput>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let attester = attester(&access, &key).await?;
    access
        .store
        .set_conversion_delivery(&input, &actor, &attester)
        .await?;
    Ok(Json(delivery_value(&a, &access.store).await?))
}
/// conversions:write. The owner's "Check saved connection": a read-only Google check of
/// the saved account and mapped actions, recorded exactly as the owner route records it.
async fn check_destination(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<google::Validation>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    Ok(Json(
        google::check_destination(&a, &access, &input, &actor).await?,
    ))
}
/// conversions:send: which advertising account receives conversions, and whether
/// inferred DNI calls are sent. Returns the saved destination and the checklist.
async fn save_destination(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<google::Destination>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let mut value = google::save_destination(&a, &access, &input, &actor).await?;
    value["setup"] = setup_status(&a, &access.store).await?;
    Ok(Json(value))
}
async fn overview(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let mut value = access.store.pipeline_overview().await?;
    value["setup"] = setup_status(&a, &access.store).await?;
    Ok(Json(value))
}
async fn policy(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<PolicyInput>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access.store.set_pipeline_policy(&input, &actor).await?;
    Ok(Json(access.store.pipeline_policy().await?))
}
async fn save_action(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(path): Path<String>,
    Json(input): Json<ActionInput>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    if path != input.key {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "The action key in the path and body must match".into(),
        ));
    }
    let actor = actor(&access, &key).await?;
    Ok(Json(
        access.store.save_conversion_action(&input, &actor).await?,
    ))
}
async fn list_actions(Extension(access): Extension<hosted::Access>) -> Api<Value> {
    conversions::owner(&access).await?;
    Ok(Json(
        json!({"data":access.store.conversion_actions().await?}),
    ))
}
async fn create_google_action(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(path): Path<String>,
    Json(input): Json<google::CreatePipelineAction>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    Ok(Json(
        google::create_pipeline_action(&a, &access.store, &path, &input, &actor).await?,
    ))
}
async fn record(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<EventInput>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = actor(&access, &key).await?;
    let id = access.store.record_pipeline_event(&input, &actor).await?;
    access
        .store
        .plan_pipeline(Some(id), &delivery::build)
        .await?;
    if a.microsoft.config.is_some() {
        access.store.plan_microsoft(Some(id)).await?;
    }
    if a.vault.is_some() {
        access.store.plan_meta(Some(id)).await?;
    }
    Ok(Json(access.store.pipeline_event(id).await?))
}
async fn list(
    Extension(access): Extension<hosted::Access>,
    Query(page): Query<EventPage>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    Ok(Json(access.store.pipeline_events(&page).await?))
}
async fn setup(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    Ok(Json(setup_status(&a, &access.store).await?))
}
/// Checklist for the pipeline flow. `next_steps` names who acts: `agent` steps any key or
/// connection with conversions:write can take; `owner_approval` switches need the owner,
/// either in Conversions or through a conversions:send credential after the owner confirms;
/// `owner` steps (Google OAuth) need the owner in Conversions.
pub(crate) async fn setup_status(a: &App, store: &Store) -> Result<Value, ApiError> {
    let mut f = store.pipeline_setup().await?;
    let connection = store.google_connection().await?;
    let connected = google::connected(a, &connection);
    let n = |k: &str| f[k].as_i64().unwrap_or(0);
    let on = |k: &str| f[k] == true;
    let mut steps: Vec<(&str, String)> = Vec::new();
    if a.google.is_none() {
        steps.push((
            "administrator",
            "Configure Google OAuth for this installation (see the Google connection guide)."
                .into(),
        ));
    } else if connection.encrypted_token.is_some() && connection.auth_failed_at.is_some() {
        steps.push(("owner", "Google authorization expired or was revoked, so nothing is sent to Google. The owner must reconnect Google in Conversions; held conversions send after that.".into()));
    } else if !connected {
        steps.push(("owner", "Owner must connect Google in Conversions. Keys and agents cannot start a Google connection.".into()));
    } else if !f["customer_id"].is_string() {
        steps.push((
            "owner_approval",
            "Choose the advertising account: the owner in Conversions, or an agent with conversions:send (set_google_destination) after the owner confirms the account name and ID.".into(),
        ));
    }
    if connected && f["customer_id"].is_string() && !on("allow_inferred_calls") && on("dni_calls") {
        steps.push((
            "owner_approval",
            "Decide whether to send calls matched to a website visit by timing on a DNI number (inferred calls). They are blocked until allowed: the owner in Conversions, or an agent with conversions:send (set_google_destination) after the owner agrees.".into(),
        ));
    }
    if n("enabled_actions") == 0 {
        steps.push(("agent", "Create a conversion action for each pipeline step (for example lead, quote_sent, order).".into()));
    } else if n("mapped_actions") < n("enabled_actions") {
        steps.push(("agent", "Map each enabled action to an existing Google import conversion action (preferred), or create a VisiConvert action in Google for it.".into()));
    }
    if connected && f["customer_id"].is_string() && n("mapped_actions") > 0 {
        if f["checked_at"].is_null() {
            steps.push(("agent", "Run the destination check once (check_google_destination, or POST /api/v1/conversion-setup/check) with revision set to connection_revision and settings_version from this checklist, and login set to the access account from list_google_accounts; VisiConvert repeats it automatically after that.".into()));
        } else if !on("pipeline_actions_valid") && on("check_current") {
            steps.push((
                "owner",
                format!(
                    "Destination check needs attention: {}",
                    f["check_message"]
                        .as_str()
                        .unwrap_or("check the mapped actions")
                ),
            ));
        }
    }
    // Switches an agent may flip only with conversions:send and the owner's go-ahead.
    if !on("delivery_enabled") {
        steps.push((
            "owner_approval",
            "Turn on Google delivery: the owner in Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner confirms.".into(),
        ));
    }
    if !on("auto_send") {
        steps.push((
            "owner_approval",
            "Turn on automatic sending: the owner in Conversions, or an agent with conversions:send (set_conversion_delivery) after the owner confirms that no other uploader sends the same Google actions.".into(),
        ));
    }
    let ready = steps.is_empty();
    // Advisory: overlap in the Google Ads account itself. Does not block `ready`, since the
    // owner may keep an action primary on purpose.
    let mut advisory: Vec<(&str, String)> = Vec::new();
    let review = store.google_review().await?;
    if connected && f["customer_id"].is_string() {
        match &review {
            None => advisory.push(("agent", "Run review_google_conversion_setup (GET /api/v1/conversion-setup/google-review) to find other conversion actions in Google Ads that would double count with VisiConvert.".into())),
            Some(r) if r["warnings"].as_array().is_some_and(|w| !w.is_empty()) => advisory.push((
                "owner_approval",
                format!("{} Review with review_google_conversion_setup and, after the owner confirms each one, make them secondary with set_google_action_goal (conversions:send); for Google-hosted actions such as clicks to call, switch their account-level goal off for bidding with set_google_goal_biddable instead.", r["summary"].as_str().unwrap_or("")),
            )),
            Some(_) => {}
        }
    }
    f["google_review"] = review.unwrap_or(Value::Null);
    // Additive: Microsoft readiness never changes Google's ready, next_steps or summary.
    f["microsoft"] = microsoft::setup_status(a, store).await?;
    f["meta"] = meta::setup_status(a, store).await?;
    f["google_connected"] = json!(connected);
    f["google_reconnect_required"] =
        json!(connection.encrypted_token.is_some() && !connected && a.google.is_some());
    f["google_auth_failed_at"] = json!(connection.auth_failed_at);
    f["connection_revision"] = json!(connection.revision);
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
        "Ready: eligible pipeline events are sent to Google automatically.".to_string()
    } else if steps
        .iter()
        .all(|(who, _)| matches!(*who, "owner" | "owner_approval"))
    {
        format!(
            "Only owner steps are left: {}",
            steps
                .iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
    } else {
        steps[0].1.clone()
    });
    if f["consent_policy"] == "per_lead" {
        f["consent_note"] = json!(
            "Only leads with recorded advertising consent are sent. The owner can change the consent policy in Conversions, or confirm the change for an agent with conversions:send."
        );
    }
    Ok(f)
}
