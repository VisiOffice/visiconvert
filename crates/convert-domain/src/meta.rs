//! Meta Conversions API customer-information rules: normalization, hashing and click and
//! browser identifiers, shared by the planner (which hashes before anything is stored) and
//! the provider adapter.
use sha2::{Digest, Sha256};

/// Meta's standard event names. Anything else is sent as a custom event.
pub const STANDARD_EVENTS: &[&str] = &[
    "AddPaymentInfo",
    "AddToCart",
    "AddToWishlist",
    "CompleteRegistration",
    "Contact",
    "CustomizeProduct",
    "Donate",
    "FindLocation",
    "InitiateCheckout",
    "Lead",
    "Purchase",
    "Schedule",
    "Search",
    "StartTrial",
    "SubmitApplication",
    "Subscribe",
    "ViewContent",
];

/// A standard event name, or a custom one: a letter, then up to 39 letters, digits or
/// underscores.
pub fn event_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.len() <= 40
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
pub fn standard_event(s: &str) -> bool {
    STANDARD_EVENTS.contains(&s)
}
pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
/// Trimmed and lowercased, or None if it is not an email address.
pub fn normalize_email(s: &str) -> Option<String> {
    let e = s.trim().to_lowercase();
    let (local, domain) = e.split_once('@')?;
    (!local.is_empty() && domain.contains('.') && !e.chars().any(char::is_whitespace)).then_some(e)
}
/// Digits with the country code (US numbers get a leading 1), or None.
pub fn normalize_phone(s: &str) -> Option<String> {
    if s.contains('@') {
        return None;
    }
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_start_matches('0').to_string();
    match digits.len() {
        10 => Some(format!("1{digits}")),
        11..=15 => Some(digits),
        _ => None,
    }
}
/// The `fbc` value for a click ID first seen at `seen_ms` (Unix milliseconds).
pub fn fbc(fbclid: &str, seen_ms: i64) -> Option<String> {
    (!fbclid.is_empty()
        && fbclid.len() <= 500
        && fbclid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| format!("fb.1.{seen_ms}.{fbclid}"))
}
/// A browser-stored `_fbp` or `_fbc` cookie value, accepted only in Meta's format.
pub fn cookie_value(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(4, '.').collect();
    s.len() <= 600
        && parts.len() == 4
        && parts[0] == "fb"
        && parts[1].bytes().all(|b| b.is_ascii_digit())
        && parts[2].bytes().all(|b| b.is_ascii_digit())
        && !parts[3].is_empty()
        && parts[3]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Area codes in California, used to decide Limited Data Use for calls without an IP.
const CALIFORNIA: &[&str] = &[
    "209", "213", "279", "310", "323", "341", "350", "369", "408", "415", "424", "442", "510",
    "530", "559", "562", "619", "626", "628", "650", "657", "661", "669", "707", "714", "747",
    "760", "805", "818", "820", "831", "840", "858", "909", "916", "925", "949", "951",
];
/// Whether a normalized US phone (11 digits starting with 1) has a California area code.
pub fn california_phone(normalized: &str) -> bool {
    normalized.len() == 11 && normalized.starts_with('1') && CALIFORNIA.contains(&&normalized[1..4])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_matches_meta_examples() {
        assert_eq!(
            normalize_email(" John_Smith@gmail.com ").as_deref(),
            Some("john_smith@gmail.com")
        );
        assert_eq!(normalize_email("nope"), None);
        assert_eq!(
            normalize_phone("(650)555-1212").as_deref(),
            Some("16505551212")
        );
        assert_eq!(
            normalize_phone("+1 602-555-0100").as_deref(),
            Some("16025550100")
        );
        assert_eq!(normalize_phone("555-0100"), None);
        assert_eq!(normalize_phone("a@b.co"), None);
        assert_eq!(
            sha256_hex("john_smith@gmail.com"),
            "62a14e44f765419d10fea99367361a727c12365e2520f32218d505ed9aa0f62f"
        );
    }
    #[test]
    fn click_and_cookie_values() {
        assert_eq!(
            fbc("AbCdEf_12-3", 1554763741205).as_deref(),
            Some("fb.1.1554763741205.AbCdEf_12-3")
        );
        assert_eq!(fbc("bad value", 1), None);
        assert!(cookie_value("fb.1.1596403881668.1116446470"));
        assert!(cookie_value("fb.1.1554763741205.AbCdEf"));
        assert!(!cookie_value("fb.1.x.1"));
        assert!(!cookie_value("fb.1.1"));
        assert!(!cookie_value("fb.1.1.<script>"));
    }
    #[test]
    fn names_and_california() {
        assert!(event_name("Lead") && event_name("QuoteSent") && event_name("quote_sent_2"));
        assert!(!event_name("2x") && !event_name("has space") && !event_name(&"a".repeat(41)));
        assert!(standard_event("Purchase") && !standard_event("QuoteSent"));
        assert!(california_phone("12135550100") && !california_phone("16025550100"));
    }
}
