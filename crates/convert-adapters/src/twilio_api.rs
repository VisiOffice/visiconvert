//! Read-only Twilio verification. Never purchases a number or rewrites routing.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Twilio rejected the credentials. Check the Account SID and Auth Token.")]
    Credentials,
    #[error("Twilio is unavailable or rate limited. Try again shortly.")]
    Unavailable,
    #[error("Twilio returned an unexpected response.")]
    Response,
    #[error("That voice-capable number was not found in this Twilio account.")]
    Number,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    origin: String,
}

#[derive(Deserialize, Serialize)]
pub struct Number {
    pub sid: String,
    pub phone_number: String,
    pub friendly_name: String,
}

#[derive(Serialize)]
pub struct Inventory {
    pub numbers: Vec<Number>,
    pub has_more: bool,
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
            origin: "https://api.twilio.com".into(),
        })
    }

    #[cfg(feature = "test-support")]
    pub fn for_test(origin: String) -> Self {
        let mut client = Self::new().unwrap();
        client.origin = origin;
        client
    }

    async fn get(
        &self,
        account: &str,
        token: &str,
        suffix: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, Error> {
        if !super::twilio::valid_sid(account, "AC") {
            return Err(Error::Credentials);
        }
        let mut response = self
            .http
            .get(format!(
                "{}/2010-04-01/Accounts/{account}{suffix}",
                self.origin
            ))
            .basic_auth(account, Some(token))
            .query(query)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(Error::Credentials),
            429 | 500..=599 => return Err(Error::Unavailable),
            _ => return Err(Error::Response),
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if body.len() + chunk.len() > 1_048_576 {
                return Err(Error::Response);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| Error::Response)
    }

    pub async fn verify_account(&self, account: &str, token: &str) -> Result<(), Error> {
        let value = self.get(account, token, ".json", &[]).await?;
        if value["sid"].as_str() != Some(account) || value["status"].as_str() != Some("active") {
            return Err(Error::Credentials);
        }
        Ok(())
    }

    pub async fn numbers(
        &self,
        account: &str,
        token: &str,
        phone: Option<&str>,
    ) -> Result<Inventory, Error> {
        let mut query = vec![("PageSize", "100")];
        if let Some(phone) = phone {
            query.push(("PhoneNumber", phone));
        }
        let value = self
            .get(account, token, "/IncomingPhoneNumbers.json", &query)
            .await?;
        let entries = value["incoming_phone_numbers"]
            .as_array()
            .ok_or(Error::Response)?;
        let mut numbers = Vec::new();
        for entry in entries {
            if entry["account_sid"].as_str() != Some(account) {
                return Err(Error::Response);
            }
            if entry["capabilities"]["voice"].as_bool() != Some(true) {
                continue;
            }
            let number: Number =
                serde_json::from_value(entry.clone()).map_err(|_| Error::Response)?;
            if !super::twilio::valid_sid(&number.sid, "PN") {
                return Err(Error::Response);
            }
            if convert_domain::us_number(&number.phone_number).is_ok() {
                numbers.push(number);
            }
        }
        Ok(Inventory {
            numbers,
            has_more: value["next_page_uri"]
                .as_str()
                .is_some_and(|v| !v.is_empty()),
        })
    }

    pub async fn verify_number(
        &self,
        account: &str,
        token: &str,
        phone: &str,
    ) -> Result<(), Error> {
        let inventory = self.numbers(account, token, Some(phone)).await?;
        if !inventory.numbers.iter().any(|n| n.phone_number == phone) {
            return Err(Error::Number);
        }
        Ok(())
    }
}
