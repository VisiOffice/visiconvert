//! Operations shared by the CLI and the MCP server. Each one maps onto existing API endpoints and
//! adds the bookkeeping a person or agent should not have to do by hand: versions, revisions,
//! short lead IDs and dollar amounts.
use crate::client::{Client, TwilioNumber};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use convert_domain::*;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Status {
    pub server: String,
    pub healthy: bool,
    pub workspace: Value,
    pub twilio: Value,
    pub signalwire: Value,
    pub tracking_numbers: usize,
    pub next_steps: Vec<String>,
}

pub async fn status(client: &Client) -> Result<Status> {
    let healthy = client.health().await.is_ok();
    let workspace = client.bootstrap().await?;
    let twilio = match client.connection().await {
        Ok(value) => value,
        Err(crate::client::Error::Forbidden(message)) => {
            json!({"visible": false, "message": message})
        }
        Err(error) => return Err(error.into()),
    };
    let signalwire = match client.provider_connection(PhoneProvider::Signalwire).await {
        Ok(value) => value,
        Err(crate::client::Error::Forbidden(message)) => json!({"visible":false,"message":message}),
        Err(error) => return Err(error.into()),
    };
    let tracking = client
        .numbers()
        .await?
        .into_iter()
        .filter(|n| !n.is_demo)
        .count();
    let mut next_steps = Vec::new();
    if twilio["connected"] == false && signalwire["connected"] == false {
        if let Some(message) = twilio["setup_message"].as_str() {
            next_steps.push(message.to_owned());
        } else {
            next_steps.push(if client.public_key() { "Connect Twilio or SignalWire in the workspace UI. Scoped API keys cannot change provider credentials.".into() } else { "Connect Twilio: visiconvert connect twilio --account-sid AC… --token-stdin".into() });
        }
    } else if tracking == 0 {
        next_steps.push(
            "Add a tracking number: visiconvert numbers available, then visiconvert numbers add"
                .into(),
        );
    }
    Ok(Status {
        server: client.base().to_owned(),
        healthy,
        workspace,
        twilio,
        signalwire,
        tracking_numbers: tracking,
        next_steps,
    })
}

pub async fn connect_twilio(client: &Client, account_sid: &str, auth_token: &str) -> Result<Value> {
    if !account_sid.starts_with("AC") || account_sid.len() != 34 {
        bail!("A Twilio Account SID starts with AC and is 34 characters long");
    }
    let current = client.connection().await?;
    let revision = current["revision"].as_i64().unwrap_or(0);
    client
        .save_twilio(account_sid, auth_token.trim(), revision)
        .await?;
    Ok(client.connection().await?)
}

/// Numbers in the connected Twilio account that are not tracked yet.
pub async fn available_numbers(client: &Client) -> Result<Value> {
    available_numbers_for(client, PhoneProvider::Twilio).await
}
pub async fn available_numbers_for(client: &Client, provider: PhoneProvider) -> Result<Value> {
    let inventory = client.provider_numbers(provider).await?;
    let tracked: Vec<String> = client
        .numbers()
        .await?
        .into_iter()
        .map(|n| n.phone_number)
        .collect();
    let untracked: Vec<TwilioNumber> = inventory
        .numbers
        .into_iter()
        .filter(|n| !tracked.contains(&n.phone_number))
        .collect();
    Ok(json!({"untracked": untracked, "has_more": inventory.has_more}))
}

pub struct AddNumber {
    pub provider: PhoneProvider,
    pub phone_number: String,
    pub forward_to: String,
    pub source: String,
    pub campaign: String,
    pub label: Option<String>,
}

pub async fn add_number(client: &Client, input: AddNumber) -> Result<Value> {
    let number = NewNumber {
        provider: input.provider,
        label: input.label.unwrap_or_else(|| input.campaign.clone()),
        phone_number: normalize_phone(&input.phone_number)?,
        destination: normalize_phone(&input.forward_to)?,
        source: input.source,
        campaign: input.campaign,
    };
    number.validate().map_err(|e| anyhow!(e.0))?;
    client.add_number(&number).await?;
    let connection = client
        .provider_connection(input.provider)
        .await
        .unwrap_or(Value::Null);
    let voice_url = connection["voice_url"].as_str().unwrap_or("").to_owned();
    let status_url = connection["status_url"].as_str().unwrap_or("").to_owned();
    let provider_name = match input.provider {
        PhoneProvider::Twilio => "Twilio",
        PhoneProvider::Signalwire => "SignalWire",
    };
    let manual_step = if voice_url.is_empty() {
        format!(
            "Run `visiconvert status` as the workspace owner to see the webhook URLs to set in {provider_name}."
        )
    } else {
        format!(
            "In the {provider_name} console, configure incoming voice and call status webhooks as HTTP POST using the URLs below. For SignalWire select cXML handling. Preserve the old routing and test a call before moving production traffic."
        )
    };
    Ok(json!({
        "tracking_number": number.phone_number,
        "forwards_to": number.destination,
        "source": number.source,
        "campaign": number.campaign,
        "label": number.label,
        "provider": input.provider,
        "manual_step": manual_step,
        "voice_url": voice_url,
        "status_url": status_url,
    }))
}

/// US numbers in E.164. Accepts the common ways people write them.
pub fn normalize_phone(input: &str) -> Result<String> {
    let digits: String = input.chars().filter(char::is_ascii_digit).collect();
    let digits = match digits.len() {
        10 => digits,
        11 if digits.starts_with('1') => digits[1..].to_owned(),
        _ => bail!("Use a US number such as +1 512-555-0142"),
    };
    let number = format!("+1{digits}");
    us_number(&number).map_err(|e| anyhow!(e.0))?;
    Ok(number)
}

#[derive(Default)]
pub struct LeadFilter {
    pub since: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub qualification: Option<String>,
    pub include_demo: bool,
    pub limit: Option<usize>,
}

pub async fn list_leads(client: &Client, filter: &LeadFilter) -> Result<Vec<Lead>> {
    let mut leads = client.leads().await?;
    leads.retain(|lead| {
        (filter.include_demo || !lead.is_demo)
            && filter.since.is_none_or(|since| lead.created_at >= since)
            && filter
                .source
                .as_ref()
                .is_none_or(|s| lead.source.eq_ignore_ascii_case(s))
            && filter
                .qualification
                .as_ref()
                .is_none_or(|q| lead.qualification == *q)
    });
    leads.sort_by_key(|lead| std::cmp::Reverse(lead.created_at));
    if let Some(limit) = filter.limit {
        leads.truncate(limit);
    }
    Ok(leads)
}

/// Finds a lead by its full ID or a unique prefix of at least 6 characters.
pub async fn find_lead(client: &Client, id: &str) -> Result<Lead> {
    let id = id.trim().to_ascii_lowercase();
    if client.public_key()
        && let Ok(uuid) = id.parse::<Uuid>()
    {
        return Ok(client.lead(uuid).await?);
    }
    if id.len() < 6 {
        bail!("Use at least 6 characters of the lead ID");
    }
    let matches: Vec<Lead> = client
        .leads()
        .await?
        .into_iter()
        .filter(|lead| lead.id.to_string().starts_with(&id))
        .collect();
    match matches.len() {
        0 => bail!("No lead matches {id}"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => bail!("{n} leads start with {id}; use more of the ID"),
    }
}

pub async fn add_lead(client: &Client, mut lead: NewLead) -> Result<Uuid> {
    if lead.external_id.trim().is_empty() {
        lead.external_id = format!("cli-{}", Uuid::new_v4());
    }
    lead.validate().map_err(|e| anyhow!(e.0))?;
    Ok(client.add_lead(&lead).await?)
}

pub fn parse_qualification(value: &str) -> Result<Qualification> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "unreviewed" => Qualification::Unreviewed,
        "qualified" => Qualification::Qualified,
        "unqualified" => Qualification::Unqualified,
        "spam" => Qualification::Spam,
        _ => bail!("Use qualified, unqualified, spam or unreviewed"),
    })
}

/// Sets qualification using the lead's current version. Notes are kept unless replaced.
pub async fn qualify(
    client: &Client,
    id: &str,
    qualification: Qualification,
    notes: Option<String>,
) -> Result<Value> {
    let lead = find_lead(client, id).await?;
    let update = LeadUpdate {
        version: lead.version,
        qualification: qualification.clone(),
        notes: notes.unwrap_or(lead.notes),
    };
    client
        .update_lead(lead.id, &update)
        .await
        .map_err(|error| match error {
            crate::client::Error::Conflict(_) => {
                anyhow!(
                    "Someone changed this lead while it was being updated. Read it again and retry."
                )
            }
            other => other.into(),
        })?;
    Ok(
        json!({"lead": lead.id, "name": lead.name, "qualification": qualification.as_str(), "version": lead.version + 1}),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeStatus {
    Booked,
    Canceled,
}

/// Records or revises a booking. The reference (job or invoice number) identifies it; repeating
/// the same reference revises the existing outcome instead of adding a second one.
pub async fn record_outcome(
    client: &Client,
    id: &str,
    reference: &str,
    amount_minor: Option<i64>,
    status: OutcomeStatus,
) -> Result<Value> {
    let lead = find_lead(client, id).await?;
    let existing = client
        .outcomes(lead.id)
        .await?
        .into_iter()
        .find(|o| o.external_id == reference);
    let amount_minor = match (amount_minor, &existing, status) {
        (Some(amount), _, _) => amount,
        (None, Some(previous), _) => previous.amount_minor,
        (None, None, OutcomeStatus::Booked) => {
            bail!("Give the booked amount, for example --amount 1250.00")
        }
        (None, None, OutcomeStatus::Canceled) => {
            bail!("There is no booking {reference} on this lead to cancel")
        }
    };
    let input = OutcomeInput {
        external_id: reference.trim().to_owned(),
        amount_minor,
        currency: "USD".into(),
        status: if status == OutcomeStatus::Booked {
            "booked"
        } else {
            "canceled"
        }
        .into(),
        revision: existing.as_ref().map_or(1, |o| o.revision + 1),
    };
    input.validate().map_err(|e| anyhow!(e.0))?;
    client.put_outcome(lead.id, &input).await.map_err(|error| match error {
        crate::client::Error::Conflict(_) => anyhow!(
            "Reference {reference} is already used on a different lead, or the booking changed while it was being saved"
        ),
        other => other.into(),
    })?;
    Ok(json!({
        "lead": lead.id, "name": lead.name, "reference": input.external_id,
        "status": input.status, "amount": money(input.amount_minor), "revision": input.revision,
    }))
}

pub async fn outcomes(client: &Client, id: &str) -> Result<Value> {
    let lead = find_lead(client, id).await?;
    let outcomes: Vec<Value> = client
        .outcomes(lead.id)
        .await?
        .into_iter()
        .map(|o| json!({"reference": o.external_id, "status": o.status, "amount": money(o.amount_minor), "revision": o.revision}))
        .collect();
    Ok(json!({"lead": lead.id, "name": lead.name, "outcomes": outcomes}))
}

/// Parses "$1,250.00", "1250" or "1250.5" into cents without floating point.
pub fn parse_money(input: &str) -> Result<i64> {
    let cleaned: String = input
        .trim()
        .trim_start_matches('$')
        .chars()
        .filter(|c| *c != ',')
        .collect();
    let (dollars, cents) = cleaned.split_once('.').unwrap_or((&cleaned, ""));
    if dollars.is_empty() && cents.is_empty() || cents.len() > 2 {
        bail!("Use an amount like 1250 or 1250.00");
    }
    let dollars: i64 = if dollars.is_empty() {
        0
    } else {
        dollars.parse().context("Use an amount like 1250.00")?
    };
    let cents: i64 = format!("{cents:0<2}")
        .parse()
        .context("Use an amount like 1250.00")?;
    dollars
        .checked_mul(100)
        .and_then(|d| d.checked_add(cents))
        .ok_or_else(|| anyhow!("That amount is too large"))
}

pub fn money(minor: i64) -> String {
    format!("${}.{:02}", group(minor / 100), minor % 100)
}

fn group(value: i64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "7d", "24h", "30m" or a date such as 2026-10-01 (midnight UTC).
pub fn parse_since(input: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let input = input.trim();
    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
    }
    let (amount, unit) = input.split_at(input.len().saturating_sub(1));
    let amount: i64 = amount
        .parse()
        .map_err(|_| anyhow!("Use 7d, 24h, 30m or a date like 2026-10-01"))?;
    let span = match unit {
        "d" => Duration::days(amount),
        "h" => Duration::hours(amount),
        "m" => Duration::minutes(amount),
        _ => bail!("Use 7d, 24h, 30m or a date like 2026-10-01"),
    };
    Ok(now - span)
}

pub async fn source_report(client: &Client) -> Result<Value> {
    let rows: Vec<Value> = client
        .summary()
        .await?
        .into_iter()
        .map(|s| json!({"source": s.source, "leads": s.leads, "qualified": s.qualified, "bookings": s.bookings, "booked": money(s.booked_minor)}))
        .collect();
    Ok(json!({"sources": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_round_trips_without_floats() {
        assert_eq!(parse_money("$1,250.00").unwrap(), 125_000);
        assert_eq!(parse_money("1250.5").unwrap(), 125_050);
        assert_eq!(parse_money("0.07").unwrap(), 7);
        assert_eq!(parse_money("89").unwrap(), 8_900);
        assert!(parse_money("12.345").is_err());
        assert!(parse_money("abc").is_err());
        assert!(parse_money(".").is_err());
        assert_eq!(money(125_050), "$1,250.50");
        assert_eq!(money(7), "$0.07");
        assert_eq!(money(123_456_789), "$1,234,567.89");
    }

    #[test]
    fn phones_normalize_to_e164() {
        assert_eq!(normalize_phone("(512) 555-0142").unwrap(), "+15125550142");
        assert_eq!(normalize_phone("+1 512 555 0142").unwrap(), "+15125550142");
        assert_eq!(normalize_phone("15125550142").unwrap(), "+15125550142");
        assert!(normalize_phone("555-0142").is_err());
        assert!(normalize_phone("+1 112 555 0142").is_err());
    }

    #[test]
    fn since_accepts_spans_and_dates() {
        let now = "2026-10-22T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(parse_since("7d", now).unwrap(), now - Duration::days(7));
        assert_eq!(parse_since("24h", now).unwrap(), now - Duration::hours(24));
        assert_eq!(
            parse_since("2026-10-01", now).unwrap().to_rfc3339(),
            "2026-10-01T00:00:00+00:00"
        );
        assert!(parse_since("soon", now).is_err());
    }

    #[test]
    fn qualification_names_match_the_api() {
        assert_eq!(parse_qualification("SPAM").unwrap(), Qualification::Spam);
        assert!(parse_qualification("maybe").is_err());
    }
}
