//! Service-only bridge into the shared MCP tool dispatcher. The ephemeral signed
//! delegation never leaves the engine; every API call rechecks the Rails grant.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use convert_store::api_keys::KeyAccess;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const SCOPES: &[&str] = &[
    "read",
    "setup:write",
    "numbers:write",
    "leads:write",
    "bookings:write",
];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    workspace_id: Uuid,
    user_id: Uuid,
    credential_version: i64,
    grant_id: Uuid,
    scopes: Vec<String>,
    resource: String,
}
#[derive(Serialize, Deserialize)]
struct Delegation {
    workspace_id: Uuid,
    user_id: Uuid,
    credential_version: i64,
    grant_id: Uuid,
    scopes: Vec<String>,
    expires: i64,
}
fn denied() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, "Invalid MCP delegation".into())
}
fn valid_scopes(scopes: &[String]) -> bool {
    scopes.len() <= SCOPES.len()
        && scopes.iter().any(|s| s == "read")
        && scopes.iter().all(|s| SCOPES.contains(&s.as_str()))
}
fn sign(value: &Delegation, key: &str) -> Result<String, ApiError> {
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).map_err(|_| denied())?);
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    mac.update(b"visiconvert-mcp-delegation-v1:");
    mac.update(body.as_bytes());
    Ok(format!(
        "vc_mcp_{body}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
fn verify(token: &str, key: &str) -> Result<Delegation, ApiError> {
    let (body, signature) = token
        .strip_prefix("vc_mcp_")
        .and_then(|s| s.split_once('.'))
        .ok_or_else(denied)?;
    if body.len() > 4096 {
        return Err(denied());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    mac.update(b"visiconvert-mcp-delegation-v1:");
    mac.update(body.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).map_err(|_| denied())?)
        .map_err(|_| denied())?;
    let value: Delegation =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).map_err(|_| denied())?)
            .map_err(|_| denied())?;
    let now = chrono::Utc::now().timestamp();
    if value.expires <= now
        || value.expires > now + 60
        || value.credential_version < 0
        || !valid_scopes(&value.scopes)
    {
        return Err(denied());
    }
    Ok(value)
}
pub async fn authenticate(app: &App, token: &str) -> Result<KeyAccess, ApiError> {
    let config = app.cloud.as_ref().ok_or_else(denied)?;
    let d = verify(token, &config.service_key)?;
    public_api::hosted_access_with_mcp(
        app,
        d.workspace_id,
        d.user_id,
        d.credential_version,
        Some((d.grant_id, &d.scopes)),
    )
    .await?;
    let name = app
        .store
        .mcp_identity(d.workspace_id, d.user_id, d.credential_version)
        .await?
        .ok_or_else(denied)?;
    Ok(KeyAccess {
        store: Store {
            pool: app.store.pool.clone(),
            workspace: d.workspace_id,
        },
        name,
        actor: format!("MCP connection {}", d.grant_id),
        id: d.grant_id,
        owner: Some(d.user_id),
        credential_version: d.credential_version,
        scopes: d.scopes,
    })
}
async fn handle(app: Shared, input: Input, message: Value) -> Api<Value> {
    let config = app.cloud.as_ref().ok_or_else(denied)?;
    if !valid_scopes(&input.scopes) || input.credential_version < 0 {
        return Err(denied());
    }
    public_api::hosted_access_with_mcp(
        &app,
        input.workspace_id,
        input.user_id,
        input.credential_version,
        Some((input.grant_id, &input.scopes)),
    )
    .await?;
    // Strict loopback-only origin. Never send service-derived authority to a caller-provided URL.
    let origin = std::env::var("VISICONVERT_MCP_LOOPBACK_ORIGIN")
        .unwrap_or_else(|_| "http://127.0.0.1:3100".into());
    let url = url::Url::parse(&origin).map_err(|_| denied())?;
    if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]")) {
        return Err(denied());
    }
    let d = Delegation {
        workspace_id: input.workspace_id,
        user_id: input.user_id,
        credential_version: input.credential_version,
        grant_id: input.grant_id,
        scopes: input.scopes.clone(),
        expires: chrono::Utc::now().timestamp() + 60,
    };
    let token = sign(&d, &config.service_key)?;
    let client = convert_cli::client::Client::new(&origin, &token).map_err(|_| denied())?;
    let server = convert_cli::mcp::Server::scoped(Ok(client), input.scopes);
    Ok(Json(json!({"reply": server.handle(message).await})))
}

fn challenge(app: &App) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"invalid_token"})),
    )
        .into_response();
    if let Some(config) = &app.cloud {
        let value = format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\", scope=\"read\"",
            config.cloud_origin
        );
        if let Ok(value) = value.parse() {
            response.headers_mut().insert("www-authenticate", value);
        }
    }
    response
}
pub async fn remote(
    State(app): State<Shared>,
    headers: HeaderMap,
    method: axum::http::Method,
    body: axum::body::Bytes,
) -> Response {
    let mut response = remote_inner(app, headers, method, body).await;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn remote_inner(
    app: Shared,
    headers: HeaderMap,
    method: axum::http::Method,
    body: axum::body::Bytes,
) -> Response {
    let Some(config) = &app.cloud else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(origin) = headers.get("origin") {
        let origin = origin.to_str().unwrap_or("");
        let extra = std::env::var("VISICONVERT_MCP_ALLOWED_ORIGINS")
            .unwrap_or_else(|_| "https://claude.ai,https://claude.com".into());
        if origin != config.cloud_origin && !extra.split(',').any(|o| !o.is_empty() && o == origin)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if token.len() != 47
        || !token.starts_with("vcm_")
        || !token[4..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return challenge(&app);
    }
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let response = client
        .post(format!("{}/internal/mcp-access", config.cloud_origin))
        .bearer_auth(&config.service_key)
        .json(&json!({"access_token":token}))
        .send()
        .await;
    let Ok(response) = response else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return challenge(&app);
    }
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")]).into_response();
    }
    if !response.status().is_success() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Ok(input) = response.json::<Input>().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if input.resource != format!("{}/mcp", config.cloud_origin) || !valid_scopes(&input.scopes) {
        return challenge(&app);
    }
    if method != axum::http::Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [("allow", "POST")]).into_response();
    }
    if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        != Some("application/json")
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !accept.contains("application/json") || !accept.contains("text/event-stream") {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    let version = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("2025-03-26");
    if !["2025-11-25", "2025-06-18", "2025-03-26"].contains(&version) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if body.len() > 65_536 {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
            ),
        )
            .into_response(),
    };
    if !message.is_object()
        || message["jsonrpc"] != "2.0"
        || !message["method"].is_string()
        || message
            .get("id")
            .is_some_and(|v| !v.is_string() && !v.is_i64() && !v.is_u64())
        || message.get("params").is_some_and(|v| !v.is_object())
    {
        return (StatusCode::BAD_REQUEST, Json(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}))).into_response();
    }
    if message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    match handle(app, input, message).await {
        Ok(Json(reply)) => Json(reply["reply"].clone()).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
    async fn delegation_preserves_engine_membership_and_version_fences() {
        let workspace = Uuid::new_v4();
        let user = Uuid::new_v4();
        let store = Store::connect(
            &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        store
            .provision_hosted(workspace, user, "MCP identity")
            .await
            .unwrap();
        assert_eq!(
            store.mcp_identity(workspace, user, 0).await.unwrap(),
            Some("MCP identity".into())
        );
        assert!(
            store
                .mcp_identity(workspace, Uuid::new_v4(), 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .mcp_identity(Uuid::new_v4(), user, 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .mcp_identity(workspace, user, 1)
                .await
                .unwrap()
                .is_none()
        );
        store.revoke_hosted_user(user, 1).await.unwrap();
        assert!(
            store
                .mcp_identity(workspace, user, 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .mcp_identity(workspace, user, 1)
                .await
                .unwrap()
                .is_some()
        );
        sqlx::query(
            "UPDATE hosted_memberships SET enabled=false WHERE workspace_id=$1 AND user_id=$2",
        )
        .bind(workspace)
        .bind(user)
        .execute(&store.pool)
        .await
        .unwrap();
        assert!(
            store
                .mcp_identity(workspace, user, 1)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn delegation_rejects_tampering_expiry_and_unknown_scopes() {
        let mut d = Delegation {
            workspace_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            credential_version: 0,
            grant_id: Uuid::new_v4(),
            scopes: vec!["read".into()],
            expires: chrono::Utc::now().timestamp() + 30,
        };
        let token = sign(&d, "secret").ok().unwrap();
        assert!(verify(&token, "secret").is_ok());
        assert!(verify(&token, "other").is_err());
        assert!(verify(&format!("{token}x"), "secret").is_err());
        d.expires = chrono::Utc::now().timestamp() - 1;
        assert!(verify(&sign(&d, "secret").ok().unwrap(), "secret").is_err());
        d.expires += 30;
        d.scopes.push("admin".into());
        assert!(verify(&sign(&d, "secret").ok().unwrap(), "secret").is_err());
    }
}
