//! Buying and releasing SignalWire numbers in the workspace's connected project.
//! Purchases cost money, so each one is idempotent, capped and owner-only.
use super::*;
use convert_adapters::signalwire_api;
use convert_store::managed::{Purchase, due_releases, recoverable_purchases};
use convert_store::signalwire::SignalWireConnection;
use serde::Deserialize;
use std::time::Duration;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/numbers/search", post(search))
        .route("/numbers/purchase", post(purchase))
        .route("/numbers/purchases/{id}", get(purchase_status))
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
/// Includes failed carrier attempts and cleanup, so deletion cannot reset the cap.
fn purchase_attempt_limit() -> i64 {
    setting("VISICONVERT_PURCHASE_ATTEMPT_LIMIT", 25)
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
    // Existing receipts remain inspectable even when new spending is disabled.
    match access.store.purchase_status(input.request_id).await {
        Ok(_) => {}
        Err(convert_store::Error::NotFound) => {
            authorize_managed_purchase(&a, &access.store, &c).await?
        }
        Err(error) => return Err(error.into()),
    }
    match access
        .store
        .begin_purchase(
            input.request_id,
            &n,
            &c,
            purchase_limit(),
            purchase_attempt_limit(),
        )
        .await?
    {
        Purchase::Registered(id) => {
            return Ok(Json(json!({"id": id, "phone_number": n.phone_number})));
        }
        Purchase::Unfinished => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This purchase is awaiting reconciliation or did not complete. Check its status using the same request ID; do not submit a new purchase.".into(),
            ));
        }
        Purchase::Started => {}
    }
    if c.managed {
        access.store.start_reservation(input.request_id).await?;
        let receipt = match billing_request(
            &a,
            json!({"operation":"reserve", "workspace_id":access.store.workspace,
            "project_id":c.project_id,"request_id":input.request_id,"phone_number":n.phone_number}),
        )
        .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                if matches!(error, BillingFailure::Rejected(_)) {
                    access.store.reject_reservation(input.request_id).await?;
                } else {
                    // No carrier request started. Cancel it, but retain a lookup
                    // obligation for any hold whose response was lost.
                    access
                        .store
                        .cancel_unstarted_purchase(input.request_id)
                        .await?;
                }
                return Err(error.into());
            }
        };
        let reservation = receipt["reservation_id"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or_else(billing_unavailable)?;
        if receipt["workspace_id"] != access.store.workspace.to_string()
            || receipt["project_id"] != c.project_id.to_string()
            || receipt["request_id"] != input.request_id.to_string()
            || receipt["status"] != "held"
        {
            return Err(billing_unavailable());
        }
        access
            .store
            .record_reservation(input.request_id, reservation)
            .await?;
    }
    if let Err(error) = access
        .store
        .start_carrier_purchase(input.request_id, purchase_attempt_limit())
        .await
    {
        // A conflict/rate limit before the carrier call is safe to cancel; held
        // reservations are released by the same durable settlement outbox.
        access
            .store
            .cancel_unstarted_purchase(input.request_id)
            .await?;
        return Err(error.into());
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
            &purchase_marker(access.store.workspace, input.request_id),
        )
        .await
    {
        Ok(bought) => bought,
        Err(error) => {
            if matches!(
                error,
                signalwire_api::Error::Taken | signalwire_api::Error::Credentials
            ) {
                access.store.fail_purchase(input.request_id).await?;
            }
            // Timeouts, 5xx and malformed success responses are ambiguous: retain
            // the durable intent and recover by marker, never re-POST the purchase.
            return Err(error.into());
        }
    };
    access
        .store
        .record_purchase_number(input.request_id, &bought.sid)
        .await?;
    match access
        .store
        .finish_purchase(input.request_id, &n, &bought.sid)
        .await
    {
        Ok(id) => Ok(Json(json!({"id": id, "phone_number": n.phone_number}))),
        Err(error) => {
            if matches!(
                error,
                convert_store::Error::Validation(_) | convert_store::Error::Conflict
            ) {
                // Persist the cleanup obligation before any DELETE. Database/network
                // errors stay pending: the registration commit itself may have succeeded.
                access.store.cleanup_purchase(input.request_id).await?;
            }
            Err(error.into())
        }
    }
}

fn purchase_marker(workspace: Uuid, request: Uuid) -> String {
    format!("VisiConvert purchase {workspace}/{request}")
}
async fn purchase_status(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    Ok(Json(access.store.purchase_status(id).await?))
}

/// Rails owns managed-spending eligibility. A normal workspace entitlement is not
/// an allowance to spend the platform's carrier balance. BYOC pays its own carrier.
async fn authorize_managed_purchase(
    a: &App,
    store: &Store,
    connection: &SignalWireConnection,
) -> Result<(), ApiError> {
    if !connection.managed {
        return Ok(());
    }
    let denied = || {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Managed number purchasing is unavailable until usage billing is enabled".into(),
        )
    };
    let cloud = a.cloud.as_ref().ok_or_else(denied)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| denied())?;
    let response = client
        .post(format!(
            "{}/internal/managed-number-access",
            cloud.cloud_origin
        ))
        .bearer_auth(&cloud.service_key)
        .json(&json!({"workspace_id":store.workspace,"project_id":connection.project_id}))
        .send()
        .await
        .map_err(|_| denied())?;
    if !response.status().is_success() {
        return Err(denied());
    }
    let value: Value = response.json().await.map_err(|_| denied())?;
    if value["allowed"] != true
        || value["workspace_id"] != store.workspace.to_string()
        || value["project_id"] != connection.project_id.to_string()
    {
        return Err(denied());
    }
    Ok(())
}

fn billing_unavailable() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Managed purchase billing is unavailable; retain the request ID for reconciliation".into(),
    )
}
#[derive(Debug)]
enum BillingFailure {
    Rejected(StatusCode),
    Unknown,
}
impl From<BillingFailure> for ApiError {
    fn from(value: BillingFailure) -> Self {
        match value {
            BillingFailure::Rejected(StatusCode::CONFLICT | StatusCode::FORBIDDEN) => Self(
                StatusCode::CONFLICT,
                "Managed purchase was not authorized; check billing before making a new request"
                    .into(),
            ),
            _ => billing_unavailable(),
        }
    }
}
async fn billing_request(a: &App, body: Value) -> Result<Value, BillingFailure> {
    let cloud = a.cloud.as_ref().ok_or(BillingFailure::Unknown)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent("VisiConvert/0.1")
        .build()
        .map_err(|_| BillingFailure::Unknown)?;
    let mut response = client
        .post(format!(
            "{}/internal/managed-number-access",
            cloud.cloud_origin
        ))
        .bearer_auth(&cloud.service_key)
        .json(&body)
        .send()
        .await
        .map_err(|_| BillingFailure::Unknown)?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::CONFLICT
    ) {
        return Err(BillingFailure::Rejected(response.status()));
    }
    if !response.status().is_success() {
        return Err(BillingFailure::Unknown);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| BillingFailure::Unknown)?
    {
        if bytes.len() + chunk.len() > 8192 {
            return Err(BillingFailure::Unknown);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| BillingFailure::Unknown)
}
/// Retry terminal receipts until Rails acknowledges the exact operation. Neither
/// successful purchases nor ambiguous timeouts release budget automatically.
pub async fn settle_reservations(a: &App) -> anyhow::Result<()> {
    for row in convert_store::managed::reservation_settlements(&a.store.pool).await? {
        let workspace = row.workspace;
        let project = row.project;
        let request = row.request;
        let store = Store {
            pool: a.store.pool.clone(),
            workspace,
        };
        let reservation = match row.reservation {
            Some(id) => id,
            None => {
                // A failed, pre-carrier request can still own a late Rails hold.
                // Never treat an HTTP lookup error as proof that no hold exists.
                let Ok(value) = billing_request(a, json!({"operation":"lookup","workspace_id":workspace,"project_id":project,"request_id":request})).await else {
                    tracing::warn!(%workspace,%request,"Canceled purchase reservation lookup unresolved; retain billing hold");
                    continue;
                };
                if value["workspace_id"] != workspace.to_string()
                    || value["project_id"] != project.to_string()
                    || value["request_id"] != request.to_string()
                    || value["status"] != "held"
                {
                    tracing::warn!(%workspace,%request,"Canceled purchase reservation identity requires review");
                    continue;
                }
                let Some(id) = value["reservation_id"].as_i64().filter(|v| *v > 0) else {
                    continue;
                };
                store.record_reservation(request, id).await?;
                id
            }
        };
        let status = row.status;
        let sid = if status == "registered" {
            row.carrier_sid
        } else {
            None
        };
        let expected = if status == "registered" {
            "committed"
        } else {
            "released"
        };
        let result = billing_request(
            a,
            json!({"operation":"settle","workspace_id":workspace,"project_id":project,
            "request_id":request,"reservation_id":reservation,"outcome":status,"carrier_sid":sid}),
        )
        .await;
        if let Ok(value) = result
            && value["workspace_id"] == workspace.to_string()
            && value["project_id"] == project.to_string()
            && value["request_id"] == request.to_string()
            && value["reservation_id"] == reservation
            && value["status"] == expected
        {
            store
                .settle_reservation_receipt(request, reservation, &status)
                .await?;
            continue;
        }
        tracing::warn!(%workspace,%request,"Purchase reservation settlement will retry");
    }
    Ok(())
}

/// Recover an interrupted purchase from carrier inventory. A phone number alone is
/// not ownership evidence: the request-specific marker must also match. Missing
/// inventory remains pending for operator investigation rather than freeing capacity.
pub async fn reconcile_purchases(a: &App) -> anyhow::Result<()> {
    for pending in recoverable_purchases(&a.store.pool).await? {
        let result: anyhow::Result<()> = async {
            let store = Store { pool:a.store.pool.clone(), workspace:pending.workspace };
            if !pending.cleanup && matches!(pending.phase.as_str(), "prepared" | "reserving" | "reserved") {
                store.cancel_unstarted_purchase(pending.request).await?;
                return Ok(());
            }
            let (c, token) = signalwire::credentials(a, &store).await.map_err(|_| anyhow::anyhow!("Purchase credentials unavailable"))?;
            anyhow::ensure!(c.space_host==pending.space && c.project_id==pending.project, "Purchase project changed; manual reconciliation required");
            if pending.managed && pending.reservation.is_none() {
                let receipt=billing_request(a,json!({"operation":"lookup","workspace_id":pending.workspace,"project_id":pending.project,"request_id":pending.request})).await.map_err(|_|anyhow::anyhow!("Reservation lookup unavailable"))?;
                anyhow::ensure!(receipt["workspace_id"]==pending.workspace.to_string() && receipt["project_id"]==pending.project.to_string() && receipt["request_id"]==pending.request.to_string() && receipt["status"]=="held", "Reservation identity mismatch");
                let reservation=receipt["reservation_id"].as_i64().filter(|v|*v>0).ok_or_else(||anyhow::anyhow!("Reservation missing"))?;
                store.record_reservation(pending.request,reservation).await?;
            }
            let sid = if let Some(sid) = pending.carrier_sid { sid } else {
                let inventory = a.signalwire_client.numbers(&pending.space, &pending.project.to_string(), &token, Some(&pending.number.phone_number)).await?;
                let marker = purchase_marker(pending.workspace,pending.request);
                let Some(number) = inventory.numbers.iter().find(|n| n.phone_number==pending.number.phone_number && n.friendly_name==marker) else {
                    tracing::warn!(workspace=%pending.workspace, request=%pending.request, "Purchase outcome unresolved; carrier reconciliation required");
                    return Ok(());
                };
                store.record_purchase_number(pending.request,&number.sid).await?;
                number.sid.clone()
            };
            if pending.cleanup {
                a.signalwire_client.release(&pending.space,&pending.project.to_string(),&token,&sid).await?;
                store.complete_cleanup(pending.request).await?;
            } else if let Err(error) = store.finish_purchase(pending.request,&pending.number,&sid).await {
                if matches!(error, convert_store::Error::Validation(_) | convert_store::Error::Conflict) {
                    store.cleanup_purchase(pending.request).await?;
                }
                return Err(error.into());
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            tracing::warn!(workspace=%pending.workspace, request=%pending.request, %error, "Purchase recovery will retry");
        }
    }
    Ok(())
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
        if !store.claim_release(due.number_id, hold_days()).await? {
            continue;
        }
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
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(error) = reconcile_purchases(&app).await {
                tracing::error!(%error, "Purchase recovery pass failed");
            }
            if let Err(error) = settle_reservations(&app).await {
                tracing::error!(%error, "Reservation settlement will retry");
            }
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
