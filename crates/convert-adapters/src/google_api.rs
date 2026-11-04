//! Google OAuth, Ads discovery and bounded Data Manager transport.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
pub const SCOPES: &str =
    "https://www.googleapis.com/auth/adwords https://www.googleapis.com/auth/datamanager";

/// What VisiConvert creates in Google for one outcome. Categories are a fixed allowlist.
#[derive(Debug, Clone)]
pub struct ActionSpec {
    pub category: &'static str,
    pub counting: &'static str,
    pub always_default: bool,
    pub currency: String,
}
pub const PIPELINE_CATEGORIES: &[&str] = &[
    "QUALIFIED_LEAD",
    "CONVERTED_LEAD",
    "SUBMIT_LEAD_FORM",
    "REQUEST_QUOTE",
    "BOOK_APPOINTMENT",
    "PURCHASE",
    "DEFAULT",
];
impl ActionSpec {
    /// The fixed reviewed outcomes: one valueless qualified lead per click, or each booking.
    pub fn reviewed(kind: &str, currency: &str) -> Option<Self> {
        Some(match kind {
            "qualified" => Self {
                category: "QUALIFIED_LEAD",
                counting: "ONE_PER_CLICK",
                always_default: true,
                currency: currency.into(),
            },
            "booking" => Self {
                category: "PURCHASE",
                counting: "MANY_PER_CLICK",
                always_default: false,
                currency: currency.into(),
            },
            _ => return None,
        })
    }
    /// A pipeline action: values come from events, counting follows the VisiConvert action.
    pub fn pipeline(counting: &str, category: Option<&str>, currency: &str) -> Option<Self> {
        let once = match counting {
            "once_per_click" => true,
            "every" => false,
            _ => return None,
        };
        let category = match category {
            Some(c) => *PIPELINE_CATEGORIES.iter().find(|v| **v == c)?,
            None if once => "QUALIFIED_LEAD",
            None => "PURCHASE",
        };
        Some(Self {
            category,
            counting: if once {
                "ONE_PER_CLICK"
            } else {
                "MANY_PER_CLICK"
            },
            always_default: false,
            currency: currency.into(),
        })
    }
    fn valid(&self) -> bool {
        PIPELINE_CATEGORIES.contains(&self.category)
            && matches!(self.counting, "ONE_PER_CLICK" | "MANY_PER_CLICK")
            && self.currency.len() == 3
            && self.currency.bytes().all(|b| b.is_ascii_uppercase())
    }
}
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
        "Google refused to change this conversion action's goal setting through the API. Some action types (for example store visits or other system actions) can only be changed in Google Ads, or not at all."
    )]
    NotMutable,
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
/// One conversion action of the saved account with its settings and recent activity,
/// for reviewing overlap with VisiConvert. All values come from Google as reported.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ReviewedAction {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub category: String,
    pub owner: String,
    #[serde(rename = "primary_for_goal")]
    pub primary: bool,
    pub counting: String,
    #[serde(rename = "click_window_days")]
    pub window_days: i64,
    pub default_value: f64,
    pub currency: String,
    pub always_use_default_value: bool,
    pub conversions_30d: f64,
    pub conversions_90d: f64,
    pub last_conversion_date: Option<String>,
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
        let (status, value) = self.read(request).await?;
        Self::classify(status, value, oauth)
    }
    async fn read(&self, request: reqwest::RequestBuilder) -> Result<(u16, Value), Error> {
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
        Ok((status, value))
    }
    fn classify(status: u16, value: Value, oauth: bool) -> Result<Value, Error> {
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
        self.search_capped(token, login, customer, query, 200).await
    }
    async fn search_capped(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        query: &str,
        cap: usize,
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
        let more = rows.len() > cap
            || value["nextPageToken"]
                .as_str()
                .is_some_and(|s| !s.is_empty());
        Ok((rows.into_iter().take(cap).collect(), more))
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
        if ids.is_empty() || ids.len() > 50 || ids.iter().any(|id| !action_id(id)) {
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
                    window_days: int64(&v["clickThroughLookbackWindowDays"]).unwrap_or(0),
                    eligible: owner == customer && status == "ENABLED" && kind == "UPLOAD_CLICKS",
                })
            })
            .collect::<Result<_, _>>()?;
        Ok((account, actions, more))
    }
    /// Exact lookup avoids the 200-action discovery cap. Names are generated from
    /// a workspace UUID and validated action keys, never free text. Google enforces
    /// unique names, so a retry finds the action an interrupted attempt created.
    pub async fn setup_action(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        name: &str,
        known_id: Option<&str>,
        spec: &ActionSpec,
    ) -> Result<String, Error> {
        if !customer_id(customer)
            || !customer_id(login)
            || !spec.valid()
            || name.len() > 100
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" -()_".contains(&b))
            || known_id.is_some_and(|id| !action_id(id))
        {
            return Err(Error::Response);
        }
        let (category, count, fixed, currency) = (
            spec.category,
            spec.counting,
            spec.always_default,
            spec.currency.as_str(),
        );
        let filter = match known_id {
            Some(id) => format!("conversion_action.id = {id}"),
            None => format!("conversion_action.name = '{name}'"),
        };
        let query = format!(
            "SELECT conversion_action.id, conversion_action.name, conversion_action.type, conversion_action.status, conversion_action.owner_customer, conversion_action.category, conversion_action.counting_type, conversion_action.value_settings.default_value, conversion_action.value_settings.default_currency_code, conversion_action.value_settings.always_use_default_value FROM conversion_action WHERE {filter} AND conversion_action.status IN ('ENABLED', 'HIDDEN', 'REMOVED') LIMIT 2"
        );
        let (rows, more) = self.search(token, login, customer, &query).await?;
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
                || value["alwaysUseDefaultValue"].as_bool().unwrap_or(false) != fixed
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
                "valueSettings":{"defaultValue":0,"defaultCurrencyCode":currency,"alwaysUseDefaultValue":fixed}
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

impl Client {
    /// Every non-removed conversion action in `customer` with settings and all-conversion
    /// activity. Dates are `YYYY-MM-DD`: the last 30 and 90 days up to `today`. The last
    /// conversion date comes from the newest 1000 daily rows, so it can be missing for a
    /// rarely used action in a very busy account; totals are exact.
    pub async fn review_actions(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        (today, from30, from90): (&str, &str, &str),
    ) -> Result<(Vec<ReviewedAction>, bool), Error> {
        let date = |d: &str| d.len() == 10 && d.bytes().all(|b| b.is_ascii_digit() || b == b'-');
        if ![today, from30, from90].into_iter().all(date) {
            return Err(Error::Response);
        }
        let (rows, more) = self.search(token, login, customer, "SELECT conversion_action.id, conversion_action.name, conversion_action.status, conversion_action.type, conversion_action.category, conversion_action.owner_customer, conversion_action.primary_for_goal, conversion_action.counting_type, conversion_action.click_through_lookback_window_days, conversion_action.value_settings.default_value, conversion_action.value_settings.default_currency_code, conversion_action.value_settings.always_use_default_value FROM conversion_action WHERE conversion_action.status != 'REMOVED' ORDER BY conversion_action.id LIMIT 201").await?;
        let mut actions = rows
            .iter()
            .map(|r| {
                let v = &r["conversionAction"];
                let id = v["id"]
                    .as_str()
                    .filter(|id| action_id(id))
                    .ok_or(Error::Response)?;
                let value = &v["valueSettings"];
                Ok(ReviewedAction {
                    id: id.into(),
                    name: v["name"].as_str().unwrap_or(id).into(),
                    status: v["status"].as_str().unwrap_or("UNKNOWN").into(),
                    kind: v["type"].as_str().unwrap_or("UNKNOWN").into(),
                    category: v["category"].as_str().unwrap_or("UNKNOWN").into(),
                    owner: v["ownerCustomer"]
                        .as_str()
                        .unwrap_or("")
                        .strip_prefix("customers/")
                        .unwrap_or("")
                        .into(),
                    primary: v["primaryForGoal"].as_bool().unwrap_or(false),
                    counting: v["countingType"].as_str().unwrap_or("UNKNOWN").into(),
                    window_days: int64(&v["clickThroughLookbackWindowDays"]).unwrap_or(0),
                    default_value: value["defaultValue"].as_f64().unwrap_or(0.0),
                    currency: value["defaultCurrencyCode"].as_str().unwrap_or("").into(),
                    always_use_default_value: value["alwaysUseDefaultValue"]
                        .as_bool()
                        .unwrap_or(false),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let prefix = format!("customers/{customer}/conversionActions/");
        let action_of = |r: &Value| -> Option<String> {
            r["segments"]["conversionAction"]
                .as_str()?
                .strip_prefix(&prefix)
                .map(String::from)
        };
        let conversions = |r: &Value| r["metrics"]["allConversions"].as_f64().unwrap_or(0.0);
        for (from, ninety) in [(from30, false), (from90, true)] {
            let (rows, _) = self.search_capped(token, login, customer, &format!("SELECT segments.conversion_action, metrics.all_conversions FROM customer WHERE segments.date BETWEEN '{from}' AND '{today}'"), 1000).await?;
            for r in &rows {
                if let Some(id) = action_of(r)
                    && let Some(a) = actions.iter_mut().find(|a| a.id == id)
                {
                    if ninety {
                        a.conversions_90d += conversions(r)
                    } else {
                        a.conversions_30d += conversions(r)
                    }
                }
            }
        }
        let (rows, _) = self.search_capped(token, login, customer, &format!("SELECT segments.conversion_action, segments.date, metrics.all_conversions FROM customer WHERE segments.date BETWEEN '{from90}' AND '{today}' AND metrics.all_conversions > 0 ORDER BY segments.date DESC LIMIT 1000"), 1000).await?;
        for r in &rows {
            if let (Some(id), Some(day)) = (action_of(r), r["segments"]["date"].as_str())
                && let Some(a) = actions.iter_mut().find(|a| a.id == id)
                && a.last_conversion_date.as_deref().is_none_or(|d| d < day)
            {
                a.last_conversion_date = Some(day.into());
            }
        }
        Ok((actions, more))
    }
    /// Change only `primary_for_goal` of one action. Nothing else is sent, so names,
    /// values, counting and status cannot change. A 400 means Google refused the change.
    pub async fn set_action_primary(
        &self,
        token: &str,
        login: &str,
        customer: &str,
        id: &str,
        primary: bool,
    ) -> Result<(), Error> {
        if !customer_id(customer) || !customer_id(login) || !action_id(id) {
            return Err(Error::Response);
        }
        let resource = format!("customers/{customer}/conversionActions/{id}");
        let (status, value) = self
            .read(
                self.http
                    .post(format!(
                        "{}/customers/{customer}/conversionActions:mutate",
                        self.ads
                    ))
                    .bearer_auth(token)
                    .header("login-customer-id", login)
                    .json(
                        &json!({"partialFailure":false,"validateOnly":false,"operations":[{
                        "update":{"resourceName":resource,"primaryForGoal":primary},
                        "updateMask":"primary_for_goal"}]}),
                    ),
            )
            .await?;
        if status == 400 {
            return Err(Error::NotMutable);
        }
        let value = Self::classify(status, value, false)?;
        let ok = value["results"]
            .as_array()
            .is_some_and(|r| r.len() == 1 && r[0]["resourceName"] == resource.as_str());
        if !ok {
            return Err(Error::Response);
        }
        Ok(())
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
    if snapshot["kind"] == "pipeline" {
        return pipeline_request(snapshot, login);
    }
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
fn identifiers(snapshot: &Value) -> serde_json::Map<String, Value> {
    let mut identifiers = serde_json::Map::new();
    for k in ["gclid", "gbraid", "wbraid"] {
        if let Some(s) = snapshot["click_identifiers"][k]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control))
        {
            identifiers.insert(k.into(), json!(s));
        }
    }
    identifiers
}
/// Automatic pipeline event: action ID from the snapshot, optional value, and no consent
/// block when the workspace attested it does not advertise in the EEA, UK or Switzerland.
fn pipeline_request(snapshot: &Value, login: &str) -> Result<Value, DeliveryError> {
    let customer = snapshot["customer_id"]
        .as_str()
        .ok_or(DeliveryError::Invalid)?;
    let action = snapshot["google_action_id"]
        .as_str()
        .ok_or(DeliveryError::Invalid)?;
    let identifiers = identifiers(snapshot);
    if !customer_id(customer) || !customer_id(login) || !action_id(action) || identifiers.is_empty()
    {
        return Err(DeliveryError::Invalid);
    }
    let mut event = json!({"transactionId":snapshot["transaction_id"],"eventTimestamp":snapshot["event_at"],"adIdentifiers":identifiers,"eventSource":if snapshot["channel"]=="call" {"PHONE"} else {"WEB"}});
    match snapshot["consent_policy"].as_str() {
        Some("not_required_outside_eea") => (),
        Some("per_lead") => {
            if snapshot["consent"]["ad_user_data"] != "granted" {
                return Err(DeliveryError::Invalid);
            }
            let personalization = match snapshot["consent"]["ad_personalization"].as_str() {
                Some("granted") => "CONSENT_GRANTED",
                Some("denied") => "CONSENT_DENIED",
                _ => return Err(DeliveryError::Invalid),
            };
            event["consent"] =
                json!({"adUserData":"CONSENT_GRANTED","adPersonalization":personalization});
        }
        _ => return Err(DeliveryError::Invalid),
    }
    if !snapshot["value_minor"].is_null() {
        let cents = snapshot["value_minor"]
            .as_i64()
            .filter(|c| *c >= 0 && *c <= 9_000_000_000_000)
            .ok_or(DeliveryError::Invalid)?;
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
            e.remove("currency");
        }
        v
    };
    strip(a) == strip(b)
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    #[test]
    fn pipeline_request_consent_policy_value_and_identity() {
        let mut s = json!({"kind":"pipeline","channel":"form","transaction_id":"vc_p","event_at":"2026-10-30T10:00:00Z","value_minor":2000,"customer_id":"1234567890","google_action_id":"321","click_identifiers":{"gclid":"click"},"consent_policy":"not_required_outside_eea","consent":{"ad_user_data":"unknown","reference":"private"},"external_id":"private-order"});
        let r = event_request(&s, "1234567890").unwrap();
        let e = &r["events"][0];
        assert!(e.get("consent").is_none());
        assert_eq!(e["conversionValue"], json!(20.0));
        assert_eq!(r["destinations"][0]["productDestinationId"], "321");
        assert!(!r.to_string().contains("private"));
        s["value_minor"] = Value::Null;
        let none = event_request(&s, "1234567890").unwrap();
        assert!(none["events"][0].get("conversionValue").is_none());
        assert!(same_delivery_identity(&r, &none));
        s["consent_policy"] = json!("per_lead");
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
        s["consent"] = json!({"ad_user_data":"granted","ad_personalization":"granted"});
        assert_eq!(
            event_request(&s, "1234567890").unwrap()["events"][0]["consent"]["adPersonalization"],
            "CONSENT_GRANTED"
        );
        s["consent_policy"] = json!("other");
        assert_eq!(event_request(&s, "1234567890"), Err(DeliveryError::Invalid));
    }
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

/// Google's REST API encodes int64 fields as JSON strings (proto3 JSON mapping), so read
/// either form. Reading only numbers silently turned every click window into 0.
fn int64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
#[cfg(test)]
mod int64_tests {
    use super::*;
    #[test]
    fn int64_reads_google_string_encoding() {
        assert_eq!(int64(&json!("90")), Some(90));
        assert_eq!(int64(&json!(30)), Some(30));
        assert_eq!(int64(&json!("x")), None);
        assert_eq!(int64(&Value::Null), None);
    }
}
