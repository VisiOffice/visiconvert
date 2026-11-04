//! Typed client for the VisiConvert HTTP API. It only calls endpoints the server already serves.
use convert_domain::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The server rejected the workspace key. Run `visiconvert login` with a current key.")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Rejected(String),
    #[error("The server returned {status}: {message}")]
    Server { status: u16, message: String },
    #[error("Could not reach {url}. Check the server URL and that it is running.")]
    Unreachable { url: String },
    #[error("The server returned a response this version of visiconvert does not understand.")]
    Decode,
}

/// A number in the connected Twilio account, as `/api/connections/twilio/numbers` returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwilioNumber {
    pub sid: String,
    pub phone_number: String,
    pub friendly_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub numbers: Vec<TwilioNumber>,
    pub has_more: bool,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl Client {
    pub fn new(base: &str, key: &str) -> anyhow::Result<Self> {
        let parsed = url::Url::parse(base)?;
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        anyhow::ensure!(
            (parsed.scheme() == "https" || (parsed.scheme() == "http" && local))
                && parsed.host_str().is_some()
                && parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "Use an HTTPS server origin; HTTP is permitted only for exact loopback hosts"
        );
        let builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("visiconvert-cli/", env!("CARGO_PKG_VERSION")));
        // Delegations are engine-local credentials and must never use an ambient proxy.
        let builder = if key.starts_with("vc_mcp_") {
            builder.no_proxy()
        } else {
            builder
        };
        Ok(Self {
            http: builder.build()?,
            base: parsed.origin().ascii_serialization(),
            key: key.to_owned(),
        })
    }

    pub fn public_key(&self) -> bool {
        self.key.starts_with("vc_key_") || self.key.starts_with("vc_mcp_")
    }
    pub async fn lead(&self, id: Uuid) -> Result<Lead, Error> {
        self.send(self.http.get(self.url(&format!("/api/leads/{id}"))))
            .await
    }
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, Error> {
        let response =
            request
                .bearer_auth(&self.key)
                .send()
                .await
                .map_err(|_| Error::Unreachable {
                    url: self.base.clone(),
                })?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let message = body["error"].as_str().unwrap_or("").to_owned();
        match status {
            200..=299 => serde_json::from_value(body).map_err(|_| Error::Decode),
            401 => Err(Error::Unauthorized),
            403 => Err(Error::Forbidden(message)),
            404 => Err(Error::NotFound(message)),
            409 => Err(Error::Conflict(message)),
            400 | 422 => Err(Error::Rejected(message)),
            _ => Err(Error::Server { status, message }),
        }
    }

    fn url(&self, path: &str) -> String {
        let path = if self.public_key() && path.starts_with("/api/") {
            let tail = path.strip_prefix("/api/").unwrap();
            format!(
                "/api/v1/{}",
                match tail {
                    "bootstrap" => "workspace",
                    "summary" => "reports/sources",
                    other => other,
                }
            )
        } else {
            path.to_owned()
        };
        format!("{}{path}", self.base)
    }

    pub async fn setup(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/setup"))).await
    }
    pub async fn sites(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/sites"))).await
    }
    pub async fn save_site(&self, id: Option<Uuid>, input: &Value) -> Result<Value, Error> {
        let request = match id {
            Some(id) => self.http.patch(self.url(&format!("/api/sites/{id}"))),
            None => self.http.post(self.url("/api/sites")),
        };
        self.send(request.json(input)).await
    }
    pub async fn site_pool(&self, id: Uuid) -> Result<Value, Error> {
        self.send(
            self.http
                .get(self.url(&format!("/api/sites/{id}/number-pool"))),
        )
        .await
    }
    pub async fn save_site_pool(&self, id: Uuid, input: &Value) -> Result<Value, Error> {
        self.send(
            self.http
                .put(self.url(&format!("/api/sites/{id}/number-pool")))
                .json(input),
        )
        .await
    }

    /// Pipeline conversions. Keys and MCP with conversions:write can define and map actions
    /// and post events; conversions:send also switches delivery, automatic sending and the
    /// consent policy. The Google connection stays owner-only in the UI.
    pub async fn conversion_setup(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/conversion-setup")))
            .await
    }
    pub async fn check_google_destination(&self, input: &Value) -> Result<Value, Error> {
        self.send(
            self.http
                .post(self.url("/api/conversion-setup/check"))
                .json(input),
        )
        .await
    }
    pub async fn set_google_destination(&self, input: &Value) -> Result<Value, Error> {
        self.send(
            self.http
                .put(self.url("/api/conversion-setup/destination"))
                .json(input),
        )
        .await
    }
    pub async fn google_review(&self, login: Option<&str>) -> Result<Value, Error> {
        let mut request = self
            .http
            .get(self.url("/api/conversion-setup/google-review"));
        if let Some(login) = login {
            request = request.query(&[("login", login)]);
        }
        self.send(request).await
    }
    pub async fn set_google_action_goal(&self, id: &str, input: &Value) -> Result<Value, Error> {
        let id = urlencoding(id);
        self.send(
            self.http
                .put(self.url(&format!("/api/conversion-setup/google-actions/{id}/goal")))
                .json(input),
        )
        .await
    }
    pub async fn conversion_delivery(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/conversion-delivery")))
            .await
    }
    pub async fn set_conversion_delivery(&self, input: &Value) -> Result<Value, Error> {
        self.send(
            self.http
                .put(self.url("/api/conversion-delivery"))
                .json(input),
        )
        .await
    }
    pub async fn conversion_actions(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/conversion-actions")))
            .await
    }
    /// Keys are validated server-side; percent-encoding keeps a bad one in its path segment.
    pub async fn save_conversion_action(&self, key: &str, input: &Value) -> Result<Value, Error> {
        let key = urlencoding(key);
        self.send(
            self.http
                .put(self.url(&format!("/api/conversion-actions/{key}")))
                .json(input),
        )
        .await
    }
    pub async fn create_google_conversion_action(
        &self,
        key: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        let key = urlencoding(key);
        self.send(
            self.http
                .post(self.url(&format!("/api/conversion-actions/{key}/google-action")))
                .json(input),
        )
        .await
    }
    pub async fn record_conversion_event(&self, input: &Value) -> Result<Value, Error> {
        self.send(
            self.http
                .post(self.url("/api/conversion-events"))
                .json(input),
        )
        .await
    }
    pub async fn conversion_events(&self, query: &[(&str, String)]) -> Result<Value, Error> {
        self.send(
            self.http
                .get(self.url("/api/conversion-events"))
                .query(query),
        )
        .await
    }
    pub async fn google_accounts(&self, login: Option<&str>) -> Result<Value, Error> {
        let mut request = self.http.get(self.url("/api/connections/google/accounts"));
        if let Some(login) = login {
            request = request.query(&[("login", login)]);
        }
        self.send(request).await
    }
    pub async fn google_actions(&self, login: &str, customer: &str) -> Result<Value, Error> {
        self.send(
            self.http
                .get(self.url("/api/connections/google/actions"))
                .query(&[("login", login), ("customer", customer)]),
        )
        .await
    }

    pub async fn health(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/health"))).await
    }

    pub async fn bootstrap(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/bootstrap"))).await
    }

    pub async fn provider_connection(
        &self,
        provider: convert_domain::PhoneProvider,
    ) -> Result<Value, Error> {
        self.send(
            self.http
                .get(self.url(&format!("/api/connections/{}", provider.as_str()))),
        )
        .await
    }
    pub async fn provider_numbers(
        &self,
        provider: convert_domain::PhoneProvider,
    ) -> Result<Inventory, Error> {
        self.send(
            self.http
                .get(self.url(&format!("/api/connections/{}/numbers", provider.as_str()))),
        )
        .await
    }
    pub async fn connection(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/connections/twilio")))
            .await
    }

    pub async fn save_twilio(
        &self,
        account_sid: &str,
        auth_token: &str,
        revision: i64,
    ) -> Result<Value, Error> {
        if self.public_key() {
            return Err(Error::Forbidden(
                "API keys cannot change provider credentials. Connect Twilio in the workspace UI."
                    .into(),
            ));
        }
        let body =
            json!({"account_sid": account_sid, "auth_token": auth_token, "revision": revision});
        self.send(
            self.http
                .put(self.url("/api/connections/twilio"))
                .json(&body),
        )
        .await
    }

    pub async fn numbers(&self) -> Result<Vec<TrackingNumber>, Error> {
        self.send(self.http.get(self.url("/api/numbers"))).await
    }

    pub async fn add_number(&self, number: &NewNumber) -> Result<Value, Error> {
        self.send(self.http.post(self.url("/api/numbers")).json(number))
            .await
    }

    pub async fn leads(&self) -> Result<Vec<Lead>, Error> {
        if !self.public_key() {
            return self.send(self.http.get(self.url("/api/leads"))).await;
        }
        let mut result = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let mut request = self
                .http
                .get(self.url("/api/leads"))
                .query(&[("limit", "100")]);
            if let Some(c) = &cursor {
                request = request.query(&[("cursor", c)]);
            }
            let page: Value = self.send(request).await?;
            let rows: Vec<Lead> =
                serde_json::from_value(page["data"].clone()).map_err(|_| Error::Decode)?;
            result.extend(rows);
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            let Some(c) = &cursor else { break };
            if !seen.insert(c.clone()) {
                return Err(Error::Decode);
            }
        }
        Ok(result)
    }

    pub async fn add_lead(&self, lead: &NewLead) -> Result<Uuid, Error> {
        let created: Value = self
            .send(self.http.post(self.url("/api/leads")).json(lead))
            .await?;
        created["id"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .ok_or(Error::Decode)
    }

    pub async fn update_lead(&self, id: Uuid, update: &LeadUpdate) -> Result<Value, Error> {
        self.send(
            self.http
                .patch(self.url(&format!("/api/leads/{id}")))
                .json(update),
        )
        .await
    }

    pub async fn outcomes(&self, id: Uuid) -> Result<Vec<Outcome>, Error> {
        self.send(
            self.http
                .get(self.url(&format!("/api/leads/{id}/outcomes"))),
        )
        .await
    }

    pub async fn put_outcome(&self, id: Uuid, outcome: &OutcomeInput) -> Result<Value, Error> {
        self.send(
            self.http
                .put(self.url(&format!("/api/leads/{id}/outcomes")))
                .json(outcome),
        )
        .await
    }

    pub async fn summary(&self) -> Result<Vec<SourceSummary>, Error> {
        self.send(self.http.get(self.url("/api/summary"))).await
    }
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
