//! Signed outbound webhooks for hosted and self-hosted workspaces.
//!
//! Events are recorded by database triggers (migration 0022). This module manages
//! endpoints, signs requests per the Standard Webhooks spec, guards destinations
//! against server-side request forgery, and runs the delivery worker.
use super::*;
use aes_gcm::aead::{OsRng, rand_core::RngCore};
use axum::extract::Query;
use base64::{Engine, engine::general_purpose::STANDARD};
use convert_store::{
    api_keys::KeyAccess,
    public_api::Page,
    webhooks::{Attempt, Claim, EndpointInput},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use url::{Host, Url};

/// Self-hosted operators may allow private addresses and plain HTTP. Never set in hosted mode.
pub static ALLOW_PRIVATE: AtomicBool = AtomicBool::new(false);
const TIMEOUT: Duration = Duration::from_secs(10);
const CONCURRENCY: usize = 8;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/{id}", get(show).patch(update).delete(remove))
        .route("/webhooks/{id}/rotate-secret", post(rotate))
        .route("/webhooks/{id}/test", post(test))
        .route("/webhooks/{id}/deliveries", get(deliveries))
        .route("/webhook-deliveries/{id}/redeliver", post(redeliver))
}

// ---- Signing ----

/// A new secret in the Standard Webhooks `whsec_` format.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("whsec_{}", STANDARD.encode(bytes))
}
/// `v1,<base64 HMAC-SHA256("{id}.{timestamp}.{body}")>` keyed by the decoded secret.
pub fn sign(secret: &str, id: &str, timestamp: i64, body: &str) -> Option<String> {
    let key = STANDARD
        .decode(secret.strip_prefix("whsec_").unwrap_or(secret))
        .ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(format!("{id}.{timestamp}.{body}").as_bytes());
    Some(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

// ---- Destination safety ----

/// Syntax checks applied when an endpoint is saved and again before each attempt.
pub fn check_url(raw: &str, allow_private: bool) -> Result<Url, &'static str> {
    let url = Url::parse(raw.trim()).map_err(|_| "Enter a valid URL")?;
    match url.scheme() {
        "https" => {}
        "http" if allow_private => {}
        _ => return Err("Use an https:// URL"),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Remove credentials from the URL; requests are signed instead");
    }
    if url.fragment().is_some() {
        return Err("Remove the #fragment from the URL");
    }
    match url.host().ok_or("The URL needs a host")? {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if !allow_private
                && (domain == "localhost"
                    || domain.ends_with(".localhost")
                    || domain.ends_with(".internal")
                    || domain.ends_with(".local")
                    || !domain.contains('.'))
            {
                return Err("Use a public hostname");
            }
        }
        Host::Ipv4(_) | Host::Ipv6(_) if !allow_private => {
            return Err("Use a hostname, not an IP address");
        }
        _ => {}
    }
    if !allow_private && url.port().is_some_and(|p| p != 443) {
        return Err("Use the default HTTPS port 443");
    }
    Ok(url)
}
/// True for addresses on the public internet.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 (64:ff9b::/96) embeds an IPv4 address.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                return public_v4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                ));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || s[0..6] == [0, 0, 0, 0, 0, 0])
        }
    }
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (18..20).contains(&b))
        || a >= 240)
}
/// Resolve once and return the checked address, so the request cannot be rebound.
async fn destination(url: &Url, allow_private: bool) -> Result<SocketAddr, Attempt> {
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<SocketAddr> = match url.host() {
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Some(Host::Domain(domain)) => tokio::net::lookup_host((domain, port))
            .await
            .map_err(|_| Attempt::Retry {
                status: None,
                error: "The hostname could not be resolved".into(),
                after: None,
            })?
            .collect(),
        None => vec![],
    };
    if addresses.is_empty() {
        return Err(Attempt::Retry {
            status: None,
            error: "The hostname has no addresses".into(),
            after: None,
        });
    }
    if !allow_private && addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err(Attempt::Failed {
            status: None,
            error: "The hostname resolves to a private or reserved address".into(),
        });
    }
    Ok(addresses[0])
}

// ---- Management ----

async fn owner(app: &App, access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash {
        if !access.store.manages_connection(hash).await? {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "Only the workspace owner can manage webhooks".into(),
            ));
        }
        let (user, version) = access.store.api_identity(hash).await?;
        public_api::hosted_access(app, access.store.workspace, user, version).await?;
    }
    Ok(())
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
fn vault(app: &App) -> Result<&secrets::Vault, ApiError> {
    app.vault.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Webhooks need VISICONVERT_ENCRYPTION_KEY to store signing secrets".into(),
    ))
}
fn checked(input: &mut EndpointInput) -> Result<(), ApiError> {
    let url = check_url(&input.url, ALLOW_PRIVATE.load(Ordering::Relaxed))
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.into()))?;
    input.url = url.to_string();
    Ok(())
}
fn sealed(app: &App, workspace: Uuid, id: Uuid, secret: &str) -> Result<String, ApiError> {
    vault(app)?
        .seal_webhook(workspace, id, secret)
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not store the secret".into(),
            )
        })
}
async fn list(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    owner(&app, &access).await?;
    Ok(Json(access.store.webhook_endpoints().await?))
}
async fn show(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    owner(&app, &access).await?;
    Ok(Json(access.store.webhook_endpoint(id).await?))
}
async fn create(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(mut input): Json<EndpointInput>,
) -> Api<Value> {
    owner(&app, &access).await?;
    checked(&mut input)?;
    let id = Uuid::new_v4();
    let secret = new_secret();
    let sealed = sealed(&app, access.store.workspace, id, &secret)?;
    let actor = actor(&access, &key).await?;
    let mut endpoint = access
        .store
        .create_webhook_endpoint(id, &input, &sealed, &actor)
        .await?;
    endpoint["secret"] = json!(secret);
    Ok(Json(endpoint))
}
async fn update(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(mut input): Json<EndpointInput>,
) -> Api<Value> {
    owner(&app, &access).await?;
    checked(&mut input)?;
    let actor = actor(&access, &key).await?;
    Ok(Json(
        access
            .store
            .update_webhook_endpoint(id, &input, &actor)
            .await?,
    ))
}
async fn remove(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    owner(&app, &access).await?;
    let actor = actor(&access, &key).await?;
    access.store.delete_webhook_endpoint(id, &actor).await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: i32,
}
async fn rotate(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(input): Json<Version>,
) -> Api<Value> {
    owner(&app, &access).await?;
    let secret = new_secret();
    let sealed = sealed(&app, access.store.workspace, id, &secret)?;
    let actor = actor(&access, &key).await?;
    let mut endpoint = access
        .store
        .rotate_webhook_secret(id, input.version, &sealed, &actor)
        .await?;
    endpoint["secret"] = json!(secret);
    Ok(Json(endpoint))
}
async fn test(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    owner(&app, &access).await?;
    vault(&app)?;
    let delivery = access.store.queue_webhook_test(id).await?;
    Ok(Json(json!({"delivery_id":delivery})))
}
async fn deliveries(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> Api<Value> {
    owner(&app, &access).await?;
    Ok(Json(access.store.webhook_deliveries(id, &page).await?))
}
async fn redeliver(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    owner(&app, &access).await?;
    let actor = actor(&access, &key).await?;
    access.store.redeliver_webhook(id, &actor).await?;
    Ok(Json(json!({"ok":true})))
}

// ---- Worker ----

pub fn spawn(a: Shared) {
    tokio::spawn(async move {
        let mut listener = a.store.webhook_listener().await.ok();
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        let mut pruned = tokio::time::Instant::now() - Duration::from_secs(3600);
        loop {
            while let Ok(true) = run_batch(&a).await {}
            if pruned.elapsed() >= Duration::from_secs(3600) {
                if let Err(error) = a.store.prune_webhooks().await {
                    tracing::warn!(%error, "Webhook pruning failed");
                }
                pruned = tokio::time::Instant::now();
            }
            match listener.as_mut() {
                Some(l) => {
                    tokio::select! {
                        _ = tick.tick() => {}
                        r = l.recv() => if r.is_err() {
                            listener = a.store.webhook_listener().await.ok();
                        }
                    }
                }
                None => {
                    tick.tick().await;
                    listener = a.store.webhook_listener().await.ok();
                }
            }
        }
    });
}
/// Send up to CONCURRENCY due deliveries. Returns true if anything was sent, so the
/// caller loops until per-endpoint limits and due times leave nothing to claim.
pub(crate) async fn run_batch(a: &App) -> Result<bool, convert_store::Error> {
    let mut claims = Vec::new();
    while claims.len() < CONCURRENCY {
        match a.store.claim_webhook().await? {
            Some(claim) => claims.push(claim),
            None => break,
        }
    }
    let sent = !claims.is_empty();
    let mut set = tokio::task::JoinSet::new();
    for claim in claims {
        let store = a.store.for_workspace(claim.workspace);
        let vault = a.vault.clone();
        set.spawn(async move {
            let attempt = send(&store, vault.as_ref(), &claim).await;
            match store.finish_webhook(&claim, attempt).await {
                Ok(true) => {
                    tracing::info!(endpoint = %claim.endpoint_id, "Webhook endpoint disabled")
                }
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "Recording a webhook attempt failed"),
            }
        });
    }
    while set.join_next().await.is_some() {}
    Ok(sent)
}
async fn send(store: &Store, vault: Option<&secrets::Vault>, claim: &Claim) -> Attempt {
    let retry = |error: &str| Attempt::Retry {
        status: None,
        error: error.into(),
        after: None,
    };
    let allow_private = ALLOW_PRIVATE.load(Ordering::Relaxed);
    let url = match check_url(&claim.url, allow_private) {
        Ok(url) => url,
        Err(e) => {
            return Attempt::Failed {
                status: None,
                error: e.into(),
            };
        }
    };
    let Some(vault) = vault else {
        return retry("The encryption key is not configured");
    };
    let open = |sealed: &str| {
        vault
            .open_webhook(claim.workspace, claim.endpoint_id, sealed)
            .ok()
    };
    let Some(secret) = open(&claim.secret) else {
        return Attempt::Failed {
            status: None,
            error: "The signing secret could not be decrypted".into(),
        };
    };
    let payload = match store.webhook_payload(claim).await {
        Ok(p) => p,
        Err(_) => return retry("The event could not be prepared"),
    };
    // Demo leads can be flagged just after insertion, so check again before sending.
    if payload["data"]["is_demo"] == true {
        return Attempt::Skipped("Demo data is not sent".into());
    }
    let body = payload.to_string();
    let address = match destination(&url, allow_private).await {
        Ok(a) => a,
        Err(attempt) => return attempt,
    };
    let id = claim.event_id.to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let mut signatures: Vec<String> = [
        Some(secret),
        claim.previous_secret.as_deref().and_then(open),
    ]
    .into_iter()
    .flatten()
    .filter_map(|s| sign(&s, &id, timestamp, &body))
    .collect();
    signatures.dedup();
    let mut builder = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent("VisiConvert-Webhooks/1");
    if let Some(Host::Domain(domain)) = url.host() {
        builder = builder.resolve(domain, address);
    }
    let Ok(client) = builder.build() else {
        return retry("The HTTP client could not start");
    };
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("webhook-id", &id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signatures.join(" "))
        .body(body)
        .send()
        .await;
    let response = match response {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return retry("The endpoint did not respond within 10 seconds"),
        Err(_) => return retry("The connection to the endpoint failed"),
    };
    let status = response.status().as_u16();
    match status {
        200..=299 => Attempt::Succeeded(status),
        410 => Attempt::Gone,
        408 | 429 | 500..=599 => Attempt::Retry {
            status: Some(status),
            error: format!("The endpoint returned {status}"),
            after: response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok()),
        },
        _ => Attempt::Failed {
            status: Some(status),
            error: format!("The endpoint returned {status}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_standard_webhooks_reference() {
        // Test vector from the Standard Webhooks specification.
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = r#"{"test": 2432232314}"#;
        assert_eq!(
            sign(secret, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330, body).unwrap(),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
        let fresh = new_secret();
        assert!(fresh.starts_with("whsec_"));
        assert_eq!(STANDARD.decode(&fresh[6..]).unwrap().len(), 32);
        assert_ne!(fresh, new_secret());
    }

    #[test]
    fn urls_are_restricted_unless_private_destinations_are_allowed() {
        assert!(check_url("https://hooks.example.com/visiconvert", false).is_ok());
        for bad in [
            "http://hooks.example.com/",
            "ftp://hooks.example.com/",
            "https://user:pass@hooks.example.com/",
            "https://hooks.example.com:8443/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://localhost/",
            "https://crm.localhost/",
            "https://metadata.google.internal/",
            "https://intranet/",
            "https://hooks.example.com/#x",
            "not a url",
        ] {
            assert!(check_url(bad, false).is_err(), "{bad}");
        }
        assert!(check_url("http://127.0.0.1:8080/hook", true).is_ok());
        assert!(check_url("http://n8n:5678/webhook/x", true).is_ok());
        assert!(check_url("ftp://n8n/", true).is_err());
    }

    #[test]
    fn private_and_reserved_addresses_are_rejected() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "192.0.0.8",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
}
