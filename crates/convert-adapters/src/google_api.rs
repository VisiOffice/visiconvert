//! Google OAuth and read-only Ads discovery. Deliberately no ingestion/mutation API.
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
        })
    }
    #[cfg(feature = "test-support")]
    pub fn for_test(origin: String) -> Self {
        let mut c = Self::new().unwrap();
        c.oauth = origin.clone();
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
        let (rows,more)=self.search(token,login,customer,"SELECT conversion_action.id, conversion_action.name, conversion_action.status, conversion_action.type, conversion_action.owner_customer, conversion_action.primary_for_goal, conversion_action.click_through_lookback_window_days FROM conversion_action ORDER BY conversion_action.id LIMIT 201").await?;
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
}
