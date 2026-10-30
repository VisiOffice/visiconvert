//! SignalWire Compatibility API. Verification is read-only; only `purchase` and `release`
//! change carrier inventory, and callers must gate them explicitly.
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
    #[error("That number is no longer available. Search again and choose another.")]
    Taken,
    #[error("SignalWire could not find that resource.")]
    Missing,
    #[error("SignalWire rejected the request.")]
    Rejected,
}

/// A local number offered for purchase. Not yet owned by anyone.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct Available {
    pub phone_number: String,
    #[serde(default)]
    pub locality: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
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
        self.call(
            reqwest::Method::GET,
            space,
            account,
            token,
            suffix,
            query,
            None,
        )
        .await
        .map_err(|error| match error {
            Error::Missing => Error::Response,
            other => other,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn call(
        &self,
        method: reqwest::Method,
        space: &str,
        account: &str,
        token: &str,
        suffix: &str,
        query: &[(&str, &str)],
        form: Option<&[(&str, &str)]>,
    ) -> Result<Value, Error> {
        if !valid_space(space) || !valid_id(account) {
            return Err(Error::Credentials);
        }
        let mut request = self
            .http
            .request(
                method,
                format!(
                    "{}/api/laml/2010-04-01/Accounts/{account}{suffix}",
                    if self.origin.is_empty() {
                        format!("https://{space}")
                    } else {
                        self.origin.clone()
                    }
                ),
            )
            .basic_auth(account, Some(token))
            .query(query);
        if let Some(form) = form {
            request = request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(serde_urlencoded::to_string(form).map_err(|_| Error::Response)?);
        }
        let mut response = request.send().await.map_err(|_| Error::Unavailable)?;
        match response.status().as_u16() {
            200 | 201 => {}
            204 => return Ok(Value::Null),
            400 => return Err(Error::Rejected),
            401 | 403 => return Err(Error::Credentials),
            404 => return Err(Error::Missing),
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

    /// Complete, bounded pagination. Never follow carrier-provided URLs with
    /// credentials; extract only pagination controls and preserve our filters.
    pub async fn usage_records(
        &self,
        space: &str,
        account: &str,
        token: &str,
        resource: &str,
        filters: &[(&str, &str)],
    ) -> Result<Vec<Value>, Error> {
        let key = match resource {
            "/Calls" => "calls",
            "/IncomingPhoneNumbers" => "incoming_phone_numbers",
            _ => return Err(Error::Response),
        };
        let mut paging = vec![("Page".to_string(), "0".to_string())];
        let mut seen = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        let mut records = Vec::new();
        for _ in 0..100 {
            let mut query = filters.to_vec();
            query.push(("PageSize", "1000"));
            query.extend(paging.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            let page = self.get(space, account, token, resource, &query).await?;
            let rows = page[key].as_array().ok_or(Error::Response)?;
            for row in rows {
                let sid = row["sid"]
                    .as_str()
                    .filter(|v| valid_id(v))
                    .ok_or(Error::Response)?;
                if row["account_sid"] != account
                    || !ids.insert(sid.to_string())
                    || records.len() >= 50_000
                {
                    return Err(Error::Response);
                }
                records.push(row.clone());
            }
            let next = match page.get("next_page_uri") {
                Some(Value::Null) => return Ok(records),
                Some(Value::String(s)) if s.is_empty() => return Ok(records),
                Some(Value::String(s)) => s,
                _ => return Err(Error::Response),
            };
            if !next.starts_with('/') || next.starts_with("//") || !seen.insert(next.clone()) {
                return Err(Error::Response);
            }
            let url = reqwest::Url::parse(&format!("https://{space}{next}"))
                .map_err(|_| Error::Response)?;
            let expected = format!("/api/laml/2010-04-01/Accounts/{account}{resource}");
            if url.path() != expected && url.path() != format!("{expected}.json") {
                return Err(Error::Response);
            }
            paging = url
                .query_pairs()
                .filter(|(k, _)| k == "Page" || k == "PageToken")
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            if paging.is_empty() {
                return Err(Error::Response);
            }
        }
        Err(Error::Response)
    }
    pub async fn usage_call(
        &self,
        space: &str,
        account: &str,
        token: &str,
        sid: &str,
    ) -> Result<Value, Error> {
        if !valid_id(sid) {
            return Err(Error::Response);
        }
        let call = self
            .get(space, account, token, &format!("/Calls/{sid}"), &[])
            .await?;
        if call["sid"] != sid || call["account_sid"] != account {
            return Err(Error::Response);
        }
        Ok(call)
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

    /// Local voice numbers available in a US area code. Nothing is reserved.
    pub async fn search_local(
        &self,
        space: &str,
        account: &str,
        token: &str,
        area_code: &str,
        limit: u8,
    ) -> Result<Vec<Available>, Error> {
        if area_code.len() != 3 || !area_code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Response);
        }
        let size = limit.clamp(1, 30).to_string();
        let value = self
            .get(
                space,
                account,
                token,
                "/AvailablePhoneNumbers/US/Local",
                &[
                    ("AreaCode", area_code),
                    ("VoiceEnabled", "true"),
                    ("PageSize", &size),
                ],
            )
            .await?;
        let entries = value["available_phone_numbers"]
            .as_array()
            .ok_or(Error::Response)?;
        let mut numbers = Vec::new();
        for entry in entries {
            let number: Available =
                serde_json::from_value(entry.clone()).map_err(|_| Error::Response)?;
            if convert_domain::us_number(&number.phone_number).is_ok() {
                numbers.push(number);
            }
        }
        Ok(numbers)
    }

    /// Buys one number into this project and points its voice and status webhooks at the
    /// given URLs. This is a billable carrier change.
    #[allow(clippy::too_many_arguments)]
    pub async fn purchase(
        &self,
        space: &str,
        account: &str,
        token: &str,
        phone: &str,
        voice_url: &str,
        status_url: &str,
        purchase_marker: &str,
    ) -> Result<Number, Error> {
        convert_domain::us_number(phone).map_err(|_| Error::Taken)?;
        let value = self
            .call(
                reqwest::Method::POST,
                space,
                account,
                token,
                "/IncomingPhoneNumbers",
                &[],
                Some(&[
                    ("PhoneNumber", phone),
                    ("FriendlyName", purchase_marker),
                    ("VoiceUrl", voice_url),
                    ("VoiceMethod", "POST"),
                    ("StatusCallback", status_url),
                    ("StatusCallbackMethod", "POST"),
                ]),
            )
            .await
            .map_err(|error| match error {
                // The Compatibility API answers 400/404 when someone else bought it first.
                Error::Rejected | Error::Missing => Error::Taken,
                other => other,
            })?;
        let number: Number = serde_json::from_value(value.clone()).map_err(|_| Error::Response)?;
        if value["account_sid"].as_str() != Some(account)
            || number.phone_number != phone
            || !valid_id(&number.sid)
        {
            return Err(Error::Response);
        }
        Ok(number)
    }

    /// Releases a number from this project. Releasing one that is already gone succeeds.
    pub async fn release(
        &self,
        space: &str,
        account: &str,
        token: &str,
        sid: &str,
    ) -> Result<(), Error> {
        if !valid_id(sid) {
            return Err(Error::Response);
        }
        match self
            .call(
                reqwest::Method::DELETE,
                space,
                account,
                token,
                &format!("/IncomingPhoneNumbers/{sid}"),
                &[],
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Missing) => Ok(()),
            Err(error) => Err(error),
        }
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
