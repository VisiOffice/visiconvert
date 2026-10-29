//! Buying and releasing SignalWire numbers in the workspace's connected project.
//! Purchases cost money, so each one is idempotent, capped and owner-only.
use super::*;
use convert_adapters::signalwire_api;
use convert_store::managed::{Purchase, due_releases};
use serde::Deserialize;
use std::time::Duration;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/numbers/search", post(search))
        .route("/numbers/purchase", post(purchase))
        .route("/numbers/{id}/release", post(release))
        .route("/numbers/{id}/restore", post(restore))
}

fn setting(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(default)
}
/// Live purchased numbers plus purchases in flight, per workspace.
fn purchase_limit() -> i64 {
    setting("VISICONVERT_PURCHASE_LIMIT", 25)
}
/// Days a removed number keeps forwarding before it is released at the carrier.
fn hold_days() -> i32 {
    setting("VISICONVERT_RELEASE_HOLD_DAYS", 45).min(365) as i32
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    area_code: String,
    #[serde(default)]
    limit: Option<u8>,
}
async fn search(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Search>,
) -> Api<Value> {
    connections::authorize(&access).await?;
    if input.area_code.len() != 3 || !input.area_code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a three-digit US area code".into(),
        ));
    }
    let (c, token) = signalwire::credentials(&a, &access.store).await?;
    let numbers = a
        .signalwire_client
        .search_local(
            &c.space_host,
            &c.project_id.to_string(),
            &token,
            &input.area_code,
            input.limit.unwrap_or(10),
        )
        .await?;
    Ok(Json(json!({"numbers": numbers})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Buy {
    request_id: Uuid,
    phone_number: String,
    destination: String,
    label: String,
    source: String,
    campaign: String,
}
async fn purchase(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Buy>,
) -> Api<Value> {
    connections::authorize(&access).await?;
    let n = NewNumber {
        provider: PhoneProvider::Signalwire,
        label: input.label,
        phone_number: input.phone_number,
        destination: input.destination,
        source: input.source,
        campaign: input.campaign,
    };
    n.validate()
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    let (c, token) = signalwire::credentials(&a, &access.store).await?;
    match access
        .store
        .begin_purchase(input.request_id, &n.phone_number, purchase_limit())
        .await?
    {
        Purchase::Registered(id) => {
            return Ok(Json(json!({"id": id, "phone_number": n.phone_number})));
        }
        Purchase::Unfinished => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This purchase request did not complete. Search again and retry with a new request ID.".into(),
            ));
        }
        Purchase::Started => {}
    }
    let project = c.project_id.to_string();
    let prefix = signalwire::webhook_prefix(&a, access.store.workspace);
    let bought = match a
        .signalwire_client
        .purchase(
            &c.space_host,
            &project,
            &token,
            &n.phone_number,
            &format!("{prefix}/voice"),
            &format!("{prefix}/status"),
        )
        .await
    {
        Ok(bought) => bought,
        Err(error) => {
            access.store.fail_purchase(input.request_id).await?;
            return Err(error.into());
        }
    };
    match access
        .store
        .finish_purchase(input.request_id, &n, c.revision, &bought.sid)
        .await
    {
        Ok(id) => Ok(Json(json!({"id": id, "phone_number": n.phone_number}))),
        Err(error) => {
            // Never keep paying for a number nothing routes to.
            if a.signalwire_client
                .release(&c.space_host, &project, &token, &bought.sid)
                .await
                .is_err()
            {
                tracing::error!(workspace = %access.store.workspace, "Release after failed registration failed");
            }
            access.store.fail_purchase(input.request_id).await?;
            Err(error.into())
        }
    }
}

async fn release(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    connections::authorize(&access).await?;
    access.store.request_release(id).await?;
    Ok(Json(json!({"ok": true, "hold_days": hold_days()})))
}

async fn restore(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    connections::authorize(&access).await?;
    access.store.restore_number(id).await?;
    Ok(Json(json!({"ok": true})))
}

/// Releases numbers whose hold has expired. Failures are retried on the next pass.
pub async fn release_due(a: &App) -> anyhow::Result<usize> {
    let mut released = 0;
    for due in due_releases(&a.store.pool, hold_days()).await? {
        let store = Store {
            pool: a.store.pool.clone(),
            workspace: due.workspace_id,
        };
        let Ok((c, token)) = signalwire::credentials(a, &store).await else {
            tracing::warn!(workspace = %due.workspace_id, "No SignalWire connection for a held number");
            continue;
        };
        match a
            .signalwire_client
            .release(
                &c.space_host,
                &c.project_id.to_string(),
                &token,
                &due.carrier_sid,
            )
            .await
        {
            Ok(()) => {
                store.mark_released(due.number_id).await?;
                released += 1;
            }
            Err(error) => {
                tracing::warn!(workspace = %due.workspace_id, %error, "Number release failed; will retry")
            }
        }
    }
    Ok(released)
}

pub fn spawn_release_loop(app: Shared) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(error) = release_due(&app).await {
                tracing::error!(%error, "Release pass failed");
            }
        }
    });
}

impl From<signalwire_api::Error> for ApiError {
    fn from(error: signalwire_api::Error) -> Self {
        let status = match error {
            signalwire_api::Error::Credentials | signalwire_api::Error::Number => {
                StatusCode::BAD_REQUEST
            }
            signalwire_api::Error::Taken => StatusCode::CONFLICT,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self(status, error.to_string())
    }
}
