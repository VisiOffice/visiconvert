use super::*;
use convert_adapters::twilio_api;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/connections/twilio",
            get(show).put(save).delete(disconnect),
        )
        .route("/connections/twilio/numbers", get(inventory))
}

async fn authorize(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash
        && !access.store.manages_connection(hash).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can manage phone connections".into(),
        ));
    }
    Ok(())
}

fn legacy(a: &App, access: &hosted::Access) -> bool {
    access.session_hash.is_none()
        && access.store.workspace == a.store.workspace
        && a.twilio_token.is_some()
}

fn available(a: &App) -> Option<&'static str> {
    if a.vault.is_none() {
        Some(
            "Your administrator must configure the server encryption key before saving provider credentials.",
        )
    } else if !a.public_url.starts_with("https://") {
        Some("Your administrator must configure an HTTPS public URL before connecting Twilio.")
    } else {
        None
    }
}

async fn show(State(a): State<Shared>, Extension(access): Extension<hosted::Access>) -> Api<Value> {
    authorize(&access).await?;
    let connection = access.store.twilio_connection().await?;
    let environment = legacy(&a, &access);
    let mode = if connection.is_some() {
        "workspace"
    } else if environment {
        "environment"
    } else {
        "disconnected"
    };
    let prefix = if environment {
        "/webhooks/twilio".to_string()
    } else {
        format!("/webhooks/twilio/workspaces/{}", access.store.workspace)
    };
    let active_numbers = access
        .store
        .numbers()
        .await?
        .iter()
        .filter(|n| !n.is_demo)
        .count();
    Ok(Json(json!({
        "mode": mode, "connected": connection.is_some() || environment,
        "account_sid": connection.as_ref().map(|c| c.account_sid.as_str()).or(if environment { a.twilio_account.as_deref() } else { None }),
        "revision": connection.as_ref().map_or(0, |c| c.revision),
        "setup_available": !environment && available(&a).is_none(),
        "setup_message": if environment { Some("This installation uses environment credentials. Keep them until your administrator completes the workspace migration.") } else { available(&a) },
        "can_disconnect": connection.is_some() && active_numbers == 0,
        "voice_url": format!("{}{prefix}/voice", a.public_url),
        "status_url": format!("{}{prefix}/status", a.public_url)
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    account_sid: String,
    auth_token: String,
    revision: i64,
}

impl From<twilio_api::Error> for ApiError {
    fn from(error: twilio_api::Error) -> Self {
        let status = match error {
            twilio_api::Error::Credentials | twilio_api::Error::Number => StatusCode::BAD_REQUEST,
            twilio_api::Error::Unavailable | twilio_api::Error::Response => StatusCode::BAD_GATEWAY,
        };
        Self(status, error.to_string())
    }
}

async fn save(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Credentials>,
) -> Api<Value> {
    authorize(&access).await?;
    if legacy(&a, &access) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Migrate the existing environment configuration before saving workspace credentials"
                .into(),
        ));
    }
    if let Some(reason) = available(&a) {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, reason.into()));
    }
    if !twilio::valid_sid(&input.account_sid, "AC")
        || input.auth_token.len() != 32
        || !input.auth_token.bytes().all(|c| c.is_ascii_hexdigit())
        || input.revision < 0
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid Twilio Account SID and 32-character Auth Token".into(),
        ));
    }
    a.twilio_client
        .verify_account(&input.account_sid, &input.auth_token)
        .await?;
    let encrypted = a
        .vault
        .as_ref()
        .unwrap()
        .seal(
            access.store.workspace,
            &input.account_sid,
            &input.auth_token,
        )
        .map_err(|_| vault_error())?;
    access
        .store
        .save_twilio_connection(&input.account_sid, &encrypted, input.revision)
        .await?;
    Ok(Json(json!({"ok":true})))
}

pub fn vault_error() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Provider credentials are unavailable. Ask the administrator to check the encryption key."
            .into(),
    )
}

pub fn decrypt(
    a: &App,
    store: &Store,
    connection: &convert_store::connections::TwilioConnection,
) -> Result<String, ApiError> {
    a.vault
        .as_ref()
        .ok_or_else(vault_error)?
        .open(
            store.workspace,
            &connection.account_sid,
            &connection.encrypted_token,
        )
        .map_err(|_| vault_error())
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
    authorize(&access).await?;
    access
        .store
        .remove_twilio_connection(input.revision)
        .await?;
    Ok(Json(json!({"ok":true})))
}

async fn inventory(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<twilio_api::Inventory> {
    authorize(&access).await?;
    if let Some(connection) = access.store.twilio_connection().await? {
        let token = decrypt(&a, &access.store, &connection)?;
        return Ok(Json(
            a.twilio_client
                .numbers(&connection.account_sid, &token, None)
                .await?,
        ));
    }
    if legacy(&a, &access) {
        return Ok(Json(
            a.twilio_client
                .numbers(
                    a.twilio_account.as_deref().unwrap(),
                    a.twilio_token.as_deref().unwrap(),
                    None,
                )
                .await?,
        ));
    }
    Err(ApiError(
        StatusCode::CONFLICT,
        "Connect Twilio before loading numbers".into(),
    ))
}

pub async fn register(a: &App, access: &hosted::Access, n: &NewNumber) -> Result<(), ApiError> {
    authorize(access).await?;
    n.validate()
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    if let Some(connection) = access.store.twilio_connection().await? {
        let token = decrypt(a, &access.store, &connection)?;
        a.twilio_client
            .verify_number(&connection.account_sid, &token, &n.phone_number)
            .await?;
        access
            .store
            .add_number_with_connection(n, Some(connection.revision))
            .await?;
    } else {
        if access.session_hash.is_some() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Connect Twilio in Connections before registering a number".into(),
            ));
        }
        if legacy(a, access) {
            a.twilio_client
                .verify_number(
                    a.twilio_account.as_deref().unwrap(),
                    a.twilio_token.as_deref().unwrap(),
                    &n.phone_number,
                )
                .await?;
        }
        // Preserve offline self-hosted registration. Without credentials it cannot route calls.
        access.store.add_number(n).await?;
    }
    Ok(())
}

pub async fn webhook(
    State(a): State<Shared>,
    Path((workspace, endpoint)): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    if !["voice", "dial-complete", "status"].contains(&endpoint.as_str()) {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found".into()));
    }
    if !headers.contains_key("x-twilio-signature") {
        return Err(ApiError(StatusCode::FORBIDDEN, "Invalid webhook".into()));
    }
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    let connection = store
        .twilio_connection()
        .await?
        .ok_or_else(|| ApiError(StatusCode::FORBIDDEN, "Invalid webhook".into()))?;
    let token = decrypt(&a, &store, &connection)?;
    let prefix = format!("/webhooks/twilio/workspaces/{workspace}");
    super::process_webhook(
        &store,
        &a.public_url,
        &prefix,
        &connection.account_sid,
        &token,
        &endpoint,
        headers,
        body,
    )
    .await
}
