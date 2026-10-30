//! Optional, browser-bound cloud handoff. Service credentials never enter the browser.
use super::*;
use axum::{extract::Query, http::header, response::Redirect};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct Config {
    pub service_key: String,
    pub cloud_origin: String,
    pub app_origin: Option<String>,
}
pub fn browser_origin(app: &App) -> &str {
    app.cloud
        .as_ref()
        .and_then(|config| config.app_origin.as_deref())
        .unwrap_or(&app.public_url)
}
#[derive(Clone)]
pub struct Access {
    pub store: Store,
    pub name: String,
    pub session_hash: Option<String>,
}
pub fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
pub fn origin(value: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(value)?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    anyhow::ensure!(
        (url.scheme() == "https" || (url.scheme() == "http" && loopback))
            && url.host_str().is_some()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "Hosted origins require HTTPS (HTTP is allowed only on loopback)"
    );
    Ok(url.origin().ascii_serialization())
}
pub fn configuration(public_url: &str, operator_key: &str) -> anyhow::Result<Option<Config>> {
    let key = std::env::var("VISICONVERT_CLOUD_SERVICE_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let cloud = std::env::var("VISICONVERT_CLOUD_ORIGIN")
        .ok()
        .filter(|s| !s.is_empty());
    let app_origin = std::env::var("VISICONVERT_APP_ORIGIN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|value| origin(&value))
        .transpose()?;
    match (key, cloud) {
        (None, None) => {
            anyhow::ensure!(
                app_origin.is_none(),
                "VISICONVERT_APP_ORIGIN requires hosted Cloud configuration"
            );
            Ok(None)
        }
        (Some(service_key), Some(cloud)) => {
            anyhow::ensure!(
                hex64(&service_key) && service_key != operator_key,
                "Use an independent 64-character hex cloud service key"
            );
            let cloud_origin = origin(&cloud)?;
            if let Some(app_origin) = &app_origin {
                anyhow::ensure!(
                    app_origin == &cloud_origin,
                    "The unified app origin must equal VISICONVERT_CLOUD_ORIGIN"
                );
            } else {
                anyhow::ensure!(
                    origin(public_url)? != cloud_origin,
                    "Set VISICONVERT_APP_ORIGIN for a same-origin hosted app"
                );
            }
            Ok(Some(Config {
                service_key,
                cloud_origin,
                app_origin,
            }))
        }
        _ => anyhow::bail!(
            "Set both VISICONVERT_CLOUD_SERVICE_KEY and VISICONVERT_CLOUD_ORIGIN, or neither"
        ),
    }
}
pub fn routes(app: Shared) -> Router<Shared> {
    Router::new()
        .route("/internal/cloud/workspaces", post(provision))
        .route(
            "/internal/cloud/workspaces/{workspace}/carrier-adoption/preview",
            post(super::adoption::preview),
        )
        .route(
            "/internal/cloud/workspaces/{workspace}/carrier-adoption",
            post(super::adoption::apply),
        )
        .route(
            "/internal/cloud/workspaces/{workspace}/carrier-adoption/status",
            post(super::adoption::status),
        )
        .route("/internal/cloud/launch", post(launch))
        .route("/internal/cloud/revoke-user", post(revoke_user))
        .route(
            "/internal/cloud/workspaces/{workspace}/signalwire",
            post(super::signalwire::save_managed),
        )
        .route(
            "/internal/cloud/workspaces/{workspace}/usage",
            post(super::managed_usage::report),
        )
        .route(
            "/internal/cloud/workspaces/{workspace}/service-policy",
            post(super::service_policy::apply),
        )
        .route_layer(middleware::from_fn_with_state(app, service_auth))
        .route("/auth/start", get(start))
        .route("/auth/prepare", post(prepare))
        .route("/auth/exchange", post(exchange))
}
async fn prepare(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Result<Response, ApiError> {
    let Some(config) = &app.cloud else {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found".into()));
    };
    if config.app_origin.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found".into()));
    }
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(browser_origin(&app)) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Open the workspace from VisiConvert.".into(),
        ));
    }
    if !app.store.hosted_exists(input.workspace_id).await? {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Workspace not available".into(),
        ));
    }
    let secret = random_secret();
    let secure = if browser_origin(&app).starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    Ok((
        [(
            header::SET_COOKIE,
            format!("vc_handoff={secret}; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=300{secure}"),
        )],
        Json(json!({"state":digest(&secret)})),
    )
        .into_response())
}
async fn service_auth(State(app): State<Shared>, request: Request, next: Next) -> Response {
    let Some(config) = &app.cloud else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let supplied = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if supplied
        .as_bytes()
        .ct_eq(config.service_key.as_bytes())
        .into()
    {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
#[derive(Deserialize)]
struct Provision {
    workspace_id: Uuid,
    user_id: Uuid,
    name: String,
}
async fn provision(State(app): State<Shared>, Json(input): Json<Provision>) -> Api<Value> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use a workspace name of 1–100 characters".into(),
        ));
    }
    app.store
        .provision_hosted(input.workspace_id, input.user_id, name)
        .await?;
    Ok(Json(
        json!({"workspace_id":input.workspace_id,"status":"ready"}),
    ))
}
#[derive(Deserialize)]
struct Launch {
    workspace_id: Uuid,
    user_id: Uuid,
    state: String,
    #[serde(default)]
    credential_version: i64,
}
async fn launch(State(app): State<Shared>, Json(input): Json<Launch>) -> Api<Value> {
    if !hex64(&input.state) || input.credential_version < 0 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Invalid handoff state".into(),
        ));
    }
    let code = format!("vc_code_{}", random_secret());
    if !app
        .store
        .issue_hosted_code(
            input.workspace_id,
            input.user_id,
            &input.state,
            &digest(&code),
            input.credential_version,
        )
        .await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Workspace access unavailable".into(),
        ));
    }
    Ok(Json(json!({"code":code,"expires_in":60})))
}
#[derive(Deserialize)]
struct RevokeUser {
    user_id: Uuid,
    credential_version: i64,
}
async fn revoke_user(State(app): State<Shared>, Json(input): Json<RevokeUser>) -> Api<Value> {
    if input.credential_version < 1 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Invalid credential version".into(),
        ));
    }
    let version = app
        .store
        .revoke_hosted_user(input.user_id, input.credential_version)
        .await?;
    Ok(Json(
        json!({"user_id":input.user_id,"credential_version":version}),
    ))
}
#[derive(Deserialize)]
struct Start {
    workspace_id: Uuid,
}
async fn start(
    State(app): State<Shared>,
    Query(input): Query<Start>,
) -> Result<Response, ApiError> {
    let config = app
        .cloud
        .as_ref()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Not found".into()))?;
    if !app.store.hosted_exists(input.workspace_id).await? {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Workspace not available".into(),
        ));
    }
    let secret = random_secret();
    let destination = format!(
        "{}/workspaces/{}/authorize?state={}",
        config.cloud_origin,
        input.workspace_id,
        digest(&secret)
    );
    let mut response = Redirect::to(&destination).into_response();
    let secure = if app.public_url.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("vc_handoff={secret}; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=300{secure}")
            .parse()
            .unwrap(),
    );
    Ok(response)
}
#[derive(Deserialize)]
struct Exchange {
    code: String,
}
async fn exchange(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Exchange>,
) -> Result<Response, ApiError> {
    if app.cloud.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "Not found".into()));
    }
    let fail = || {
        ApiError(StatusCode::UNAUTHORIZED,"This sign-in link expired or belongs to another browser. Open the workspace again from VisiConvert Cloud.".into())
    };
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    if origin != Some(browser_origin(&app)) && origin != Some(app.public_url.as_str()) {
        return Err(fail());
    }
    let secret = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .find_map(|v| v.trim().strip_prefix("vc_handoff="))
        .filter(|v| hex64(v))
        .ok_or_else(fail)?;
    if !input.code.strip_prefix("vc_code_").is_some_and(hex64) {
        return Err(fail());
    }
    let token = format!("vc_session_{}", random_secret());
    if !app
        .store
        .exchange_hosted_code(&digest(&input.code), &digest(secret), &digest(&token))
        .await?
    {
        return Err(fail());
    }
    let mut response = Json(json!({"token":token,"expires_in":900})).into_response();
    let secure = if app.public_url.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("vc_handoff=; HttpOnly; SameSite=Lax; Path=/auth; Max-Age=0{secure}")
            .parse()
            .unwrap(),
    );
    Ok(response)
}
pub async fn logout(axum::Extension(access): axum::Extension<Access>) -> Api<Value> {
    if let Some(hash) = access.session_hash {
        access.store.revoke_hosted_session(&hash).await?;
    }
    Ok(Json(json!({"ok":true})))
}
