//! Owner-only Google OAuth and read-only destination verification.
use super::*;
use axum::{extract::Query, http::header, response::Redirect};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use convert_adapters::google_api::{self};
use convert_store::google::{Attempt, Connection};
use serde::Deserialize;
use sha2::{Digest, Sha256};
const CALLBACK: &str = "/auth/google/callback";
const COOKIE: &str = "vc_google_browser";
#[derive(Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub redirect: String,
}
impl Config {
    pub fn from_env(origin: &str, vault: bool) -> anyhow::Result<Option<Self>> {
        let get = |name| std::env::var(name).ok().filter(|s| !s.is_empty());
        match (
            get("VISICONVERT_GOOGLE_CLIENT_ID"),
            get("VISICONVERT_GOOGLE_CLIENT_SECRET"),
        ) {
            (None, None) => Ok(None),
            (Some(id), Some(secret)) => {
                anyhow::ensure!(vault, "Google OAuth requires VISICONVERT_ENCRYPTION_KEY");
                let origin = hosted::origin(origin)?;
                anyhow::ensure!(
                    id.ends_with(".apps.googleusercontent.com")
                        && id.len() < 256
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)),
                    "Invalid Google OAuth client ID"
                );
                anyhow::ensure!(
                    (16..=256).contains(&secret.len())
                        && secret
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
                    "Invalid Google OAuth client secret"
                );
                Ok(Some(Self {
                    client_id: id,
                    client_secret: secret,
                    redirect: format!("{origin}{CALLBACK}"),
                }))
            }
            _ => anyhow::bail!(
                "Set both VISICONVERT_GOOGLE_CLIENT_ID and VISICONVERT_GOOGLE_CLIENT_SECRET, or neither"
            ),
        }
    }
    fn fingerprint(&self) -> String {
        hosted::digest(&format!("{}\n{}", self.client_id, self.redirect))
    }
    fn url(&self, state: &str, verifier: &str) -> String {
        let mut u = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        u.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect.as_str()),
            ("response_type", "code"),
            ("scope", google_api::SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
            ("state", state),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]);
        u.into()
    }
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn config(a: &App) -> Result<&Config, ApiError> {
    a.google.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Your administrator must configure Google OAuth before connecting.".into(),
        )
    })
}
async fn owner(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(h) = &access.session_hash
        && !access.store.manages_connection(h).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can manage Google connections".into(),
        ));
    }
    Ok(())
}
impl From<google_api::Error> for ApiError {
    fn from(e: google_api::Error) -> Self {
        Self(
            match e {
                google_api::Error::Credentials
                | google_api::Error::Scope
                | google_api::Error::Access => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            },
            e.to_string(),
        )
    }
}
pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/connections/google", get(show).delete(disconnect))
        .route("/connections/google/start", post(start))
        .route("/connections/google/accounts", get(accounts))
        .route("/connections/google/actions", get(actions))
        .route("/connections/google/validate", post(validate))
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
async fn show(State(a): State<Shared>, Extension(access): Extension<hosted::Access>) -> Api<Value> {
    owner(&access).await?;
    let c = access.store.google_connection().await?;
    let configured = a.google.is_some() && a.vault.is_some();
    let matching = a
        .google
        .as_ref()
        .is_some_and(|g| c.client_fingerprint.as_deref() == Some(g.fingerprint().as_str()));
    Ok(Json(
        json!({"configured":configured,"connected":c.encrypted_token.is_some() && matching,
        "reconnect_required":c.encrypted_token.is_some() && !matching,"revision":c.revision,"connected_at":c.connected_at,
        "redirect_uri":format!("{}{CALLBACK}",hosted::browser_origin(&a)),"validation":if matching {c.validation}else{None},"checked_at":if matching {c.checked_at}else{None},"uploads_enabled":access.store.delivery_settings().await?["enabled"]}),
    ))
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
        .seal_google(
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
    access.store.begin_google_oauth(&attempt).await?;
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
        Redirect::to(&format!("/auth/google/result?status={status}")).into_response();
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
            "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Google connected — VisiConvert</title><h1>Google connected</h1><p>Return to VisiConvert to select and validate your conversion destination. No conversions were uploaded. You can close this window.</p><script src='/auth/google/result.js' defer></script></html>"
        } else {
            "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Google connection — VisiConvert</title><h1>Google could not be connected</h1><p>The request expired, was canceled, or could not be verified. Return to VisiConvert and try again, granting both Google Ads and Data Manager permissions. You can close this window.</p><script src='/auth/google/result.js' defer></script></html>"
        },
    )
}
pub async fn result_script() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        "if (window.opener) { window.opener.postMessage({type:'visiconvert-google-result',status:new URLSearchParams(location.search).get('status')},location.origin); window.close(); }",
    )
}
async fn complete(a: &App, headers: &HeaderMap, q: Callback) -> Result<(), ApiError> {
    let invalid = || {
        ApiError(
            StatusCode::BAD_REQUEST,
            "Google connection expired, was canceled, or could not be verified. Start again."
                .into(),
        )
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
        .consume_google_oauth(&hosted::digest(&state), &hosted::digest(cookies[0]))
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
        .open_google(
            store.workspace,
            &format!("pkce:{}", attempt.state_hash),
            &attempt.encrypted_verifier,
        )
        .map_err(|_| connections::vault_error())?;
    let tokens = a
        .google_client
        .exchange(
            &g.client_id,
            &g.client_secret,
            &g.redirect,
            &code,
            &verifier,
        )
        .await?;
    let refresh = tokens.refresh_token.ok_or(google_api::Error::Credentials)?;
    let encrypted = vault
        .seal_google(store.workspace, "refresh", &refresh)
        .map_err(|_| connections::vault_error())?;
    store.finish_google_oauth(&attempt, &encrypted).await?;
    Ok(())
}
async fn token(a: &App, access: &hosted::Access) -> Result<(Connection, String), ApiError> {
    owner(access).await?;
    worker_token(a, &access.store).await
}
pub(crate) async fn worker_token(a: &App, store: &Store) -> Result<(Connection, String), ApiError> {
    let g = config(a)?;
    let c = store.google_connection().await?;
    if c.client_fingerprint.as_deref() != Some(g.fingerprint().as_str()) {
        return Err(google_api::Error::Credentials.into());
    }
    let vault = a.vault.as_ref().ok_or_else(connections::vault_error)?;
    let encrypted = c
        .encrypted_token
        .as_deref()
        .ok_or(google_api::Error::Credentials)?;
    let refresh = vault
        .open_google(store.workspace, "refresh", encrypted)
        .map_err(|_| connections::vault_error())?;
    let t = a
        .google_client
        .refresh(&g.client_id, &g.client_secret, &refresh)
        .await?;
    // Google normally does not rotate refresh tokens on refresh. Preserve any
    // replacement atomically if it does, without resurrecting a disconnected link.
    if let Some(next) = t.refresh_token
        && next != refresh
    {
        let encrypted = vault
            .seal_google(store.workspace, "refresh", &next)
            .map_err(|_| connections::vault_error())?;
        store
            .rotate_google_refresh(
                c.revision,
                c.encrypted_token.as_deref().unwrap(),
                &encrypted,
            )
            .await?;
    }
    Ok((c, t.access_token))
}
#[derive(Deserialize)]
struct Discovery {
    login: Option<String>,
    customer: Option<String>,
}
async fn accounts(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Query(q): Query<Discovery>,
) -> Api<Value> {
    let (_, token) = token(&a, &access).await?;
    let logins = a.google_client.logins(&token).await?;
    if let Some(login) = q.login {
        if !logins.contains(&login) {
            return Err(google_api::Error::Access.into());
        }
        let (accounts, more) = a.google_client.accounts(&token, &login).await?;
        Ok(Json(
            json!({"logins":logins,"accounts":accounts,"has_more":more}),
        ))
    } else {
        Ok(Json(
            json!({"logins":logins,"accounts":[],"has_more":false}),
        ))
    }
}
async fn actions(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Query(q): Query<Discovery>,
) -> Api<Value> {
    let (_, token) = token(&a, &access).await?;
    let login = q.login.ok_or(google_api::Error::Access)?;
    let customer = q.customer.ok_or(google_api::Error::Access)?;
    if !a.google_client.logins(&token).await?.contains(&login) {
        return Err(google_api::Error::Access.into());
    }
    let (account, actions, more) = a.google_client.actions(&token, &login, &customer).await?;
    Ok(Json(
        json!({"account":account,"actions":actions,"has_more":more}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    revision: i64,
    settings_version: i32,
    login: String,
}
async fn validate(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Validation>,
) -> Api<Value> {
    owner(&access).await?;
    access.store.clear_google_validation(input.revision).await?;
    let (connection, token) = token(&a, &access).await?;
    let settings = access.store.google_conversion_settings().await?;
    if connection.revision != input.revision
        || settings.version != input.settings_version
        || settings.version == 0
    {
        return Err(convert_store::Error::Conflict.into());
    }
    let result = async {
        if !a.google_client.logins(&token).await?.contains(&input.login) {
            return Err(google_api::Error::Access);
        }
        a.google_client
            .actions(&token, &input.login, &settings.customer_id)
            .await
    }
    .await;
    let report = match result {
        Ok((account, actions, more)) => {
            let chosen: Vec<_> = [&settings.qualified_action_id, &settings.booking_action_id]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            let valid = account["status"] == "ENABLED"
                && !chosen.is_empty()
                && chosen.iter().all(|id| {
                    actions
                        .iter()
                        .any(|a| a.id.as_str() == id.as_str() && a.eligible)
                });
            let selected: Vec<_> = actions
                .iter()
                .filter(|a| chosen.iter().any(|id| a.id.as_str() == id.as_str()))
                .collect();
            json!({"valid":valid,"login_customer_id":input.login,"account":account,"actions":selected,"settings_version":settings.version,"uploads_enabled":access.store.delivery_settings().await?["enabled"],
                "message":if valid {"Account is readable and the selected actions are enabled import actions owned by this account. Upload acceptance and attribution are not checked."}else if more {"A selected action is not verified in the first 200 results. Narrow the destination or check the action IDs."}else{"Check that this account is enabled and owns every selected, enabled import conversion action. Select the conversion-owning account when using cross-account tracking."}})
        }
        Err(e) => {
            json!({"valid":false,"settings_version":settings.version,"uploads_enabled":access.store.delivery_settings().await?["enabled"],"message":e.to_string()})
        }
    };
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    owner(&access).await?;
    access
        .store
        .save_google_validation(connection.revision, settings.version, &report, &actor)
        .await?;
    Ok(Json(report))
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
        .disconnect_google(input.revision, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
