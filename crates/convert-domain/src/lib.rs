use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod attribution;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub &'static str);

pub fn bounded(value: &str, max: usize) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(ValidationError("A required field is empty or too long"));
    }
    Ok(())
}

pub fn us_number(value: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    if bytes.len() != 12
        || !value.starts_with("+1")
        || !bytes[2..].iter().all(u8::is_ascii_digit)
        || !(b'2'..=b'9').contains(&bytes[2])
        || !(b'2'..=b'9').contains(&bytes[5])
    {
        return Err(ValidationError("Use a +1 number in +12025550123 format"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Qualification {
    Unreviewed,
    Qualified,
    Unqualified,
    Spam,
}

impl Qualification {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unreviewed => "unreviewed",
            Self::Qualified => "qualified",
            Self::Unqualified => "unqualified",
            Self::Spam => "spam",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PhoneProvider {
    #[default]
    Twilio,
    Signalwire,
}
impl PhoneProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Twilio => "twilio",
            Self::Signalwire => "signalwire",
        }
    }
}
impl std::str::FromStr for PhoneProvider {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "twilio" => Ok(Self::Twilio),
            "signalwire" => Ok(Self::Signalwire),
            _ => Err(ValidationError("Provider must be twilio or signalwire")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNumber {
    #[serde(default)]
    pub provider: PhoneProvider,
    pub label: String,
    pub phone_number: String,
    pub destination: String,
    pub source: String,
    pub campaign: String,
}

impl NewNumber {
    pub fn validate(&self) -> Result<(), ValidationError> {
        bounded(&self.label, 100)?;
        bounded(&self.source, 100)?;
        bounded(&self.campaign, 150)?;
        us_number(&self.phone_number)?;
        us_number(&self.destination)?;
        if self.phone_number == self.destination {
            return Err(ValidationError(
                "The destination must differ from the tracking number",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewLead {
    pub external_id: String,
    pub name: String,
    pub contact: String,
    pub source: String,
    pub campaign: String,
    pub notes: String,
}

impl NewLead {
    pub fn validate(&self) -> Result<(), ValidationError> {
        bounded(&self.external_id, 150)?;
        bounded(&self.name, 100)?;
        bounded(&self.contact, 200)?;
        bounded(&self.source, 100)?;
        if self.campaign.chars().count() > 150 || self.notes.chars().count() > 4000 {
            return Err(ValidationError("Campaign or notes are too long"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeadUpdate {
    pub version: i32,
    pub qualification: Qualification,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeInput {
    pub external_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub revision: i32,
}

impl OutcomeInput {
    pub fn validate(&self) -> Result<(), ValidationError> {
        bounded(&self.external_id, 150)?;
        if self.currency != "USD" {
            return Err(ValidationError("This pilot supports USD outcomes only"));
        }
        if self.amount_minor < 0 || self.amount_minor > 100_000_000_000 {
            return Err(ValidationError(
                "Amount must be between zero and one billion dollars",
            ));
        }
        if !["booked", "canceled"].contains(&self.status.as_str()) || self.revision < 1 {
            return Err(ValidationError(
                "Use booked or canceled and a positive revision",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingNumber {
    #[serde(default)]
    pub provider: PhoneProvider,
    pub id: Uuid,
    pub label: String,
    pub phone_number: String,
    pub destination: String,
    pub source: String,
    pub campaign: String,
    pub is_demo: bool,
    /// Bought through VisiConvert, so it can be released from here.
    #[serde(default)]
    pub purchased: bool,
    /// Set while a purchased number is in its release hold. It still forwards calls.
    #[serde(default)]
    pub release_requested_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lead {
    pub id: Uuid,
    #[serde(default)]
    pub inquiry_id: Option<Uuid>,
    pub name: String,
    pub contact: String,
    pub channel: String,
    pub source: String,
    pub campaign: String,
    pub qualification: String,
    pub notes: String,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub is_demo: bool,
    pub call_status: Option<String>,
    pub duration_seconds: Option<i32>,
    pub tracking_number: Option<String>,
    pub destination: Option<String>,
    pub booked_minor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub external_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub revision: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummary {
    pub source: String,
    pub leads: i64,
    pub qualified: i64,
    pub bookings: i64,
    pub booked_minor: i64,
}

/// This parses only supplied UTM evidence. A missing UTM is unknown, not proven direct traffic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttributionPreview {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub evidence: String,
}

pub fn attribution_preview(input: &str) -> Result<AttributionPreview, ValidationError> {
    let url = url::Url::parse(input)
        .map_err(|_| ValidationError("Enter an absolute HTTP or HTTPS URL"))?;
    if !["http", "https"].contains(&url.scheme()) {
        return Err(ValidationError("Enter an absolute HTTP or HTTPS URL"));
    }
    let param = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().chars().take(150).collect::<String>())
            .filter(|s| !s.is_empty())
    };
    let source = param("utm_source");
    Ok(AttributionPreview {
        evidence: if source.is_some() { "utm" } else { "unknown" }.into(),
        source,
        medium: param("utm_medium"),
        campaign: param("utm_campaign"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_utm_is_not_direct() {
        let value = attribution_preview("https://example.com/?gclid=click").unwrap();
        assert_eq!(value.evidence, "unknown");
        assert_eq!(value.source, None);
    }
    #[test]
    fn extracts_only_explicit_evidence() {
        let value = attribution_preview(
            "https://example.com/?utm_source=google&utm_campaign=Spring+rentals",
        )
        .unwrap();
        assert_eq!(value.campaign.as_deref(), Some("Spring rentals"));
        assert_eq!(value.source.as_deref(), Some("google"));
        assert!(attribution_preview("javascript:alert(1)").is_err());
    }
    #[test]
    fn rejects_dial_injection_and_invalid_amounts() {
        for n in [
            "+12025550101</Number>",
            "+12025550101w123",
            "911",
            "+442071234567",
        ] {
            assert!(us_number(n).is_err());
        }
        let n = NewNumber {
            provider: Default::default(),
            label: "Test".into(),
            phone_number: "+12025550101".into(),
            destination: "+12025550101".into(),
            source: "Google".into(),
            campaign: "Search".into(),
        };
        assert!(n.validate().is_err());
        let o = OutcomeInput {
            external_id: "booking-1".into(),
            amount_minor: -1,
            currency: "USD".into(),
            status: "booked".into(),
            revision: 1,
        };
        assert!(o.validate().is_err());
    }
}
