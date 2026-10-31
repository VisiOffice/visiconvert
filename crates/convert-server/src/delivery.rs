//! Google delivery for both self-hosted and hosted workspaces. No automatic staging.
use super::*;
use convert_adapters::google_api::{self, DeliveryError};
use convert_store::conversions::Revision;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/conversions/delivery/settings",
            axum::routing::put(settings),
        )
        .route("/conversions/{id}/validate", post(validate))
        .route("/conversions/deliveries/{id}/queue", post(queue))
        .route("/conversions/deliveries/{id}/recheck", post(recheck))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    enabled: bool,
    version: i32,
}
async fn settings(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Settings>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access
        .store
        .set_delivery_enabled(input.enabled, input.version, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn validate(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<Revision>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    let (connection, token) = google::worker_token(&a, &access.store).await?;
    let mut guard = access.store.preparation_guard(id, input.version).await?;
    let report = guard.fresh(connection.revision, false).await?;
    let request = google_api::event_request(
        &guard.job["snapshot"],
        report["login_customer_id"].as_str().unwrap_or(""),
    )?;
    if let Some(prior) = guard.previous_request().await?
        && !google_api::same_delivery_identity(&prior, &request)
    {
        return Err(ApiError(StatusCode::CONFLICT,"An event already sent can only update its booking value. Its destination, identifiers, original event time and consent must remain unchanged.".into()));
    }
    let result = a.google_client.ingest(&token, &request, true).await?;
    Ok(Json(
        guard
            .validated(
                request,
                connection.revision,
                result["warnings"] == true,
                &actor,
            )
            .await?,
    ))
}
async fn queue(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    conversions::owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access.store.queue_delivery(id, &actor).await?;
    Ok(Json(json!({"ok":true})))
}
async fn recheck(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    conversions::owner(&access).await?;
    access.store.recheck_delivery(id).await?;
    Ok(Json(json!({"ok":true})))
}
impl From<DeliveryError> for ApiError {
    fn from(e: DeliveryError) -> Self {
        Self(
            match e {
                DeliveryError::Retry => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            },
            e.message().into(),
        )
    }
}
pub fn spawn(a: Shared) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            for _ in 0..20 {
                match run_one(&a).await {
                    Ok(true) => (),
                    Ok(false) => break,
                    Err(_) => {
                        tracing::error!(
                            "Google delivery database pass failed; durable attempts will be recovered"
                        );
                        break;
                    }
                }
            }
        }
    });
}
/// One durable job; testable with a local provider and a disposable database.
pub(crate) async fn run_one(a: &App) -> Result<bool, convert_store::Error> {
    let Some((workspace, id)) = a.store.claim_delivery().await? else {
        return Ok(false);
    };
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    // Refresh before acquiring the workspace lock; refresh rotation uses the DB.
    let credentials = google::worker_token(a, &store).await;
    let mut guard = store.claimed_guard(id).await?;
    let d = guard.delivery.as_ref().unwrap().clone();
    let polling = d["state"] == "polling";
    let count = d[if polling { "polls" } else { "attempts" }]
        .as_i64()
        .unwrap_or(1);
    let (connection, token) = match credentials {
        Ok(c) => c,
        Err(_) => {
            // A temporary token outage is bounded, and no event is sent without it.
            let retry = count < if polling { 24 } else { 5 };
            guard.finish(if retry {if polling {"submitted"} else {"retry"}} else {"needs_review"},None,"Google authorization unavailable. Check the connection; bounded retries are scheduled.",false,if retry {Some(backoff(count))} else {None}).await?;
            return Ok(true);
        }
    };
    if polling {
        if d["connection_revision"] != connection.revision {
            guard.finish("needs_review",None,"Google connection changed. Recheck the saved receipt after confirming access to its original destination.",false,None).await?;
            return Ok(true);
        }
        let result = a
            .google_client
            .delivery_status(
                &token,
                d["request_id"].as_str().unwrap_or(""),
                &d["request"]["destinations"][0],
            )
            .await;
        match result {
            Ok(status) => {
                let state = status["state"].as_str().unwrap();
                let waiting = state == "submitted";
                let exhausted = waiting && count >= 24;
                guard.finish(if exhausted {"uncertain"} else {state},None,if exhausted {"Google processing has not completed. Recheck the receipt later; do not submit a duplicate."} else {status["diagnostic"].as_str().unwrap_or("")},status["warnings"]==true,if waiting && !exhausted {Some(backoff(count))} else {None}).await?;
            }
            Err(e) => {
                let retry = count < 24;
                guard
                    .finish(
                        if retry { "submitted" } else { "uncertain" },
                        None,
                        e.message(),
                        false,
                        if retry { Some(backoff(count)) } else { None },
                    )
                    .await?;
            }
        }
        return Ok(true);
    }
    let report = match guard.fresh(connection.revision, true).await {
        Ok(v) => v,
        Err(_) => {
            guard.finish("needs_review",None,"Delivery stopped: source, consent, destination, connection or workspace access changed. Review and validate again.",false,None).await?;
            return Ok(true);
        }
    };
    let expected = google_api::event_request(
        &d["snapshot"],
        report["login_customer_id"].as_str().unwrap_or(""),
    );
    if expected.as_ref().ok() != Some(&d["request"]) {
        guard
            .finish(
                "needs_review",
                None,
                "Validated Google destination changed. Review and validate again.",
                false,
                None,
            )
            .await?;
        return Ok(true);
    }
    guard.arm().await?;
    // Reacquire and recheck after the durable attempt commit. Edits and disconnects
    // serialize on this lock throughout the bounded HTTP request.
    let mut guard = store.claimed_guard(id).await?;
    if guard.fresh(connection.revision, true).await.is_err() {
        guard.not_accepted("Source changed before send. No request sent by this attempt; review the preparation.").await?;
        return Ok(true);
    }
    match a.google_client.ingest(&token, &d["request"], false).await {
        Ok(v) => {
            guard
                .finish(
                    "submitted",
                    v["requestId"].as_str(),
                    "Google accepted the request for processing. Attribution is not yet confirmed.",
                    v["warnings"] == true,
                    Some(30),
                )
                .await?
        }
        Err(e) => {
            if e == DeliveryError::Retry && count < 5 {
                guard
                    .finish("retry", None, e.message(), false, Some(backoff(count)))
                    .await?;
            } else if e == DeliveryError::Uncertain {
                guard
                    .finish("uncertain", None, e.message(), false, None)
                    .await?;
            } else {
                guard.not_accepted("Google did not accept this upload. Check the connection, action, click identifier and event window, then validate again.").await?;
            }
        }
    }
    Ok(true)
}
fn backoff(attempt: i64) -> i64 {
    (30_i64.saturating_mul(1_i64 << attempt.clamp(0, 7))).min(3600)
}
