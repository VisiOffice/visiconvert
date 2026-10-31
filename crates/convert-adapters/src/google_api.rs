//! Google OAuth, Ads discovery and bounded Data Manager transport.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
pub const SCOPES: &str =
    "https://www.googleapis.com/auth/adwords https://www.googleapis.com/auth/datamanager";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Google authorization expired or was revoked. Reconnect Google.")]
    Credentials,
    #[error(
        "A VisiConvert action already exists but its configuration changed or it was removed. Review it in Google Ads or use existing actions; no replacement was created."
    )]
    ActionConflict,
    #[error(
        "Google action creation could not be confirmed. Retry setup to recover any actions already created; do not create replacements manually."
    )]
    CreationUncertain,
    #[error(
        "Google did not grant both required permissions. Reconnect and allow Google Ads and Data Manager access."
    )]
    Scope,
    #[error(
        "Google rejected the OAuth application configuration. Ask the administrator to check its client and redirect URI."
    )]
    Configuration,
    #[error(
        "Google denied access. Check account permissions, enabled APIs and the Cloud project's production API access level."
    )]
    Access,
    #[error("Google is unavailable or rate limited. Try again shortly.")]
    Unavailable,
    #[error(
        "Google returned an unexpected response. Try again or ask the administrator to check the API configuration."
    )]
    Response,
}
#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    token_type: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub manager: bool,
    pub status: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub status: String,
    pub kind: String,
    pub owner: String,
    pub primary: bool,
    pub window_days: i64,
    pub eligible: bool,
}
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    oauth: String,
    ads: String,
    manager: String,
}
pub fn customer_id(s: &str) -> bool {
    s.len() == 10 && s.bytes().all(|c| c.is_ascii_digit())
}
pub fn action_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit())
}
fn granted(scope: &str) -> bool {
    SCOPES
        .split_whitespace()
        .all(|s| scope.split_whitespace().any(|v| v == s))
}
fn secret(s: &str) -> bool {
    !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control)
}
impl Client {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .user_agent("VisiConvert/0.1")
                .build()
                .map_err(|_| Error::Unavailable)?,
            oauth: "https://oauth2.googleapis.com".into(),
            ads: "https://googleads.googleapis.com/v26".into(),
            manager: "https://datamanager.googleapis.com/v1".into(),
        })
    }
    #[cfg(feature = "test-support")]
    pub fn for_test(origin: String) -> Self {
        let mut c = Self::new().unwrap();
        c.oauth = origin.clone();
        c.manager = format!("{origin}/v1");
        c.ads = format!("{origin}/v26");
        c
    }
    async fn response(
        &self,
        request: reqwest::RequestBuilder,
        oauth: bool,
    ) -> Result<Value, Error> {
        let mut response = request.send().await.map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(Error::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            if status >= 500 {
                Error::Unavailable
            } else {
                Error::Response
            }
        })?;
        match status {
            200 => Ok(value),
            401 => Err(Error::Credentials),
            403 => Err(Error::Access),
            429 | 500..=599 => Err(Error::Unavailable),
            400 if oauth => Err(match value["error"].as_str() {
                Some("invalid_grant") => Error::Credentials,
                Some("invalid_client" | "redirect_uri_mismatch") => Error::Configuration,
                _ => Error::Scope,
            }),
            400 if value["error"]["details"].as_array().is_some_and(|details| {
                details.iter().any(|d| {
                    d["errors"].as_array().is_some_and(|errors| {
                        errors
                            .iter()
                            .any(|e| e["errorCode"]["conversionActionError"] == "DUPLICATE_NAME")
                    })
                })
            }) =>
            {
                Err(Error::CreationUncertain)
            }
            400 => Err(Error::Access),
            _ => Err(Error::Response),
        }
    }
    async fn token(&self, form: &[(&str, &str)], initial: bool) -> Result<Tokens, Error> {
        let value = self
            .response(
                self.http.post(format!("{}/token", self.oauth)).form(form),
                true,
            )
            .await?;
        let token: Tokens = serde_json::from_value(value).map_err(|_| Error::Response)?;
        if !secret(&token.access_token) || !token.token_type.eq_ignore_ascii_case("bearer") {
            return Err(Error::Response);
        }
        if initial && !token.refresh_token.as_deref().is_some_and(secret) {
            return Err(Error::Credentials);
        }
        if token.refresh_token.as_deref().is_some_and(|s| !secret(s)) {
            return Err(Error::Response);
        }
        if token.scope.as_deref().is_some_and(|s| !granted(s)) || (initial && token.scope.is_none())
        {
            return Err(Error::Scope);
        }
        Ok(token)
    }
    pub async fn exchange(
        &self,
        id: &str,
        secret: &str,
        redirect: &str,
        code: &str,
        verifier: &str,
    ) -> Result<Tokens, Error> {
        self.token(
            &[
                ("client_id", id),
                ("client_secret", secret),
                ("redirect_uri", redirect),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
            ],
            true,
        )
        .await
    }
    pub async fn refresh(&self, id: &str, secret: &str, refresh: &str) -> Result<Tokens, Error> {
        self.token(
            &[
                ("client_id", id),
                ("client_secret", secret),
                ("refresh_token", refresh),
                ("grant_type", "refresh_token"),
            ],
            false,
        )
        .await
    }
    pub async fn logins(&self, token: &str) -> Result<Vec<String>, Error> {
        let value = self
            .response(
                self.http
                    .get(format!("{}/customers:listAccessibleCustomers", self.ads))
                    .bearer_auth(token),
                false,
            )
            .await?;
        let Some(entries) = value.get("resourceNames") else {
            return Ok(vec![]);
        };
        let entries = entries.as_array().ok_or(Error::Response)?;
        if entries.len() > 1000 {
            return Err(Error::Response);
        }
        entries
            .iter()
            .map(|v| {
                let id = v
                    .as_str()
                    .and_then(|s| s.strip_prefix("customers/"))
                    .ok_or(Error::Response)?;
                if !customer_id(id) {
                    return Err(Error::Response);
                }
                Ok(id.to_string())
            })
            .collect()
    }
    async fn search(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        query: &str,
    ) -> Result<(Vec<Value>, bool), Error> {
        if !customer_id(login) || !customer_id(customer) {
            return Err(Error::Access);
        }
        let value = self
            .response(
                self.http
                    .post(format!(
                        "{}/customers/{customer}/googleAds:search",
                        self.ads
                    ))
                    .bearer_auth(token)
                    .header("login-customer-id", login)
                    .json(&json!({"query":query})),
                false,
            )
            .await?;
        let rows = match value.get("results") {
            None => vec![],
            Some(v) => v.as_array().ok_or(Error::Response)?.clone(),
        };
        let more = rows.len() > 200
            || value["nextPageToken"]
                .as_str()
                .is_some_and(|s| !s.is_empty());
        Ok((rows.into_iter().take(200).collect(), more))
    }
    pub async fn accounts(&self, token: &str, login: &str) -> Result<(Vec<Account>, bool), Error> {
        let (rows,more)=self.search(token,login,login,"SELECT customer_client.id, customer_client.descriptive_name, customer_client.manager, customer_client.status FROM customer_client ORDER BY customer_client.id LIMIT 201").await?;
        let accounts = rows
            .iter()
            .map(|r| {
                let v = &r["customerClient"];
                let id = v["id"].as_str().ok_or(Error::Response)?;
                if !customer_id(id) {
                    return Err(Error::Response);
                }
                Ok(Account {
                    id: id.into(),
                    name: v["descriptiveName"].as_str().unwrap_or(id).into(),
                    manager: v["manager"].as_bool().unwrap_or(false),
                    status: v["status"].as_str().unwrap_or("UNKNOWN").into(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok((accounts, more))
    }
    pub async fn actions(
        &self,
        token: &str,
        login: &str,
        customer: &str,
    ) -> Result<(Value, Vec<Action>, bool), Error> {
        self.actions_matching(token, login, customer, &[]).await
    }
    pub async fn selected_actions(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        ids: &[&str],
    ) -> Result<(Value, Vec<Action>, bool), Error> {
        if ids.is_empty() || ids.len() > 2 || ids.iter().any(|id| !action_id(id)) {
            return Err(Error::Access);
        }
        self.actions_matching(token, login, customer, ids).await
    }
    async fn actions_matching(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        ids: &[&str],
    ) -> Result<(Value, Vec<Action>, bool), Error> {
        let (rows,_)=self.search(token,login,customer,"SELECT customer.id, customer.descriptive_name, customer.manager, customer.status, customer.currency_code, customer.time_zone, customer.conversion_tracking_setting.google_ads_conversion_customer FROM customer LIMIT 1").await?;
        let c = rows
            .first()
            .ok_or(Error::Access)?
            .get("customer")
            .ok_or(Error::Response)?;
        if c["id"].as_str() != Some(customer) {
            return Err(Error::Response);
        }
        let owner = c["conversionTrackingSetting"]["googleAdsConversionCustomer"]
            .as_str()
            .unwrap_or("")
            .strip_prefix("customers/")
            .unwrap_or("");
        let account = json!({"id":customer,"name":c["descriptiveName"],"status":c["status"],"currency":c["currencyCode"],"time_zone":c["timeZone"],"conversion_owner":owner});
        let filter = if ids.is_empty() {
            String::new()
        } else {
            format!(" WHERE conversion_action.id IN ({})", ids.join(","))
        };
        let query = format!(
            "SELECT conversion_action.id, conversion_action.name, conversion_action.status, conversion_action.type, conversion_action.owner_customer, conversion_action.primary_for_goal, conversion_action.click_through_lookback_window_days FROM conversion_action{filter} ORDER BY conversion_action.id LIMIT 201"
        );
        let (rows, more) = self.search(token, login, customer, &query).await?;
        let actions = rows
            .iter()
            .map(|r| {
                let v = &r["conversionAction"];
                let id = v["id"].as_str().ok_or(Error::Response)?;
                if !action_id(id) {
                    return Err(Error::Response);
                }
                let owner = v["ownerCustomer"]
                    .as_str()
                    .unwrap_or("")
                    .strip_prefix("customers/")
                    .unwrap_or("");
                let status = v["status"].as_str().unwrap_or("UNKNOWN");
                let kind = v["type"].as_str().unwrap_or("UNKNOWN");
                Ok(Action {
                    id: id.into(),
                    name: v["name"].as_str().unwrap_or(id).into(),
                    status: status.into(),
                    kind: kind.into(),
                    owner: owner.into(),
                    primary: v["primaryForGoal"].as_bool().unwrap_or(false),
                    window_days: v["clickThroughLookbackWindowDays"].as_i64().unwrap_or(0),
                    eligible: owner == customer && status == "ENABLED" && kind == "UPLOAD_CLICKS",
                })
            })
            .collect::<Result<_, _>>()?;
        Ok((account, actions, more))
    }
    /// Exact lookup avoids the 200-action discovery cap. Names are generated from
    /// a workspace UUID, never from user input. Google enforces unique names.
    pub async fn setup_action(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        name: &str,
        known_id: Option<&str>,
        kind: &str,
        currency: &str,
    ) -> Result<String, Error> {
        if !customer_id(customer)
            || !customer_id(login)
            || !matches!(kind, "qualified" | "booking")
            || currency.len() != 3
            || !currency.bytes().all(|b| b.is_ascii_uppercase())
            || name.len() > 100
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" -()".contains(&b))
            || known_id.is_some_and(|id| !action_id(id))
        {
            return Err(Error::Response);
        }
        let filter = match known_id {
            Some(id) => format!("conversion_action.id = {id}"),
            None => format!("conversion_action.name = '{name}'"),
        };
        let query = format!(
            "SELECT conversion_action.id, conversion_action.name, conversion_action.type, conversion_action.status, conversion_action.owner_customer, conversion_action.category, conversion_action.counting_type, conversion_action.value_settings.default_value, conversion_action.value_settings.default_currency_code, conversion_action.value_settings.always_use_default_value FROM conversion_action WHERE {filter} AND conversion_action.status IN ('ENABLED', 'HIDDEN', 'REMOVED') LIMIT 2"
        );
        let (rows, more) = self.search(token, login, customer, &query).await?;
        let category = if kind == "qualified" {
            "QUALIFIED_LEAD"
        } else {
            "PURCHASE"
        };
        let count = if kind == "qualified" {
            "ONE_PER_CLICK"
        } else {
            "MANY_PER_CLICK"
        };
        if more || rows.len() > 1 {
            return Err(Error::ActionConflict);
        }
        if let Some(row) = rows.first() {
            let v = &row["conversionAction"];
            let id = v["id"]
                .as_str()
                .filter(|id| action_id(id))
                .ok_or(Error::Response)?;
            let value = &v["valueSettings"];
            if v["type"] != "UPLOAD_CLICKS"
                || v["status"] != "ENABLED"
                || v["ownerCustomer"] != format!("customers/{customer}")
                || v["category"] != category
                || v["countingType"] != count
                || value["alwaysUseDefaultValue"].as_bool().unwrap_or(false)
                    != (kind == "qualified")
                || value["defaultValue"].as_f64().unwrap_or(0.0) != 0.0
                || value["defaultCurrencyCode"] != currency
            {
                return Err(Error::ActionConflict);
            }
            return Ok(id.into());
        }
        if known_id.is_some() {
            return Err(Error::ActionConflict);
        }
        let value = self.response(self.http
            .post(format!("{}/customers/{customer}/conversionActions:mutate", self.ads))
            .bearer_auth(token).header("login-customer-id", login)
            .json(&json!({"partialFailure":false,"validateOnly":false,"operations":[{"create":{
                "name":name,"type":"UPLOAD_CLICKS","status":"ENABLED","category":category,
                "countingType":count,"primaryForGoal":false,"clickThroughLookbackWindowDays":90,
                "valueSettings":{"defaultValue":0,"defaultCurrencyCode":currency,"alwaysUseDefaultValue":kind=="qualified"}
            }}]})), false).await.map_err(|e| match e {
                Error::Credentials | Error::Access => e,
                _ => Error::CreationUncertain,
            })?;
        if value.get("partialFailureError").is_some() {
            return Err(Error::CreationUncertain);
        }
        let rows = value["results"]
            .as_array()
            .filter(|r| r.len() == 1)
            .ok_or(Error::CreationUncertain)?;
        let prefix = format!("customers/{customer}/conversionActions/");
        let id = rows[0]["resourceName"]
            .as_str()
            .and_then(|s| s.strip_prefix(&prefix))
            .filter(|id| action_id(id))
            .ok_or(Error::CreationUncertain)?;
        Ok(id.into())
    }
}

/// Sanitized outcomes. Upstream bodies may contain identifiers and are never logged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeliveryError {
    Retry,
    Uncertain,
    Authorization,
    Invalid,
    Response,
}
impl DeliveryError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Retry => "Google is rate limited or unreachable; retry scheduled.",
            Self::Uncertain => {
                "Google may have received this request. Inspect Google diagnostics before any further delivery for this event."
            }
            Self::Authorization => {
                "Reconnect Google and check Data Manager access for the destination."
            }
            Self::Invalid => {
                "Google rejected the event. Check the action, identifiers, event window and consent; validate again after correction."
            }
            Self::Response => {
                "Google returned an unexpected status. Inspect the saved request ID in Google diagnostics."
            }
        }
    }
}
impl Client {
    async fn delivery_response(
        &self,
        request: reqwest::RequestBuilder,
        sending: bool,
    ) -> Result<Value, DeliveryError> {
        let mut r = request.send().await.map_err(|e| {
            if !sending || e.is_connect() {
                DeliveryError::Retry
            } else {
                DeliveryError::Uncertain
            }
        })?;
        let status = r.status().as_u16();
        match status {
            200 => (),
            401 | 403 => return Err(DeliveryError::Authorization),
            429 => return Err(DeliveryError::Retry),
            400 => return Err(DeliveryError::Invalid),
            500..=599 if sending => return Err(DeliveryError::Uncertain),
            500..=599 => return Err(DeliveryError::Retry),
            _ => {
                return Err(if sending {
                    DeliveryError::Uncertain
                } else {
                    DeliveryError::Response
                });
            }
        }
        let invalid = if sending {
            DeliveryError::Uncertain
        } else {
            DeliveryError::Response
        };
        let mut bytes = Vec::new();
        while let Some(c) = r.chunk().await.map_err(|_| invalid)? {
            if bytes.len() + c.len() > 1_048_576 {
                return Err(invalid);
            }
            bytes.extend_from_slice(&c);
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid)
    }
    pub async fn ingest(
        &self,
        token: &str,
        payload: &Value,
        validate: bool,
    ) -> Result<Value, DeliveryError> {
        let mut payload = payload.clone();
        payload["validateOnly"] = json!(validate);
        let v = self
            .delivery_response(
                self.http
                    .post(format!("{}/events:ingest", self.manager))
                    .bearer_auth(token)
                    .json(&payload),
                !validate,
            )
            .await?;
        if !validate
            && !v["requestId"].as_str().is_some_and(|s| {
                !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
            })
        {
            return Err(DeliveryError::Uncertain);
        }
        Ok(
            json!({"requestId":v["requestId"],"warnings":v["fieldWarnings"].as_array().is_some_and(|a| !a.is_empty())}),
        )
    }
    pub async fn delivery_status(
        &self,
        token: &str,
        id: &str,
        destination: &Value,
    ) -> Result<Value, DeliveryError> {
        let v = self
            .delivery_response(
                self.http
                    .get(format!("{}/requestStatus:retrieve", self.manager))
                    .bearer_auth(token)
                    .query(&[("requestId", id)]),
                false,
            )
            .await?;
        let rows = v["requestStatusPerDestination"]
            .as_array()
            .ok_or(DeliveryError::Response)?;
        if rows.len() != 1 {
            return Err(DeliveryError::Response);
        }
        let r = &rows[0];
        if r["destination"]["operatingAccount"] != destination["operatingAccount"]
            || r["destination"]["productDestinationId"] != destination["productDestinationId"]
        {
            return Err(DeliveryError::Response);
        }
        let state = match r["requestStatus"].as_str() {
            Some("SUCCESS") if r["eventsIngestionStatus"]["recordCount"] == "1" => "processed",
            Some("PROCESSING" | "REQUEST_STATUS_UNKNOWN") => "submitted",
            Some("FAILED" | "PARTIAL_SUCCESS") => "failed",
            _ => return Err(DeliveryError::Response),
        };
        Ok(
            json!({"state":state,"warnings":r["warningInfo"]["warningCounts"].as_array().is_some_and(|a| !a.is_empty()),"diagnostic":if state=="failed" {"Google reported processing errors. Inspect this request in Google diagnostics."} else {""}}),
        )
    }
}
/// Whitelist only the fields required by Data Manager; no names, notes, URLs or consent references.
pub fn event_request(snapshot: &Value, login: &str) -> Result<Value, DeliveryError> {
    let settings = &snapshot["settings"];
    let customer = settings["customer_id"]
        .as_str()
        .ok_or(DeliveryError::Invalid)?;
    let kind = snapshot["kind"].as_str().ok_or(DeliveryError::Invalid)?;
    let action = settings[if kind == "booking" {
        "booking_action_id"
    } else {
        "qualified_action_id"
    }]
    .as_str()
    .ok_or(DeliveryError::Invalid)?;
    if !customer_id(customer)
        || !customer_id(login)
        || !action_id(action)
        || !["booking", "qualified"].contains(&kind)
    {
        return Err(DeliveryError::Invalid);
    }
    let mut identifiers = serde_json::Map::new();
    for k in ["gclid", "gbraid", "wbraid"] {
        if let Some(s) = snapshot["click_identifiers"][k]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control))
        {
            identifiers.insert(k.into(), json!(s));
        }
    }
    if identifiers.is_empty() || snapshot["consent"]["ad_user_data"] != "granted" {
        return Err(DeliveryError::Invalid);
    }
    let personalization = match snapshot["consent"]["ad_personalization"].as_str() {
        Some("granted") => "CONSENT_GRANTED",
        Some("denied") => "CONSENT_DENIED",
        _ => return Err(DeliveryError::Invalid),
    };
    let mut event = json!({"transactionId":snapshot["transaction_id"],"eventTimestamp":snapshot["event_at"],"adIdentifiers":identifiers,"consent":{"adUserData":"CONSENT_GRANTED","adPersonalization":personalization},"eventSource":if snapshot["channel"]=="call" {"PHONE"} else {"WEB"}});
    if kind == "booking" {
        let cents = snapshot["amount_minor"]
            .as_i64()
            .filter(|c| *c >= 0 && *c <= 9_000_000_000_000)
            .ok_or(DeliveryError::Invalid)?;
        // Parse a decimal number from integer cents; never divide money using f64.
        event["conversionValue"] =
            serde_json::from_str(&format!("{}.{:02}", cents / 100, cents % 100))
                .map_err(|_| DeliveryError::Invalid)?;
        event["currency"] = json!("USD");
    }
    Ok(
        json!({"destinations":[{"operatingAccount":{"accountType":"GOOGLE_ADS","accountId":customer},"loginAccount":{"accountType":"GOOGLE_ADS","accountId":login},"productDestinationId":action}],"events":[event]}),
    )
}
/// Only value changes may be restated; identities and destinations stay pinned.
pub fn same_delivery_identity(a: &Value, b: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(e) = v["events"][0].as_object_mut() {
            e.remove("conversionValue");
        }
        v
    };
    strip(a) == strip(b)
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    fn snapshot() -> Value {
        json!({"kind":"booking","channel":"call","transaction_id":"vc_test","event_at":"2026-10-30T10:00:00Z","amount_minor":1,"settings":{"customer_id":"1234567890","booking_action_id":"456"},"click_identifiers":{"gclid":"click","msclkid":"not-google"},"consent":{"ad_user_data":"granted","ad_personalization":"denied","reference":"private"},"notes":"private"})
    }
    #[test]
    fn event_whitelist_money_consent_and_identity() {
        let mut s = snapshot();
        let request = event_request(&s, "1234567890").unwrap();
        assert_eq!(request["events"][0]["conversionValue"], json!(0.01));
        assert_eq!(request["events"][0]["eventSource"], "PHONE");
        assert!(!request.to_string().contains("private"));
        assert!(!request.to_string().contains("msclkid"));
        s["amount_minor"] = json!(10001);
        let update = event_request(&s, "1234567890").unwrap();
        assert!(same_delivery_identity(&request, &update));
        s["settings"]["booking_action_id"] = json!("789");
        assert!(!same_delivery_identity(
            &request,
            &event_request(&s, "1234567890").unwrap()
        ));
        s["consent"]["ad_user_data"] = json!("denied");
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
        let mut s = snapshot();
        s["consent"]["ad_personalization"] = json!("unknown");
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
        let mut s = snapshot();
        s["click_identifiers"] = json!({});
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
        let mut s = snapshot();
        s["amount_minor"] = json!(-1);
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
    }
}
