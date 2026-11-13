use super::*;
use convert_store::sites::{Site, SiteInput};
use serde::Deserialize;
use std::collections::BTreeMap;

pub fn private_routes() -> Router<Shared> {
    Router::new()
        .route("/setup", get(setup))
        .route("/sites", get(list).post(save_new))
        .route("/sites/{id}", patch(save_existing))
        .route("/sites/{id}/number-pool", get(pool).put(save_pool))
        .route("/leads/{id}/dni-evidence", get(dni_evidence))
        .route("/leads/{id}/website-evidence", get(evidence))
}
pub fn public_routes(app: Shared) -> Router<Shared> {
    let collect = Router::new()
        .route("/collect/sites/{id}/config", get(config).options(preflight))
        .route(
            "/collect/sites/{id}/sessions",
            post(session).options(preflight),
        )
        .route("/collect/sites/{id}/forms", post(form).options(preflight))
        .route(
            "/collect/sites/{id}/numbers",
            post(numbers).options(preflight),
        )
        .route_layer(middleware::from_fn_with_state(app, cors))
        .layer(DefaultBodyLimit::max(8192));
    Router::new().merge(collect).route(
        "/tracker.js",
        get(|| async {
            (
                [("content-type", "application/javascript; charset=utf-8")],
                include_str!("../assets/tracker.js"),
            )
        }),
    )
}
async fn owner(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash
        && !access.store.manages_connection(hash).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can manage sites".into(),
        ));
    }
    Ok(())
}
pub(super) async fn list(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    owner(&access).await?;
    Ok(Json(
        json!({"sites":access.store.sites().await?,"tracker_url":format!("{}/tracker.js",a.public_url)}),
    ))
}
pub(super) async fn setup(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
) -> Api<Value> {
    owner(&access).await?;
    let mut value = access.store.setup_status().await?;
    value["workspace_id"] = json!(access.store.workspace);
    value["tracker_url"] = json!(format!("{}/tracker.js", a.public_url));
    Ok(Json(value))
}
fn invalid(message: &'static str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}
fn canonical_origin(input: &str) -> Result<String, ApiError> {
    let url = url::Url::parse(input)
        .map_err(|_| invalid("Enter a website origin such as https://example.com"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || url.host_str().is_some_and(|h| h.contains('*'))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Use an HTTPS origin without a path, credentials, query, or fragment; HTTP is allowed only on localhost",
        ));
    }
    Ok(url.origin().ascii_serialization())
}
fn validate_site(input: &mut SiteInput) -> Result<(), ApiError> {
    input.name = input.name.trim().to_string();
    if input.name.is_empty() || input.name.len() > 100 || input.version < 0 {
        return Err(invalid(
            "Enter a site name up to 100 bytes and a valid version",
        ));
    }
    input.origin = canonical_origin(input.origin.trim())?;
    input.allowed_fields.sort();
    input.allowed_fields.dedup();
    if input
        .allowed_fields
        .iter()
        .any(|s| !["name", "email", "phone", "company"].contains(&s.as_str()))
    {
        return Err(invalid(
            "Allowed fields are name, email, phone, and company",
        ));
    }
    Ok(())
}
pub(super) async fn save_new(
    Extension(access): Extension<hosted::Access>,
    Json(mut input): Json<SiteInput>,
) -> Api<Value> {
    owner(&access).await?;
    validate_site(&mut input)?;
    Ok(Json(
        json!({"id":access.store.save_site(None, &input).await?}),
    ))
}
pub(super) async fn save_existing(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(mut input): Json<SiteInput>,
) -> Api<Value> {
    owner(&access).await?;
    validate_site(&mut input)?;
    Ok(Json(
        json!({"id":access.store.save_site(Some(id), &input).await?}),
    ))
}
async fn evidence(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Option<Value>> {
    Ok(Json(access.store.website_evidence(id).await?))
}
// Dynamic CORS is intentionally collection-only. Origin checks reduce browser abuse;
// public collection is not an authenticated statement that a submission is genuine.
async fn cors(State(a): State<Shared>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let site = request
        .uri()
        .path()
        .split('/')
        .nth(3)
        .and_then(|s| s.parse::<Uuid>().ok());
    let Some(site) = site else {
        return invalid("Invalid site identifier").into_response();
    };
    if canonical_origin(&origin).ok().as_deref() != Some(&origin) {
        return ApiError(
            StatusCode::FORBIDDEN,
            "Website origin is not allowed".into(),
        )
        .into_response();
    }
    if let Err(error) = a.store.public_site(site, &origin).await {
        return ApiError::from(error).into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("access-control-allow-origin", origin.parse().unwrap());
    response
        .headers_mut()
        .insert("vary", "Origin".parse().unwrap());
    response.headers_mut().insert(
        "access-control-allow-methods",
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    response.headers_mut().insert(
        "access-control-allow-headers",
        "Content-Type".parse().unwrap(),
    );
    response
}
async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}
fn origin(headers: &HeaderMap) -> &str {
    headers
        .get("origin")
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
}
async fn config(State(a): State<Shared>, Path(id): Path<Uuid>, headers: HeaderMap) -> Api<Value> {
    let site: Site = a.store.public_site(id, origin(&headers)).await?;
    Ok(Json(
        json!({"allowed_fields":site.allowed_fields,"session_minutes":30}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionInput {
    id: Uuid,
    consent: String,
    landing_url: String,
    referrer: String,
    /// Meta Pixel's first-party `_fbp` and `_fbc` cookies, when present.
    #[serde(default)]
    fbp: Option<String>,
    #[serde(default)]
    fbc: Option<String>,
}
/// The visitor's user agent, and their IP address from the header the deployment's proxy
/// sets (VISICONVERT_CLIENT_IP_HEADER, such as x-forwarded-for; the last entry is the one
/// the proxy added). Without that setting no IP address is kept.
fn browser(headers: &HeaderMap) -> convert_store::sites::Browser {
    static HEADER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let header = HEADER.get_or_init(|| {
        std::env::var("VISICONVERT_CLIENT_IP_HEADER")
            .ok()
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
    });
    convert_store::sites::Browser {
        user_agent: headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty() && !v.chars().any(char::is_control))
            .map(|v| v.chars().take(1000).collect()),
        ip: header.as_deref().and_then(|h| client_ip(headers, h)),
    }
}
fn client_ip(headers: &HeaderMap, header: &str) -> Option<std::net::IpAddr> {
    headers
        .get_all(header)
        .iter()
        .next_back()?
        .to_str()
        .ok()?
        .rsplit(',')
        .next()?
        .trim()
        .parse()
        .ok()
}
fn session_evidence(input: &SessionInput, origin: &str) -> Result<Value, ApiError> {
    if input.consent != "granted" {
        return Err(invalid("Tracking consent is required"));
    }
    if input.landing_url.len() > 3000 || input.referrer.len() > 1000 {
        return Err(invalid("Website context is too long"));
    }
    let landing =
        url::Url::parse(&input.landing_url).map_err(|_| invalid("Invalid landing URL"))?;
    if landing.origin().ascii_serialization() != origin
        || !landing.username().is_empty()
        || landing.password().is_some()
    {
        return Err(invalid(
            "Landing URL must belong to the configured website origin",
        ));
    }
    landing_evidence(
        landing,
        &input.referrer,
        origin,
        input.fbp.as_deref(),
        input.fbc.as_deref(),
    )
}
/// Visit evidence from a landing URL, as the tracker records it: campaign parameters and
/// click IDs from the query, the referrer's origin, and the classified source. The caller
/// has already checked that the URL belongs to `origin`'s site.
pub(crate) fn landing_evidence(
    mut landing: url::Url,
    referrer: &str,
    origin: &str,
    fbp: Option<&str>,
    fbc: Option<&str>,
) -> Result<Value, ApiError> {
    let mut value =
        json!({"model":"session-entry-v1","consent":"granted","confidence":"browser-reported"});
    for key in [
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "gclid",
        "gbraid",
        "wbraid",
        "msclkid",
        "fbclid",
    ] {
        let supplied: Vec<_> = landing.query_pairs().filter(|(k, _)| k == key).collect();
        if let Some((_, v)) = supplied.first() {
            if supplied.iter().any(|(_, other)| other != v) {
                return Err(invalid(
                    "Conflicting repeated campaign parameters are not accepted",
                ));
            }
            if v.len() > 200 || v.chars().any(char::is_control) {
                return Err(invalid(
                    "Campaign values must be at most 200 bytes without control characters",
                ));
            }
            value[key] = json!(v);
        }
    }
    // Malformed Pixel cookies are dropped rather than failing the visit.
    for (key, cookie) in [("fbp", fbp), ("fbc", fbc)] {
        if let Some(v) = cookie.filter(|v| convert_domain::meta::cookie_value(v)) {
            value[key] = json!(v);
        }
    }
    landing.set_query(None);
    landing.set_fragment(None);
    value["landing_url"] = json!(landing.as_str());
    let referrer = url::Url::parse(referrer)
        .ok()
        .filter(|u| {
            matches!(u.scheme(), "https" | "http")
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
        })
        .map(|u| u.origin().ascii_serialization());
    value["referrer_origin"] = json!(referrer);
    let classification =
        convert_domain::attribution::classify_source(&convert_domain::attribution::SourceContext {
            utm_source: value["utm_source"].as_str(),
            utm_medium: value["utm_medium"].as_str(),
            gclid: value["gclid"].as_str(),
            gbraid: value["gbraid"].as_str(),
            wbraid: value["wbraid"].as_str(),
            msclkid: value["msclkid"].as_str(),
            fbclid: value["fbclid"].as_str(),
            referrer: referrer.as_deref(),
            site_origin: origin,
        });
    if let Value::Object(fields) = json!(classification) {
        value
            .as_object_mut()
            .expect("session evidence is an object")
            .extend(fields);
    }
    Ok(value)
}
async fn session(
    State(a): State<Shared>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<SessionInput>,
) -> Api<Value> {
    let evidence = session_evidence(&input, origin(&headers))?;
    a.store
        .capture_visit(
            id,
            origin(&headers),
            input.id,
            &evidence,
            &browser(&headers),
        )
        .await?;
    Ok(Json(json!({"accepted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormInput {
    session_id: Uuid,
    submission_id: String,
    consent: String,
    fields: BTreeMap<String, String>,
}
async fn form(
    State(a): State<Shared>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<FormInput>,
) -> Api<Value> {
    if input.consent != "granted" {
        return Err(invalid("Tracking consent is required"));
    }
    if input.submission_id.is_empty()
        || input.submission_id.len() > 80
        || !input
            .submission_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
    {
        return Err(invalid(
            "Use a stable submission ID up to 80 letters, digits, hyphens, underscores, dots, or colons",
        ));
    }
    if input.fields.len() > 4
        || input
            .fields
            .values()
            .any(|v| v.len() > 200 || v.chars().any(char::is_control))
    {
        return Err(invalid(
            "Form values must be at most 200 bytes without control characters",
        ));
    }
    a.store
        .capture_form(
            id,
            origin(&headers),
            input.session_id,
            &input.submission_id,
            &input.fields,
        )
        .await?;
    Ok(Json(json!({"accepted":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_strips_private_query_and_referrer_path() {
        let input = SessionInput { id: Uuid::nil(), consent:"granted".into(), landing_url:"https://example.com/quote?email=private%40example.com&utm_source=google&gclid=abc#secret".into(), referrer:"https://search.example/private?secret=yes".into(), fbp:None, fbc:None };
        let value = session_evidence(&input, "https://example.com")
            .ok()
            .unwrap();
        assert_eq!(value["source"], "google");
        assert_eq!(value["gclid"], "abc");
        assert_eq!(value["landing_url"], "https://example.com/quote");
        assert_eq!(value["referrer_origin"], "https://search.example");
        assert!(!value.to_string().contains("private"));
        assert!(session_evidence(&input, "https://other.example").is_err());
    }
    #[test]
    fn classification_keeps_raw_inputs_and_rejects_conflicting_duplicates() {
        let mut input = SessionInput {
            id: Uuid::nil(),
            consent: "granted".into(),
            landing_url:
                "https://example.com/?utm_source=Newsletter&utm_medium=email&gclid=opaque-click"
                    .into(),
            referrer: String::new(),
            fbp: None,
            fbc: None,
        };
        let v = session_evidence(&input, "https://example.com")
            .ok()
            .unwrap();
        assert_eq!(v["source"], "Newsletter");
        assert_eq!(v["utm_source"], "Newsletter");
        assert_eq!(v["gclid"], "opaque-click");
        assert_eq!(v["source_basis"], "utm");
        assert!(v["source_warning"].as_str().unwrap().contains("differs"));
        input.landing_url = "https://example.com/?gclid=a&gclid=b".into();
        assert!(session_evidence(&input, "https://example.com").is_err());
        input.landing_url = "https://example.com/?gclid=a&gclid=a".into();
        let v = session_evidence(&input, "https://example.com")
            .ok()
            .unwrap();
        assert_eq!(v["source"], "google_ads");
        assert_eq!(v["source_basis"], "click_id");
        input.referrer = "https://user:private@google.com/private".into();
        assert!(
            session_evidence(&input, "https://example.com")
                .ok()
                .unwrap()["referrer_origin"]
                .is_null()
        );
    }

    #[test]
    fn meta_cookies_and_client_ip() {
        let input = SessionInput {
            id: Uuid::nil(),
            consent: "granted".into(),
            landing_url: "https://example.com/?fbclid=IwAR1".into(),
            referrer: String::new(),
            fbp: Some("fb.1.1596403881668.1116446470".into()),
            fbc: Some("not a cookie".into()),
        };
        let v = session_evidence(&input, "https://example.com")
            .ok()
            .unwrap();
        assert_eq!(v["fbclid"], "IwAR1");
        assert_eq!(v["fbp"], "fb.1.1596403881668.1116446470");
        assert!(v.get("fbc").is_none());
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1, 203.0.113.9".parse().unwrap());
        assert_eq!(
            client_ip(&headers, "x-forwarded-for"),
            Some("203.0.113.9".parse().unwrap())
        );
        headers.insert("x-forwarded-for", "spoofed".parse().unwrap());
        assert_eq!(client_ip(&headers, "x-forwarded-for"), None);
        assert_eq!(client_ip(&headers, "x-real-ip"), None);
    }

    #[test]
    fn origins_require_exact_secure_websites() {
        for bad in [
            "null",
            "https://a.example/path",
            "https://user:pass@a.example",
            "http://a.example",
            "https://a.example?x=1",
            "https://*.example.com",
        ] {
            assert!(canonical_origin(bad).is_err(), "{bad}");
        }
        assert_eq!(
            canonical_origin("https://Example.com/").ok().unwrap(),
            "https://example.com"
        );
        assert!(canonical_origin("http://localhost:4173").is_ok());
    }
}

pub(super) async fn pool(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    owner(&access).await?;
    Ok(Json(access.store.dni_pool(id).await?))
}
pub(super) async fn save_pool(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<convert_store::dni::PoolInput>,
) -> Api<Value> {
    owner(&access).await?;
    let pool_id = access.store.save_dni_pool(id, &input).await?;
    Ok(Json(json!({"saved":true,"pool_id":pool_id})))
}
async fn dni_evidence(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Option<Value>> {
    Ok(Json(access.store.dni_evidence(id).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NumberInput {
    session_id: Uuid,
    consent: String,
    /// Published numbers marked on the page. Omitted by single-pool trackers.
    #[serde(default)]
    targets: Option<Vec<String>>,
}
async fn numbers(
    State(a): State<Shared>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<NumberInput>,
) -> Api<Value> {
    if input.consent != "granted" {
        return Err(invalid("Tracking consent is required"));
    }
    Ok(Json(
        a.store
            .allocate_dni(
                id,
                origin(&headers),
                input.session_id,
                input.targets.as_deref(),
            )
            .await?,
    ))
}
