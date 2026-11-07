//! Meta Conversions API: read a dataset (Pixel) and send server events to it, with the
//! normalization and hashing Meta requires for customer information. Requests are built
//! from whitelisted JSON; access tokens, upstream messages and customer data are never
//! returned to callers or logged.
//!
//! - <https://developers.facebook.com/docs/marketing-api/conversions-api/parameters/server-event>
//! - <https://developers.facebook.com/docs/marketing-api/conversions-api/parameters/customer-information-parameters>
//! - <https://developers.facebook.com/docs/marketing-api/conversions-api/get-started>
pub use convert_domain::meta::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// Graph API version, overridable with VISICONVERT_META_GRAPH_VERSION.
pub const GRAPH_VERSION: &str = "v24.0";
/// Meta rejects a whole request when any event is older than 7 days.
pub const MAX_EVENT_AGE_SECONDS: i64 = 7 * 24 * 3600;
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error(
        "Meta rejected the access token. Generate a new Conversions API token for this dataset in Events Manager and save it again."
    )]
    Token,
    #[error(
        "This token cannot use that dataset. Check the dataset (Pixel) ID, and generate the token from that dataset's settings in Events Manager."
    )]
    Access,
    #[error("Meta is unavailable or rate limited. Try again shortly.")]
    Unavailable,
    #[error("Meta returned an unexpected response. Try again shortly.")]
    Response,
}

/// A Conversions API dataset (Pixel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dataset {
    pub id: String,
    pub name: String,
}
/// What Meta reported for an accepted request.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub events_received: i64,
    pub trace_id: String,
}
/// Sanitized delivery outcomes: only Meta's numeric error code and subcode are kept.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryError {
    /// Not applied and safe to try again later (throttled, transient, connect failure).
    Retry,
    /// The request may have been applied. Never replayed automatically.
    Uncertain,
    /// The token was rejected or cannot use the dataset.
    Authorization,
    /// Explicitly rejected: (code, subcode).
    Rejected(i64, i64),
}
impl DeliveryError {
    pub fn message(&self) -> String {
        match self {
            Self::Retry => "Meta is rate limited or unavailable; retry scheduled.".into(),
            Self::Uncertain => "Meta may have received this event. It is not sent again automatically; check Events Manager for the dataset.".into(),
            Self::Authorization => "Meta rejected the access token or its access to the dataset. Save a new Conversions API token in Conversions.".into(),
            Self::Rejected(code, sub) => format!("Meta rejected this event (code {code}, subcode {sub}). Check the event name, time and customer information."),
        }
    }
}

/// Dataset, account and similar Meta IDs are numeric.
pub fn numeric_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}
/// Access tokens are opaque printable strings.
pub fn token_shape(s: &str) -> bool {
    (20..=1024).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic())
}
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    graph: String,
    version: String,
}
impl Client {
    pub fn new() -> Result<Self, Error> {
        let version = std::env::var("VISICONVERT_META_GRAPH_VERSION")
            .ok()
            .filter(|v| {
                v.len() <= 8
                    && v.starts_with('v')
                    && v[1..].bytes().all(|b| b.is_ascii_digit() || b == b'.')
            })
            .unwrap_or_else(|| GRAPH_VERSION.into());
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .user_agent("VisiConvert/0.1")
                .build()
                .map_err(|_| Error::Unavailable)?,
            graph: "https://graph.facebook.com".into(),
            version,
        })
    }
    pub fn for_test(origin: String) -> Self {
        let mut c = Self::new().unwrap();
        c.graph = origin;
        c
    }
    async fn body(response: &mut reqwest::Response) -> Option<Value> {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).ok()
    }
    /// Read a dataset the token can use. The token travels in the Authorization header,
    /// never in a URL.
    pub async fn dataset(&self, token: &str, id: &str) -> Result<Dataset, Error> {
        if !numeric_id(id) || !token_shape(token) {
            return Err(Error::Access);
        }
        let mut response = self
            .http
            .get(format!("{}/{}/{id}", self.graph, self.version))
            .query(&[("fields", "id,name")])
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let body = Self::body(&mut response).await.ok_or(Error::Response)?;
        if let Some(e) = body.get("error") {
            return Err(setup_error(e));
        }
        if status != 200 {
            return Err(Error::Response);
        }
        let got = body["id"].as_str().unwrap_or_default();
        if got != id {
            return Err(Error::Response);
        }
        Ok(Dataset {
            id: got.into(),
            name: body["name"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect(),
        })
    }
    /// Send server events. `test_event_code` routes them to Events Manager's Test events
    /// tab, where they never count as conversions.
    pub async fn send(
        &self,
        token: &str,
        dataset: &str,
        events: &[Value],
        test_event_code: Option<&str>,
    ) -> Result<Accepted, DeliveryError> {
        if !numeric_id(dataset) || !token_shape(token) || events.is_empty() || events.len() > 1000 {
            return Err(DeliveryError::Rejected(0, 0));
        }
        let mut body = json!({"data":events});
        if let Some(code) = test_event_code {
            body["test_event_code"] = json!(code);
        }
        let result = self
            .http
            .post(format!("{}/{}/{dataset}/events", self.graph, self.version))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await;
        let mut response = match result {
            Ok(r) => r,
            Err(e) if e.is_connect() => return Err(DeliveryError::Retry),
            // The request may have reached Meta.
            Err(_) => return Err(DeliveryError::Uncertain),
        };
        let status = response.status().as_u16();
        let Some(body) = Self::body(&mut response).await else {
            return Err(if status >= 500 {
                DeliveryError::Uncertain
            } else {
                DeliveryError::Rejected(i64::from(status), 0)
            });
        };
        if let Some(e) = body.get("error") {
            return Err(delivery_error(e));
        }
        if status != 200 {
            return Err(DeliveryError::Uncertain);
        }
        let received = body["events_received"].as_i64().unwrap_or(-1);
        if received != i64::try_from(events.len()).unwrap_or(-1) {
            return Err(DeliveryError::Uncertain);
        }
        Ok(Accepted {
            events_received: received,
            trace_id: trace(&body),
        })
    }
}
fn trace(body: &Value) -> String {
    body["fbtrace_id"]
        .as_str()
        .filter(|t| {
            t.len() <= 100
                && t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .unwrap_or_default()
        .to_string()
}
fn codes(e: &Value) -> (i64, i64, bool) {
    (
        e["code"].as_i64().unwrap_or(-1),
        e["error_subcode"].as_i64().unwrap_or(0),
        e["is_transient"].as_bool() == Some(true),
    )
}
/// Graph API error codes: 190 invalid token, 102 session, 10/200-299 permissions,
/// 100 with subcode 33 an object that does not exist or cannot be loaded, 1/2/4/17/32/
/// 341/613 transient or rate limits.
fn setup_error(e: &Value) -> Error {
    match codes(e) {
        (190 | 102, _, _) => Error::Token,
        (10 | 200..=299, _, _) | (100, 33, _) => Error::Access,
        (1 | 2 | 4 | 17 | 32 | 341 | 613, _, _) | (_, _, true) => Error::Unavailable,
        _ => Error::Response,
    }
}
fn delivery_error(e: &Value) -> DeliveryError {
    match codes(e) {
        (190 | 102, _, _) | (10 | 200..=299, _, _) | (100, 33, _) => DeliveryError::Authorization,
        (1 | 2 | 4 | 17 | 32 | 341 | 613, _, _) | (_, _, true) => DeliveryError::Retry,
        (code, sub, _) => DeliveryError::Rejected(code, sub),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_names_and_errors() {
        assert_eq!(setup_error(&json!({"code":190})), Error::Token);
        assert_eq!(
            setup_error(&json!({"code":100,"error_subcode":33})),
            Error::Access
        );
        assert_eq!(setup_error(&json!({"code":4})), Error::Unavailable);
        assert_eq!(
            delivery_error(&json!({"code":190})),
            DeliveryError::Authorization
        );
        assert_eq!(delivery_error(&json!({"code":613})), DeliveryError::Retry);
        assert_eq!(
            delivery_error(&json!({"code":100,"error_subcode":2804003})),
            DeliveryError::Rejected(100, 2804003)
        );
        assert_eq!(
            delivery_error(&json!({"code":1,"is_transient":true})),
            DeliveryError::Retry
        );
    }
}
