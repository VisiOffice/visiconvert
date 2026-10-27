use super::*;
use convert_adapters::signalwire_api;
use convert_store::signalwire::SignalWireConnection;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/connections/signalwire",
            get(show).put(save).delete(disconnect),
        )
        .route("/connections/signalwire/numbers", get(inventory))
}
fn available(a: &App) -> Option<&'static str> {
    if a.vault.is_none() {
        Some(
            "Your administrator must configure the server encryption key before saving provider credentials.",
        )
    } else if !a.public_url.starts_with("https://") {
        Some("Your administrator must configure an HTTPS public URL before connecting SignalWire.")
    } else {
        None
    }
}
pub(super) async fn show(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    connections::authorize(&access).await?;
    let connection = access.store.signalwire_connection().await?;
    let used = access
        .store
        .numbers()
        .await?
        .iter()
        .any(|n| !n.is_demo && n.provider == PhoneProvider::Signalwire);
    let prefix = format!(
        "{}/webhooks/signalwire/workspaces/{}",
        a.public_url, access.store.workspace
    );
    Ok(Json(json!({
        "connected": connection.is_some(), "space_host": connection.as_ref().map(|c| &c.space_host),
        "project_id": connection.as_ref().map(|c| c.project_id), "revision": connection.as_ref().map_or(0, |c| c.revision),
        "last_webhook_at": connection.as_ref().and_then(|c| c.last_webhook_at),
        "setup_available": available(&a).is_none(), "setup_message": available(&a),
        "can_disconnect": connection.is_some() && !used,
        "voice_url": format!("{prefix}/voice"), "status_url": format!("{prefix}/status")
    })))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    space_host: String,
    project_id: Uuid,
    api_token: String,
    signing_key: String,
    revision: i64,
}
impl From<signalwire_api::Error> for ApiError {
    fn from(error: signalwire_api::Error) -> Self {
        let status = match error {
            signalwire_api::Error::Credentials | signalwire_api::Error::Number => {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::BAD_GATEWAY,
        };
        Self(status, error.to_string())
    }
}
async fn save(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Credentials>,
) -> Api<Value> {
    connections::authorize(&access).await?;
    if let Some(reason) = available(&a) {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, reason.into()));
    }
    let secret_valid =
        |s: &str| (16..=512).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic());
    if !signalwire_api::valid_space(&input.space_host)
        || input.project_id.is_nil()
        || !secret_valid(&input.api_token)
        || !secret_valid(&input.signing_key)
        || input.revision < 0
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Enter a SignalWire Space hostname, Project ID, API token, and separate webhook signing key".into()));
    }
    let project = input.project_id.to_string();
    a.signalwire_client
        .verify_account(&input.space_host, &project, &input.api_token)
        .await?;
    // Prove the inventory permission as well as account access before saving.
    a.signalwire_client
        .numbers(&input.space_host, &project, &input.api_token, None)
        .await?;
    let vault = a.vault.as_ref().ok_or_else(connections::vault_error)?;
    let token = vault
        .seal_signalwire(
            access.store.workspace,
            &input.space_host,
            input.project_id,
            "api-token",
            &input.api_token,
        )
        .map_err(|_| connections::vault_error())?;
    let signing = vault
        .seal_signalwire(
            access.store.workspace,
            &input.space_host,
            input.project_id,
            "signing-key",
            &input.signing_key,
        )
        .map_err(|_| connections::vault_error())?;
    access
        .store
        .save_signalwire_connection(
            &input.space_host,
            input.project_id,
            &signing,
            &token,
            input.revision,
        )
        .await?;
    Ok(Json(json!({"ok":true})))
}
fn decrypt(
    a: &App,
    store: &Store,
    c: &SignalWireConnection,
    signing: bool,
) -> Result<String, ApiError> {
    a.vault
        .as_ref()
        .ok_or_else(connections::vault_error)?
        .open_signalwire(
            store.workspace,
            &c.space_host,
            c.project_id,
            if signing { "signing-key" } else { "api-token" },
            if signing {
                &c.encrypted_signing_key
            } else {
                &c.encrypted_token
            },
        )
        .map_err(|_| connections::vault_error())
}
async fn connection(store: &Store) -> Result<SignalWireConnection, ApiError> {
    store.signalwire_connection().await?.ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "Connect SignalWire in Connections first".into(),
        )
    })
}
pub(super) async fn inventory(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<convert_adapters::twilio_api::Inventory> {
    connections::authorize(&access).await?;
    let c = connection(&access.store).await?;
    let token = decrypt(&a, &access.store, &c, false)?;
    Ok(Json(
        a.signalwire_client
            .numbers(&c.space_host, &c.project_id.to_string(), &token, None)
            .await?,
    ))
}
pub async fn register(a: &App, access: &hosted::Access, n: &NewNumber) -> Result<(), ApiError> {
    let c = connection(&access.store).await?;
    let token = decrypt(a, &access.store, &c, false)?;
    a.signalwire_client
        .verify_number(
            &c.space_host,
            &c.project_id.to_string(),
            &token,
            &n.phone_number,
        )
        .await?;
    access
        .store
        .add_number_with_connection(n, Some(c.revision))
        .await?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: i64,
}
async fn disconnect(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Revision>,
) -> Api<Value> {
    connections::authorize(&access).await?;
    access
        .store
        .remove_signalwire_connection(input.revision)
        .await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn webhook(
    State(a): State<Shared>,
    Path((workspace, endpoint)): Path<(Uuid, String)>,
    mut headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let denied = || ApiError(StatusCode::FORBIDDEN, "Invalid webhook".into());
    if !["voice", "dial-complete", "status"].contains(&endpoint.as_str()) {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found".into()));
    }
    // Compatibility callbacks may use either header name. Reject ambiguous values.
    if headers.get_all("x-signalwire-signature").iter().count() > 1
        || headers.get_all("x-twilio-signature").iter().count() > 1
    {
        return Err(denied());
    }
    let signature = headers
        .get("x-signalwire-signature")
        .or_else(|| headers.get("x-twilio-signature"))
        .ok_or_else(denied)?
        .clone();
    if headers
        .get("x-twilio-signature")
        .is_some_and(|v| v != signature)
    {
        return Err(denied());
    }
    headers.insert("x-twilio-signature", signature);
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    let c = store.signalwire_connection().await?.ok_or_else(denied)?;
    let signing = decrypt(&a, &store, &c, true)?;
    let prefix = format!("/webhooks/signalwire/workspaces/{workspace}");
    let result = process_provider_webhook(
        &store,
        &a.public_url,
        &prefix,
        &c.project_id.to_string(),
        &signing,
        &endpoint,
        headers,
        body,
        PhoneProvider::Signalwire,
    )
    .await?;
    store.record_signalwire_webhook(c.revision).await?;
    Ok(result)
}
