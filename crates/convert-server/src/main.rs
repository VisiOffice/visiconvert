mod adoption;
mod api_keys;
mod connections;
mod conversions;
mod delivery;
mod google;
mod hosted;
mod inquiries;
mod managed;
mod managed_usage;
mod public_api;
mod remote_mcp;
mod secrets;
mod service_policy;
mod signalwire;
mod sites;
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use clap::{Parser, Subcommand};
use convert_adapters::twilio;
use convert_domain::*;
use convert_store::{CallEvent, CallEventKind, Store};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Serve,
    Migrate,
    SeedDemo,
    /// Reconcile purchase intents and billing holds once; never buy new numbers.
    ReconcileNumberPurchases,
}
#[derive(Clone)]
struct App {
    store: Store,
    api_key: String,
    public_url: String,
    twilio_account: Option<String>,
    twilio_token: Option<String>,
    cloud: Option<hosted::Config>,
    vault: Option<secrets::Vault>,
    twilio_client: convert_adapters::twilio_api::Client,
    signalwire_client: convert_adapters::signalwire_api::Client,
    google: Option<google::Config>,
    google_client: convert_adapters::google_api::Client,
}
type Shared = Arc<App>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "convert_server=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let database = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL is required; see .env.example"))?;
    let workspace = std::env::var("VISICONVERT_WORKSPACE_ID")
        .unwrap_or_else(|_| "a5bc0083-3fa8-44b6-b0a6-c2d5da07b300".into())
        .parse()?;
    let store = Store::connect(&database, workspace).await?;
    if matches!(cli.command, Some(Command::Migrate)) {
        store.migrate().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("Migrations applied.");
        return Ok(());
    }
    if matches!(cli.command, Some(Command::SeedDemo)) {
        store.seed_demo().await?;
        println!("Demo leads ready.");
        return Ok(());
    }
    let api_key = std::env::var("VISICONVERT_API_KEY").unwrap_or_default();
    anyhow::ensure!(
        api_key.len() >= 32,
        "Set VISICONVERT_API_KEY to a random secret of at least 32 characters"
    );
    let public_url = std::env::var("PUBLIC_URL")
        .unwrap_or_else(|_| "http://localhost:3100".into())
        .trim_end_matches('/')
        .to_string();
    let parsed = url::Url::parse(&public_url)?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "PUBLIC_URL must be an HTTP(S) origin without a path, query, or credentials"
    );
    let optional = |name| std::env::var(name).ok().filter(|s| !s.is_empty());
    let account = optional("TWILIO_ACCOUNT_SID");
    let token = optional("TWILIO_AUTH_TOKEN");
    anyhow::ensure!(
        account.is_some() == token.is_some(),
        "Set both TWILIO_ACCOUNT_SID and TWILIO_AUTH_TOKEN, or neither"
    );
    if let Some(account) = &account {
        anyhow::ensure!(
            twilio::valid_sid(account, "AC") && parsed.scheme() == "https",
            "Live Twilio requires a valid account SID and HTTPS PUBLIC_URL"
        );
    }
    let cloud = hosted::configuration(&public_url, &api_key)?;
    anyhow::ensure!(
        account.is_none() || store.twilio_connection().await?.is_none(),
        "Remove legacy TWILIO_ACCOUNT_SID/TWILIO_AUTH_TOKEN when using a saved workspace connection"
    );
    let vault = optional("VISICONVERT_ENCRYPTION_KEY")
        .map(|key| {
            anyhow::ensure!(
                key != api_key,
                "Use a separate VISICONVERT_ENCRYPTION_KEY, not the workspace API key"
            );
            secrets::Vault::from_hex(&key)
        })
        .transpose()?;
    let browser_origin = cloud
        .as_ref()
        .and_then(|config| config.app_origin.as_deref())
        .unwrap_or(&public_url);
    let google = google::Config::from_env(browser_origin, vault.is_some())?;
    let app = Arc::new(App {
        store,
        api_key,
        public_url,
        twilio_account: account,
        twilio_token: token,
        cloud,
        vault,
        twilio_client: convert_adapters::twilio_api::Client::new()?,
        signalwire_client: convert_adapters::signalwire_api::Client::new()?,
        google,
        google_client: convert_adapters::google_api::Client::new()?,
    });
    if matches!(cli.command, Some(Command::ReconcileNumberPurchases)) {
        managed::reconcile_purchases(&app).await?;
        managed::settle_reservations(&app).await?;
        println!("{}", app.store.purchase_recovery_summary().await?);
        return Ok(());
    }
    let address: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3100".into())
        .parse()?;
    if app.vault.is_some() {
        managed::spawn_release_loop(app.clone());
    }
    if app.google.is_some() {
        delivery::spawn(app.clone());
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address,"VisiConvert pilot listening");
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
fn router(app: Shared) -> Router {
    let secure_origin = app.public_url.starts_with("https://");
    let mut browser = Router::new();
    if app
        .cloud
        .as_ref()
        .is_some_and(|config| config.app_origin.is_some())
    {
        for path in [
            "/session/new",
            "/registration/new",
            "/passwords/new",
            "/password_reset/edit",
            "/email_verification",
            "/email_verification/edit",
            "/workspaces",
            "/workspaces/new",
            "/workspaces/{id}",
            "/workspaces/{id}/billing",
            "/workspaces/{id}/authorize",
            "/app/{id}",
        ] {
            browser = browser.route_service(
                path,
                tower_http::services::ServeFile::new("web/dist/index.html"),
            );
        }
    }
    let api = Router::new()
        .merge(connections::routes())
        .merge(signalwire::routes())
        .merge(managed::routes())
        .merge(sites::private_routes())
        .merge(inquiries::routes())
        .merge(conversions::routes())
        .merge(delivery::routes())
        .merge(google::routes())
        .merge(api_keys::routes())
        .route("/bootstrap", get(bootstrap))
        .route("/leads", get(leads).post(add_lead))
        .route("/leads/{id}", patch(update_lead))
        .route("/leads/{id}/outcomes", get(outcomes).put(put_outcome))
        .route("/numbers", get(numbers).post(add_number))
        .route("/summary", get(summary))
        .route("/session", axum::routing::delete(hosted::logout))
        .route_layer(middleware::from_fn_with_state(app.clone(), authenticate));
    Router::new()
        .merge(browser)
        .merge(hosted::routes(app.clone()))
        .route("/mcp", axum::routing::any(remote_mcp::remote))
        .merge(sites::public_routes(app.clone()))
        .nest("/api/v1", public_api::routes(app.clone()))
        .nest("/api", api)
        .route("/auth/google/callback", get(google::callback))
        .route("/auth/google/result", get(google::result))
        .route("/auth/google/result.js", get(google::result_script))
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/runtime.json", get(|State(app): State<Shared>| async move {
            Json(json!({"hosted_app_origin": app.cloud.as_ref().and_then(|config| config.app_origin.as_deref())}))
        }))
        .route("/webhooks/signalwire/workspaces/{workspace}/{endpoint}", post(signalwire::webhook))
        .route("/webhooks/twilio/voice", post(voice))
        .route("/webhooks/twilio/dial-complete", post(dial_complete))
        .route("/webhooks/twilio/status", post(status))
        .route("/webhooks/twilio/workspaces/{workspace}/{endpoint}", post(connections::webhook))
        .layer(middleware::from_fn(
            |request: Request, next: Next| async move {
                // This pilot publishes query-free callback URLs. Never validate a different URL.
                if request.uri().path().starts_with("/webhooks/") && request.uri().query().is_some()
                {
                    return (
                        StatusCode::BAD_REQUEST,
                        "Webhook URLs must not contain a query",
                    )
                        .into_response();
                }
                next.run(request).await
            },
        ))
        .fallback_service(
            tower_http::services::ServeDir::new("web/dist")
                .not_found_service(tower_http::services::ServeFile::new("web/dist/index.html")),
        )
        .layer(DefaultBodyLimit::max(65536))
        .layer(middleware::from_fn(
            move |request: Request, next: Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert("content-security-policy", "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
                response.headers_mut().insert("x-frame-options", "DENY".parse().unwrap());
                if secure_origin {
                    response.headers_mut().insert("strict-transport-security", "max-age=31536000".parse().unwrap());
                }
                response
                    .headers_mut()
                    .insert("referrer-policy", "no-referrer".parse().unwrap());
                response
                    .headers_mut()
                    .insert("cache-control", "no-store".parse().unwrap());
                response
                    .headers_mut()
                    .insert("x-content-type-options", "nosniff".parse().unwrap());
                response
            },
        ))
        .with_state(app)
}

async fn authenticate(State(app): State<Shared>, mut request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    let access = if supplied.as_bytes().ct_eq(app.api_key.as_bytes()).into() {
        Some(hosted::Access {
            store: app.store.clone(),
            name: "My workspace".into(),
            session_hash: None,
        })
    } else if app.cloud.is_some()
        && supplied
            .strip_prefix("vc_session_")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        let hash = hosted::digest(&supplied);
        match app.store.hosted_session(&hash).await {
            Ok(value) => value.map(|(store, name)| hosted::Access {
                store,
                name,
                session_hash: Some(hash),
            }),
            Err(e) => return ApiError::from(e).into_response(),
        }
    } else {
        None
    };
    if let Some(access) = access {
        request.extensions_mut().insert(access);
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Sign in again or enter the workspace API key"})),
        )
            .into_response()
    }
}

struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<convert_store::Error> for ApiError {
    fn from(e: convert_store::Error) -> Self {
        use convert_store::Error::*;
        match e {
            Validation(e) => Self(StatusCode::BAD_REQUEST, e.to_string()),
            RateLimited => Self(StatusCode::TOO_MANY_REQUESTS, e.to_string()),
            NotFound => Self(StatusCode::NOT_FOUND, "Not found".into()),
            Conflict => Self(StatusCode::CONFLICT, e.to_string()),
            Limit => Self(StatusCode::FORBIDDEN, e.to_string()),
            Database(error) => {
                tracing::error!(%error,"Database request failed");
                Self(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The database request failed".into(),
                )
            }
        }
    }
}
type Api<T> = Result<Json<T>, ApiError>;
async fn bootstrap(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Json<Value> {
    let hosted = access.session_hash.is_some();
    Json(
        json!({"name":access.name,"id":access.store.workspace,"mode":if hosted {"hosted workspace"} else {"single-workspace pilot"},
        "twilio_configured":!hosted && a.twilio_token.is_some(),
        "voice_url":if hosted {String::new()} else {format!("{}/webhooks/twilio/voice",a.public_url)},
        "status_url":if hosted {String::new()} else {format!("{}/webhooks/twilio/status",a.public_url)},"currency":"USD","hosted":hosted}),
    )
}
async fn leads(Extension(access): Extension<hosted::Access>) -> Api<Vec<Lead>> {
    Ok(Json(access.store.leads().await?))
}
async fn numbers(Extension(access): Extension<hosted::Access>) -> Api<Vec<TrackingNumber>> {
    Ok(Json(access.store.numbers().await?))
}
async fn summary(Extension(access): Extension<hosted::Access>) -> Api<Vec<SourceSummary>> {
    Ok(Json(access.store.summary().await?))
}
async fn add_number(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(n): Json<NewNumber>,
) -> Api<Value> {
    connections::register(&a, &access, &n).await?;
    Ok(Json(json!({"ok":true})))
}
async fn add_lead(
    Extension(access): Extension<hosted::Access>,
    Json(n): Json<NewLead>,
) -> Api<Value> {
    Ok(Json(json!({"id":access.store.add_lead(&n).await?})))
}
async fn update_lead(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(n): Json<LeadUpdate>,
) -> Api<Value> {
    access.store.update_lead(id, &n).await?;
    Ok(Json(json!({"ok":true})))
}
async fn outcomes(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Vec<Outcome>> {
    Ok(Json(access.store.outcomes(id).await?))
}
async fn put_outcome(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(n): Json<OutcomeInput>,
) -> Api<Value> {
    access.store.put_outcome(id, &n).await?;
    Ok(Json(json!({"ok":true})))
}
async fn voice(
    State(a): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    webhook(a, "voice", headers, body).await
}
async fn dial_complete(
    State(a): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    webhook(a, "dial-complete", headers, body).await
}
async fn status(
    State(a): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    webhook(a, "status", headers, body).await
}
async fn webhook(
    a: Shared,
    endpoint: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let token = a.twilio_token.as_deref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Twilio is not configured".into(),
        )
    })?;
    process_webhook(
        &a.store,
        &a.public_url,
        "/webhooks/twilio",
        a.twilio_account.as_deref().unwrap(),
        token,
        endpoint,
        headers,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn process_webhook(
    store: &Store,
    public_url: &str,
    prefix: &str,
    account: &str,
    token: &str,
    endpoint: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    process_provider_webhook(
        store,
        public_url,
        prefix,
        account,
        token,
        endpoint,
        headers,
        body,
        PhoneProvider::Twilio,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn process_provider_webhook(
    store: &Store,
    public_url: &str,
    prefix: &str,
    account: &str,
    token: &str,
    endpoint: &str,
    headers: HeaderMap,
    body: Bytes,
    provider: PhoneProvider,
) -> Result<Response, ApiError> {
    let denied = || ApiError(StatusCode::FORBIDDEN, "Invalid webhook".into());
    let malformed = || ApiError(StatusCode::BAD_REQUEST, "Invalid call payload".into());
    if headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim())
        != Some("application/x-www-form-urlencoded")
    {
        return Err(malformed());
    }
    let signature = headers
        .get("x-twilio-signature")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(denied)?;
    let path = format!("{prefix}/{endpoint}");
    let fields = twilio::verify(token, &format!("{public_url}{path}"), &body, signature)
        .map_err(|_| denied())?;
    let field = |name: &str| fields.get(name).map(String::as_str).unwrap_or("");
    if field("AccountSid") != account {
        return Err(denied());
    }
    if !(match provider {
        PhoneProvider::Twilio => twilio::valid_sid(field("CallSid"), "CA"),
        PhoneProvider::Signalwire => convert_adapters::signalwire_api::valid_id(field("CallSid")),
    }) || us_number(field("To")).is_err()
        || field("From").is_empty()
        || field("From").len() > 200
    {
        return Err(malformed());
    }
    let kind = match endpoint {
        "voice" => CallEventKind::Voice,
        "dial-complete" => {
            let status = field("DialCallStatus");
            if !["completed", "busy", "no-answer", "failed", "canceled"].contains(&status) {
                return Err(malformed());
            }
            let duration = if field("DialCallDuration").is_empty() {
                None
            } else {
                Some(
                    field("DialCallDuration")
                        .parse::<i32>()
                        .ok()
                        .filter(|v| *v >= 0)
                        .ok_or_else(malformed)?,
                )
            };
            CallEventKind::Dial {
                status: status.into(),
                duration,
            }
        }
        _ => {
            let status = field("CallStatus");
            if ![
                "queued",
                "initiated",
                "ringing",
                "in-progress",
                "completed",
                "busy",
                "failed",
                "no-answer",
                "canceled",
            ]
            .contains(&status)
            {
                return Err(malformed());
            }
            let sequence = if field("SequenceNumber").is_empty() {
                0
            } else {
                field("SequenceNumber")
                    .parse::<i32>()
                    .ok()
                    .filter(|n| *n >= 0)
                    .ok_or_else(malformed)?
            };
            CallEventKind::Status {
                status: status.into(),
                sequence,
            }
        }
    };
    // Authenticate and validate first. A deliberate 200 Hangup prevents carrier
    // retries/fallback from turning a billing suspension into new billable calls.
    // Completion/status receipts still flow through for accurate final metering.
    if endpoint == "voice"
        && provider == PhoneProvider::Signalwire
        && !store.managed_voice_allowed(field("To")).await?
    {
        return Ok(([("content-type", "application/xml")], twilio::HANGUP).into_response());
    }
    let destination = store
        .call_event_for(
            &CallEvent {
                sid: field("CallSid").into(),
                to: field("To").into(),
                from: field("From").into(),
                fingerprint: twilio::fingerprint(&path, &fields),
                kind,
            },
            provider,
        )
        .await?;
    let xml = if endpoint == "voice" {
        twilio::forward(&destination, &format!("{public_url}{prefix}/dial-complete"))
            .map_err(|_| malformed())?
    } else {
        twilio::HANGUP.into()
    };
    Ok(([("content-type", "application/xml")], xml).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use hmac::{Hmac, Mac};
    use http_body_util::BodyExt;
    use sha1::Sha1;
    use tower::ServiceExt;

    fn app(configured: bool) -> Router {
        // Lazy pool: every test here must finish before any database access.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost/unused")
            .unwrap();
        router(Arc::new(App {
            store: Store {
                pool,
                workspace: Uuid::nil(),
            },
            api_key: "test-workspace-key-at-least-32-characters".into(),
            cloud: None,
            vault: None,
            twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
            signalwire_client: convert_adapters::signalwire_api::Client::new().unwrap(),
            google: None,
            google_client: convert_adapters::google_api::Client::new().unwrap(),
            public_url: "https://convert.example.com".into(),
            twilio_account: configured.then(|| format!("AC{}", "1".repeat(32))),
            twilio_token: configured.then(|| "test-token".into()),
        }))
    }
    #[tokio::test]
    async fn private_routes_require_authentication() {
        for path in [
            "/api/bootstrap",
            "/api/leads",
            "/api/numbers",
            "/api/summary",
        ] {
            let response = app(false)
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.headers()["x-frame-options"], "DENY");
            assert!(
                response.headers()["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .contains("'wasm-unsafe-eval'")
            );
            assert_eq!(
                response.headers()["strict-transport-security"],
                "max-age=31536000"
            );
        }
        let response = app(false)
            .oneshot(
                Request::builder()
                    .uri("/api/bootstrap")
                    .header(
                        "authorization",
                        "Bearer test-workspace-key-at-least-32-characters",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["twilio_configured"], false);
        assert!(!String::from_utf8_lossy(&bytes).contains("test-workspace-key"));
    }
    #[tokio::test]
    async fn callbacks_fail_closed_without_credentials_or_signature() {
        for (configured, status) in [
            (false, StatusCode::SERVICE_UNAVAILABLE),
            (true, StatusCode::FORBIDDEN),
        ] {
            let response = app(configured)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/webhooks/twilio/voice")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(Body::from("To=%2B12025550101"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
    }
    #[tokio::test]
    async fn callbacks_reject_queries_and_oversized_bodies() {
        for (path, body, status) in [
            (
                "/webhooks/twilio/voice?extra=1",
                String::new(),
                StatusCode::BAD_REQUEST,
            ),
            (
                "/webhooks/twilio/voice",
                "x".repeat(65537),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            let response = app(true)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
    }
    #[tokio::test]
    async fn correctly_signed_wrong_account_and_malformed_payload_are_rejected() {
        for (account, status) in [
            (format!("AC{}", "2".repeat(32)), StatusCode::FORBIDDEN),
            (format!("AC{}", "1".repeat(32)), StatusCode::BAD_REQUEST),
        ] {
            let mut mac = Hmac::<Sha1>::new_from_slice(b"test-token").unwrap();
            mac.update(b"https://convert.example.com/webhooks/twilio/voice");
            mac.update(b"AccountSid");
            mac.update(account.as_bytes());
            let signature = STANDARD.encode(mac.finalize().into_bytes());
            let response = app(true)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/webhooks/twilio/voice")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("x-twilio-signature", signature)
                        .body(Body::from(format!("AccountSid={account}")))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
    }
}

#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod hosted_tests;

#[cfg(test)]
mod site_tests;

#[cfg(test)]
mod inquiry_tests;

#[cfg(test)]
mod conversion_tests;

#[cfg(test)]
mod google_tests;
#[cfg(test)]
mod public_api_tests;

#[cfg(test)]
mod signalwire_tests;

#[cfg(test)]
mod managed_tests;

#[cfg(test)]
mod adoption_tests;

#[cfg(test)]
mod delivery_tests;
