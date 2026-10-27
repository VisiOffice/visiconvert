//! Reporting hints from browser-supplied evidence, never verified ad attribution.
use serde::Serialize;

#[derive(Default)]
pub struct SourceContext<'a> {
    pub utm_source: Option<&'a str>,
    pub utm_medium: Option<&'a str>,
    pub gclid: Option<&'a str>,
    pub gbraid: Option<&'a str>,
    pub wbraid: Option<&'a str>,
    pub msclkid: Option<&'a str>,
    pub fbclid: Option<&'a str>,
    pub referrer: Option<&'a str>,
    pub site_origin: &'a str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SourceClassification {
    pub source_model: &'static str,
    pub source: String,
    pub medium: Option<String>,
    pub source_basis: &'static str,
    pub click_sources: Option<String>,
    pub source_warning: Option<String>,
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn provider(source: &str) -> &str {
    match source {
        "google" | "google_ads" | "google ads" | "adwords" => "google",
        "bing" | "bing_ads" | "microsoft" | "microsoft_ads" => "microsoft",
        "facebook" | "facebook_ads" | "fb" | "meta" | "instagram" => "facebook",
        _ => source,
    }
}

/// Accept only the named domain or one of its subdomains, not substring matches.
fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn referrer_source(context: &SourceContext<'_>) -> Option<(String, &'static str)> {
    let url = url::Url::parse(context.referrer?).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.origin().ascii_serialization() == context.site_origin
    {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.');
    // Deliberately finite: unknown regional/search hosts remain referrals.
    const SEARCH: &[(&str, &str)] = &[
        ("google.com", "google"),
        ("google.co.uk", "google"),
        ("google.ca", "google"),
        ("google.com.au", "google"),
        ("google.de", "google"),
        ("google.fr", "google"),
        ("bing.com", "bing"),
        ("duckduckgo.com", "duckduckgo"),
        ("search.yahoo.com", "yahoo"),
    ];
    const SOCIAL: &[(&str, &str)] = &[
        ("facebook.com", "facebook"),
        ("instagram.com", "instagram"),
        ("linkedin.com", "linkedin"),
        ("twitter.com", "twitter"),
        ("x.com", "twitter"),
        ("t.co", "twitter"),
        ("youtube.com", "youtube"),
        ("pinterest.com", "pinterest"),
    ];
    for (domains, medium) in [(SEARCH, "organic"), (SOCIAL, "social")] {
        if let Some((_, source)) = domains
            .iter()
            .find(|(domain, _)| domain_matches(host, domain))
        {
            return Some(((*source).into(), medium));
        }
    }
    Some((url.origin().ascii_serialization(), "referral"))
}

pub fn classify_source(context: &SourceContext<'_>) -> SourceClassification {
    let mut clicks = Vec::new();
    if [context.gclid, context.gbraid, context.wbraid]
        .into_iter()
        .any(|v| nonempty(v).is_some())
    {
        clicks.push(("google", "google_ads", Some("cpc")));
    }
    if nonempty(context.msclkid).is_some() {
        clicks.push(("microsoft", "microsoft_ads", Some("cpc")));
    }
    if nonempty(context.fbclid).is_some() {
        // A Facebook click identifier alone does not establish paid traffic.
        clicks.push(("facebook", "facebook", None));
    }
    let utm = nonempty(context.utm_source);
    let medium = nonempty(context.utm_medium);
    let mut warnings = Vec::new();
    if clicks.len() > 1 {
        warnings.push("Multiple click-ID providers were supplied; no provider was preferred.");
    }
    if let Some(source) = utm {
        let normalized = source.to_lowercase();
        if clicks.iter().any(|(p, _, _)| *p != provider(&normalized)) {
            warnings.push(
                "UTM source differs from click-ID evidence; the declared UTM source was kept.",
            );
        }
    }
    if medium.is_some_and(|m| {
        matches!(
            m.to_lowercase().as_str(),
            "organic" | "social" | "email" | "referral" | "none" | "direct"
        )
    }) && clicks.iter().any(|(_, _, medium)| medium.is_some())
    {
        warnings
            .push("UTM medium differs from paid-click evidence; the declared UTM medium was kept.");
    }
    let (source, inferred_medium, basis) = if let Some(source) = utm {
        // Never silently infer a medium for an explicitly declared source.
        (source.to_owned(), None, "utm")
    } else if clicks.len() == 1 {
        (clicks[0].1.into(), clicks[0].2, "click_id")
    } else if clicks.len() > 1 {
        (
            "Unknown / conflicting click IDs".into(),
            None,
            "conflicting_click_ids",
        )
    } else if let Some((source, medium)) = referrer_source(context) {
        (source, Some(medium), "referrer")
    } else {
        ("Direct / unknown".into(), None, "unknown")
    };
    SourceClassification {
        source_model: "source-classification-v1",
        source,
        medium: medium.or(inferred_medium).map(str::to_owned),
        source_basis: basis,
        click_sources: (!clicks.is_empty())
            .then(|| clicks.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")),
        source_warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_campaign_wins_without_discarding_conflict() {
        let c = classify_source(&SourceContext {
            utm_source: Some(" Newsletter "),
            utm_medium: Some("email"),
            gclid: Some("click"),
            ..Default::default()
        });
        assert_eq!(c.source, "Newsletter");
        assert_eq!(c.medium.as_deref(), Some("email"));
        assert_eq!(c.source_basis, "utm");
        assert_eq!(c.click_sources.as_deref(), Some("google"));
        assert!(c.source_warning.unwrap().contains("differs"));
    }

    #[test]
    fn google_braids_share_one_provider_and_preserve_utm_aliases() {
        let c = classify_source(&SourceContext {
            utm_source: Some("Google"),
            gclid: Some("a"),
            gbraid: Some("b"),
            wbraid: Some("c"),
            ..Default::default()
        });
        assert_eq!(c.source, "Google");
        assert_eq!(c.medium, None);
        assert_eq!(c.source_warning, None);
        for field in ["gclid", "gbraid", "wbraid"] {
            let mut ctx = SourceContext::default();
            match field {
                "gclid" => ctx.gclid = Some("a"),
                "gbraid" => ctx.gbraid = Some("a"),
                _ => ctx.wbraid = Some("a"),
            }
            let c = classify_source(&ctx);
            assert_eq!(c.source, "google_ads");
            assert_eq!(c.medium.as_deref(), Some("cpc"));
        }
    }

    #[test]
    fn microsoft_click_and_facebook_click_have_different_medium_evidence() {
        let c = classify_source(&SourceContext {
            msclkid: Some("a"),
            ..Default::default()
        });
        assert_eq!(c.source, "microsoft_ads");
        assert_eq!(c.medium.as_deref(), Some("cpc"));
        let c = classify_source(&SourceContext {
            fbclid: Some("a"),
            ..Default::default()
        });
        assert_eq!(c.source, "facebook");
        assert_eq!(c.medium, None);
    }

    #[test]
    fn conflicting_providers_never_get_an_arbitrary_winner() {
        let c = classify_source(&SourceContext {
            gclid: Some("a"),
            msclkid: Some("b"),
            referrer: Some("https://google.com"),
            ..Default::default()
        });
        assert_eq!(c.source_basis, "conflicting_click_ids");
        assert_eq!(c.medium, None);
        assert_eq!(c.click_sources.as_deref(), Some("google, microsoft"));
    }

    #[test]
    fn empty_values_are_not_signals_and_missing_referrer_is_not_proven_direct() {
        let c = classify_source(&SourceContext {
            utm_source: Some("  "),
            gclid: Some("\t"),
            ..Default::default()
        });
        assert_eq!(c.source, "Direct / unknown");
        assert_eq!(c.source_basis, "unknown");
        assert_eq!(c.medium, None);
    }

    #[test]
    fn referrer_domains_match_boundaries_and_keep_unknown_origins() {
        for url in [
            "https://notgoogle.com",
            "https://google.com.evil.test",
            "https://google.example.com",
            "https://notx.com",
            "https://facebook.com.evil.test",
        ] {
            let c = classify_source(&SourceContext {
                referrer: Some(url),
                ..Default::default()
            });
            assert_eq!(c.source, url);
            assert_eq!(c.medium.as_deref(), Some("referral"));
        }
        for (url, source, medium) in [
            (
                "https://WWW.Google.Co.Uk/search?q=private",
                "google",
                "organic",
            ),
            ("https://l.facebook.com/redirect", "facebook", "social"),
            ("https://www.bing.com/", "bing", "organic"),
            ("https://www.google.com./", "google", "organic"),
        ] {
            let c = classify_source(&SourceContext {
                referrer: Some(url),
                ..Default::default()
            });
            assert_eq!(c.source, source);
            assert_eq!(c.medium.as_deref(), Some(medium));
        }
    }

    #[test]
    fn self_referrals_invalid_schemes_and_credentials_are_ignored() {
        for url in [
            "https://shop.example/thanks",
            "not a url",
            "ftp://google.com",
            "https://user:password@google.com",
            "https://google.com@evil.test",
        ] {
            let c = classify_source(&SourceContext {
                referrer: Some(url),
                site_origin: "https://shop.example",
                ..Default::default()
            });
            assert_eq!(c.source_basis, "unknown");
        }
    }

    #[test]
    fn click_signal_precedes_referrer_and_explicit_medium_is_preserved() {
        let c = classify_source(&SourceContext {
            gclid: Some("a"),
            utm_medium: Some("organic"),
            referrer: Some("https://bing.com"),
            ..Default::default()
        });
        assert_eq!(c.source, "google_ads");
        assert_eq!(c.medium.as_deref(), Some("organic"));
        assert!(c.source_warning.is_some());
    }
}
