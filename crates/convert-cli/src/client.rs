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
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("visiconvert-cli/", env!("CARGO_PKG_VERSION")))
                .build()?,
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
        })
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
        format!("{}{path}", self.base)
    }

    pub async fn health(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/health"))).await
    }

    pub async fn bootstrap(&self) -> Result<Value, Error> {
        self.send(self.http.get(self.url("/api/bootstrap"))).await
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
        let body =
            json!({"account_sid": account_sid, "auth_token": auth_token, "revision": revision});
        self.send(
            self.http
                .put(self.url("/api/connections/twilio"))
                .json(&body),
        )
        .await
    }

    pub async fn twilio_numbers(&self) -> Result<Inventory, Error> {
        self.send(self.http.get(self.url("/api/connections/twilio/numbers")))
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
        self.send(self.http.get(self.url("/api/leads"))).await
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
