//! POST /api/v1/chat-leads: a web chat system reports each chat with the ad click from
//! its landing page, and again when the visitor's phone or email becomes known.
use super::*;
use chrono::{DateTime, Duration as Span, Utc};
use convert_store::chat::ChatLead;
use serde::Deserialize;
use std::collections::BTreeMap;

const CLICKS: [&str; 5] = ["gclid", "gbraid", "wbraid", "msclkid", "fbclid"];
const UTMS: [&str; 5] = ["source", "medium", "campaign", "term", "content"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChatInput {
    external_id: String,
    started_at: DateTime<Utc>,
    landing_url: String,
    #[serde(default)]
    referrer: Option<String>,
    #[serde(default)]
    click: BTreeMap<String, String>,
    #[serde(default)]
    utm: BTreeMap<String, String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}
fn clean(v: &str) -> bool {
    !v.is_empty() && v.len() <= 200 && !v.chars().any(char::is_control)
}
/// The last 10 digits of a phone number with 10 to 15 digits.
pub(crate) fn phone(raw: &str) -> Result<String, ApiError> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if raw.chars().count() > 40 || !(10..=15).contains(&digits.len()) {
        return Err(bad("phone needs 10 to 15 digits"));
    }
    Ok(digits[digits.len() - 10..].into())
}
pub(crate) fn email(raw: &str) -> Result<String, ApiError> {
    let e = raw.trim().to_lowercase();
    let valid = e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
        && e.split_once('@').is_some_and(|(user, domain)| {
            !user.is_empty() && domain.contains('.') && !domain.contains('@')
        });
    if valid {
        Ok(e)
    } else {
        Err(bad("email must be an email address"))
    }
}

pub(crate) async fn create(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<ChatInput>,
) -> Api<Value> {
    let id = &input.external_id;
    if id.is_empty()
        || id.len() > 150
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
    {
        return Err(bad(
            "external_id must be 1 to 150 letters, digits, hyphens, underscores, dots or colons",
        ));
    }
    let now = Utc::now();
    if input.started_at > now + Span::minutes(5) {
        return Err(bad("started_at is in the future"));
    }
    if input.started_at < now - Span::days(90) {
        return Err(bad(
            "started_at is more than 90 days ago; ad platforms no longer accept it",
        ));
    }
    let started_at = input.started_at.min(now);
    if input.landing_url.len() > 3000 || input.referrer.as_ref().is_some_and(|r| r.len() > 1000) {
        return Err(bad("landing_url or referrer is too long"));
    }
    let mut landing =
        url::Url::parse(&input.landing_url).map_err(|_| bad("landing_url is not a URL"))?;
    if !matches!(landing.scheme(), "https" | "http")
        || landing.host_str().is_none()
        || !landing.username().is_empty()
        || landing.password().is_some()
    {
        return Err(bad("landing_url must be an http(s) page URL"));
    }
    let Some((site, origin)) = access
        .store
        .chat_site(&landing.origin().ascii_serialization())
        .await?
    else {
        return Err(bad(
            "landing_url is not on one of this workspace's enabled sites",
        ));
    };
    // Supplied click IDs and UTMs fill in what the landing URL lacks; a different value for
    // one the URL already carries is refused rather than guessed between.
    let mut extra: Vec<(String, String)> = Vec::new();
    for (map, allowed, prefix) in [(&input.click, &CLICKS, ""), (&input.utm, &UTMS, "utm_")] {
        for (k, v) in map {
            if !allowed.contains(&k.as_str()) {
                return Err(bad(format!("Unknown click or utm key: {k}")));
            }
            if !clean(v) {
                return Err(bad(
                    "Click and utm values must be 1 to 200 bytes without control characters",
                ));
            }
            let key = format!("{prefix}{k}");
            match landing.query_pairs().find(|(q, _)| *q == key) {
                Some((_, existing)) if existing != *v => {
                    return Err(bad(format!("{key} differs from the value in landing_url")));
                }
                Some(_) => {}
                None => extra.push((key, v.clone())),
            }
        }
    }
    if !extra.is_empty() {
        let mut q = landing.query_pairs_mut();
        for (k, v) in &extra {
            q.append_pair(k, v);
        }
    }
    let evidence = sites::landing_evidence(
        landing,
        input.referrer.as_deref().unwrap_or(""),
        &origin,
        None,
        None,
    )?;
    let name = match input.name.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(n) if clean(n) => Some(n.to_string()),
        Some(_) => {
            return Err(bad(
                "name must be at most 200 bytes without control characters",
            ));
        }
    };
    let phone = match input.phone.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(p) => Some(phone(p)?),
    };
    let email = match input.email.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(e) => Some(email(e)?),
    };
    let result = access
        .store
        .capture_chat(
            site,
            &evidence,
            &ChatLead {
                external_id: input.external_id,
                started_at,
                name,
                phone,
                email,
            },
        )
        .await?;
    Ok(Json(result))
}
