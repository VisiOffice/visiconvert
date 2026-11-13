//! Web chat leads posted by a chat system (POST /api/v1/chat-leads).
//!
//! A chat is linked to a website visit like a form submission, so pipeline delivery,
//! `new_lead` actions and the activity views treat it exactly as they treat forms. The
//! visit is the one the tracker already recorded with the same click ID when there is
//! one; otherwise a visit is built from the chat's landing URL. Posting the same
//! external ID again only adds or replaces the visitor's name, phone and email.
use super::*;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// Click IDs that identify one ad click. gbraid and wbraid are shared by many visitors
/// (they identify a campaign context, not a click), so they never select a visit.
const UNIQUE_CLICKS: [&str; 3] = ["gclid", "msclkid", "fbclid"];
const ALL_CLICKS: [&str; 5] = ["gclid", "gbraid", "wbraid", "msclkid", "fbclid"];

pub struct ChatLead {
    pub external_id: String,
    pub started_at: DateTime<Utc>,
    pub name: Option<String>,
    /// The last 10 digits.
    pub phone: Option<String>,
    /// Trimmed and lowercased.
    pub email: Option<String>,
}

fn display(email: &Option<String>, phone: &Option<String>) -> String {
    email
        .clone()
        .or_else(|| phone.as_ref().map(|p| format!("+1{p}")))
        .unwrap_or_else(|| "Not supplied".into())
}

/// The other spelling of an origin: `https://www.example.com` ⇄ `https://example.com`.
fn www_twin(origin: &str) -> Option<String> {
    let (scheme, host) = origin.split_once("://")?;
    Some(match host.strip_prefix("www.") {
        Some(bare) => format!("{scheme}://{bare}"),
        None => format!("{scheme}://www.{host}"),
    })
}

impl Store {
    /// The enabled site serving `origin`, treating `www.` and the bare host as one site.
    pub async fn chat_site(&self, origin: &str) -> Result<Option<(Uuid, String)>> {
        let candidates: Vec<String> = std::iter::once(origin.to_string())
            .chain(www_twin(origin))
            .collect();
        Ok(sqlx::query_as("SELECT id,origin FROM sites WHERE workspace_id=$1 AND enabled AND origin=ANY($2) ORDER BY origin=$3 DESC LIMIT 1")
            .bind(self.workspace).bind(&candidates).bind(origin)
            .fetch_optional(&self.pool).await?)
    }

    /// Record or update one chat lead. `evidence` is the visit evidence built from the
    /// chat's landing URL, as the tracker would have built it.
    pub async fn capture_chat(
        &self,
        site: Uuid,
        evidence: &Value,
        chat: &ChatLead,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("chat-lead:{}:{}", self.workspace, chat.external_id))
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query("SELECT id,name,match_phone,match_email,submission_payload FROM leads WHERE workspace_id=$1 AND channel='chat' AND external_id=$2 FOR UPDATE")
            .bind(self.workspace).bind(&chat.external_id).fetch_optional(&mut *tx).await?;
        if let Some(row) = existing {
            let id: Uuid = row.get("id");
            let old_name: String = row.get("name");
            let old_phone: Option<String> = row.get("match_phone");
            let old_email: Option<String> = row.get("match_email");
            let name = chat.name.clone().unwrap_or_else(|| old_name.clone());
            let phone = chat.phone.clone().or(old_phone.clone());
            let email = chat.email.clone().or(old_email.clone());
            if name != old_name || phone != old_phone || email != old_email {
                let mut payload: Value = row.get("submission_payload");
                payload["fields"] = fields(&name, &phone, &email);
                sqlx::query("UPDATE leads SET name=$3,contact=$4,match_phone=$5,match_email=$6,submission_payload=$7 WHERE workspace_id=$1 AND id=$2")
                    .bind(self.workspace).bind(id).bind(&name).bind(display(&email, &phone))
                    .bind(&phone).bind(&email).bind(&payload).execute(&mut *tx).await?;
                sqlx::query("UPDATE website_submissions SET payload=$3 WHERE workspace_id=$1 AND lead_id=$2")
                    .bind(self.workspace).bind(id).bind(&payload).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) VALUES($1,$2,'chat_lead_updated',$3)")
                    .bind(self.workspace).bind(id)
                    .bind(json!({"phone":phone.is_some(),"email":email.is_some()}))
                    .execute(&mut *tx).await?;
            }
            let click: bool = sqlx::query_scalar("SELECT COALESCE(bool_or(s.evidence ?| $3),false) FROM website_submissions f JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id WHERE f.workspace_id=$1 AND f.lead_id=$2")
                .bind(self.workspace).bind(id).bind(ALL_CLICKS.map(String::from).to_vec())
                .fetch_one(&mut *tx).await?;
            tx.commit().await?;
            return Ok(json!({"lead_id":id,"created":false,"click":click}));
        }

        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sites WHERE workspace_id=$1 AND id=$2 AND enabled)",
        )
        .bind(self.workspace)
        .bind(site)
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            return Err(Error::NotFound);
        }
        let click = |k: &str| evidence[k].as_str().map(str::to_string);
        let mut visit: Option<(Uuid, DateTime<Utc>)> = None;
        if UNIQUE_CLICKS.iter().any(|k| click(k).is_some()) {
            // The tracker's own visit for this click, if it recorded one around the chat.
            visit = sqlx::query_as("SELECT id,created_at FROM website_sessions WHERE site_id=$1 AND created_at <= $2::timestamptz + interval '5 minutes' AND created_at > $2::timestamptz - interval '90 days' AND (($3::text IS NOT NULL AND evidence->>'gclid'=$3) OR ($4::text IS NOT NULL AND evidence->>'msclkid'=$4) OR ($5::text IS NOT NULL AND evidence->>'fbclid'=$5)) ORDER BY created_at DESC LIMIT 1")
                .bind(site).bind(chat.started_at).bind(click("gclid")).bind(click("msclkid")).bind(click("fbclid"))
                .fetch_optional(&mut *tx).await?;
        }
        let matched = visit.is_some();
        let (session, visit_at) = match visit {
            Some(v) => v,
            None => {
                let id = Uuid::new_v4();
                let mut e = evidence.clone();
                e["captured_by"] = json!("chat_api");
                sqlx::query("INSERT INTO website_sessions(site_id,id,evidence,created_at) VALUES($1,$2,$3,$4)")
                    .bind(site).bind(id).bind(&e).bind(chat.started_at).execute(&mut *tx).await?;
                (id, chat.started_at)
            }
        };
        // A conversion can't precede its click; tolerate small clock differences.
        let at = chat.started_at.max(visit_at);
        let name = chat.name.clone().unwrap_or_else(|| "Web chat".into());
        let payload = json!({"session_id":session,"chat":true,"fields":fields(&name, &chat.phone, &chat.email)});
        let lead = Uuid::new_v4();
        sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,submission_payload,created_at,match_phone,match_email) VALUES($1,$2,$3,$4,$5,'chat',$6,$7,$8,$9,$10,$11)")
            .bind(lead).bind(self.workspace).bind(&chat.external_id).bind(&name)
            .bind(display(&chat.email, &chat.phone))
            .bind(evidence["source"].as_str().unwrap_or("Unknown"))
            .bind(evidence["utm_campaign"].as_str().unwrap_or(""))
            .bind(&payload).bind(at).bind(&chat.phone).bind(&chat.email)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO website_submissions(site_id,external_id,session_id,workspace_id,lead_id,payload,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(site).bind(format!("chat:{lead}")).bind(session).bind(self.workspace).bind(lead).bind(&payload).bind(at)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) VALUES($1,$2,'chat_lead_captured',$3)")
            .bind(self.workspace).bind(lead)
            .bind(json!({"site_id":site,"session_id":session,"visit":if matched {"matched"} else {"created"}}))
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(
            json!({"lead_id":lead,"created":true,"click":ALL_CLICKS.iter().any(|k| click(k).is_some()),
            "visit":if matched {"matched"} else {"created"}}),
        )
    }
}

fn fields(name: &str, phone: &Option<String>, email: &Option<String>) -> Value {
    let mut f = json!({"name":name});
    if let Some(p) = phone {
        f["phone"] = json!(format!("+1{p}"));
    }
    if let Some(e) = email {
        f["email"] = json!(e);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn www_twins_and_display() {
        assert_eq!(www_twin("https://www.a.com").unwrap(), "https://a.com");
        assert_eq!(www_twin("https://a.com").unwrap(), "https://www.a.com");
        assert_eq!(display(&None, &Some("6025550100".into())), "+16025550100");
        assert_eq!(
            display(&Some("a@b.co".into()), &Some("6025550100".into())),
            "a@b.co"
        );
        assert_eq!(display(&None, &None), "Not supplied");
    }
}
