//! Read-only SignalWire verification. Never purchases a number or rewrites routing.
use super::twilio_api::{Inventory, Number};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "SignalWire rejected the credentials. Check the Project ID, API token, and token permissions."
    )]
    Credentials,
    #[error("SignalWire is unavailable or rate limited. Try again shortly.")]
    Unavailable,
    #[error("SignalWire returned an unexpected response.")]
    Response,
    #[error("That voice-capable number was not found in this SignalWire account.")]
    Number,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    origin: String,
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
            origin: String::new(),
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
        space: &str,
        account: &str,
        token: &str,
        suffix: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, Error> {
        if !valid_space(space) || !valid_id(account) {
            return Err(Error::Credentials);
        }
        let mut response = self
            .http
            .get(format!(
                "{}/api/laml/2010-04-01/Accounts/{account}{suffix}",
                if self.origin.is_empty() {
                    format!("https://{space}")
                } else {
                    self.origin.clone()
                }
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

    pub async fn verify_account(
        &self,
        space: &str,
        account: &str,
        token: &str,
    ) -> Result<(), Error> {
        let value = self.get(space, account, token, "", &[]).await?;
        if value["sid"].as_str() != Some(account) || value["status"].as_str() != Some("active") {
            return Err(Error::Credentials);
        }
        Ok(())
    }

    pub async fn numbers(
        &self,
        space: &str,
        account: &str,
        token: &str,
        phone: Option<&str>,
    ) -> Result<Inventory, Error> {
        let mut query = vec![("PageSize", "100")];
        if let Some(phone) = phone {
            query.push(("PhoneNumber", phone));
        }
        let value = self
            .get(space, account, token, "/IncomingPhoneNumbers", &query)
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
            if !valid_id(&number.sid) {
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
        space: &str,
        account: &str,
        token: &str,
        phone: &str,
    ) -> Result<(), Error> {
        let inventory = self.numbers(space, account, token, Some(phone)).await?;
        if !inventory.numbers.iter().any(|n| n.phone_number == phone) {
            return Err(Error::Number);
        }
        Ok(())
    }
}

/// Only a single SignalWire Space label is accepted; callers cannot supply URLs or paths.
pub fn valid_space(space: &str) -> bool {
    space.strip_suffix(".signalwire.com").is_some_and(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricts_origin_and_identifiers() {
        assert!(valid_space("quarry-test.signalwire.com"));
        for host in [
            "signalwire.com",
            "evil.signalwire.com.evil.test",
            "https://test.signalwire.com",
            "a.b.signalwire.com",
            "-a.signalwire.com",
            "localhost",
            "a.signalwire.com:443",
            "a@b.signalwire.com",
        ] {
            assert!(!valid_space(host), "{host}");
        }
        assert!(valid_id("00000000-0000-4000-8000-000000000001"));
        assert!(!valid_id("../Accounts"));
    }
}
