use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid webhook signature")]
    Signature,
    #[error("Malformed Twilio request")]
    Malformed,
}

/// Only form-encoded Voice callbacks are supported. Retain all fields for signature verification.
pub fn verify(
    token: &str,
    public_url: &str,
    body: &[u8],
    signature: &str,
) -> Result<BTreeMap<String, String>, Error> {
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_bytes(body).map_err(|_| Error::Malformed)?;
    let mut fields = BTreeMap::new();
    for (key, value) in pairs {
        // Ambiguous duplicate fields must never be interpreted differently by signature and ingestion.
        if fields.insert(key, value).is_some() {
            return Err(Error::Malformed);
        }
    }
    if token.is_empty() {
        return Err(Error::Signature);
    }
    let mut mac = Hmac::<Sha1>::new_from_slice(token.as_bytes()).map_err(|_| Error::Signature)?;
    mac.update(public_url.as_bytes());
    for (key, value) in &fields {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    let decoded = STANDARD.decode(signature).map_err(|_| Error::Signature)?;
    mac.verify_slice(&decoded).map_err(|_| Error::Signature)?;
    Ok(fields)
}

pub fn fingerprint(path: &str, fields: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    h.update(path.as_bytes());
    // Length-prefix fields so concatenation cannot create an accidental event collision.
    for (k, v) in fields {
        for s in [k, v] {
            h.update((s.len() as u64).to_be_bytes());
            h.update(s.as_bytes());
        }
    }
    format!("{:x}", h.finalize())
}

pub fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn forward(
    destination: &str,
    completion_url: &str,
) -> Result<String, convert_domain::ValidationError> {
    convert_domain::us_number(destination)?;
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Dial action=\"{}\" method=\"POST\" answerOnBridge=\"true\" timeout=\"25\" record=\"do-not-record\"><Number>{}</Number></Dial></Response>",
        xml_escape(completion_url),
        destination
    ))
}

pub const HANGUP: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Hangup/></Response>";

pub fn valid_sid(value: &str, prefix: &str) -> bool {
    value.len() == 34
        && value.starts_with(prefix)
        && value.as_bytes()[2..].iter().all(u8::is_ascii_hexdigit)
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "https://example.com/myapp.php?foo=1&bar=2";
    const BODY: &[u8] = b"CallSid=CA1234567890ABCDE&Caller=%2B14158675310&Digits=1234&From=%2B14158675310&To=%2B18005551212";
    #[test]
    fn official_twilio_signature_vector() {
        assert!(verify("12345", URL, BODY, "L/OH5YylLD5NRKLltdqwSvS0BnU=").is_ok());
        assert!(verify("wrong", URL, BODY, "L/OH5YylLD5NRKLltdqwSvS0BnU=").is_err());
        assert!(
            verify(
                "12345",
                "https://evil.example",
                BODY,
                "L/OH5YylLD5NRKLltdqwSvS0BnU="
            )
            .is_err()
        );
        let mut tampered = BODY.to_vec();
        tampered.extend_from_slice(b"&NewField=changed");
        assert!(verify("12345", URL, &tampered, "L/OH5YylLD5NRKLltdqwSvS0BnU=").is_err());
    }
    #[test]
    fn duplicate_form_parameters_rejected() {
        assert!(matches!(
            verify("token", URL, b"To=one&To=two", ""),
            Err(Error::Malformed)
        ));
    }
    #[test]
    fn no_xml_injection_or_recording_by_default() {
        let xml = forward("+12025550102", "https://example.com/done?a=1&b=2").unwrap();
        assert!(xml.contains("a=1&amp;b=2"));
        assert!(xml.contains("record=\"do-not-record\""));
        assert!(forward("+12025550102<Hangup/>", URL).is_err());
    }
}
