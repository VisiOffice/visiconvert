//! Reviewed, durable conversion preparation. This module never calls Google.
use crate::{Error, Result, Store};
use chrono::{DateTime, Duration, Utc};
use convert_domain::ValidationError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub customer_id: String,
    pub qualified_action_id: String,
    pub booking_action_id: String,
    pub allow_inferred_calls: bool,
    pub version: i32,
}
impl Settings {
    fn validate(&self) -> Result<()> {
        let digits = |s: &str, min, max| {
            (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
        };
        if !digits(&self.customer_id, 10, 10)
            || (!self.qualified_action_id.is_empty() && !digits(&self.qualified_action_id, 1, 20))
            || (!self.booking_action_id.is_empty() && !digits(&self.booking_action_id, 1, 20))
            || (self.qualified_action_id.is_empty() && self.booking_action_id.is_empty())
            || (!self.qualified_action_id.is_empty()
                && self.qualified_action_id == self.booking_action_id)
            || self.version < 0
        {
            return Err(ValidationError("Use a 10-digit Google customer ID and distinct numeric conversion action IDs (at least one)").into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    #[default]
    Unknown,
    Granted,
    Denied,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub ad_user_data: Decision,
    pub ad_personalization: Decision,
    pub effective_at: Option<DateTime<Utc>>,
    pub reference: String,
    pub version: i32,
}
impl Consent {
    fn validate(&self) -> Result<()> {
        if self.version < 0
            || self.reference.chars().count() > 300
            || self.effective_at.is_some_and(|t| t > Utc::now())
            || ((self.ad_user_data != Decision::Unknown
                || self.ad_personalization != Decision::Unknown)
                && (self.reference.trim().is_empty() || self.effective_at.is_none()))
        {
            return Err(ValidationError("Record the existing consent decision's time and evidence reference; future times are not allowed").into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageInput {
    pub kind: String,
    pub entity_key: String,
    pub fingerprint: String,
    pub version: i32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub version: i32,
}

fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
fn transaction_id(workspace: Uuid, kind: &str, key: &str) -> String {
    format!("vc_{}", &digest(&json!([workspace, kind, key]))[..48])
}
pub(crate) async fn settings(conn: &mut PgConnection, workspace: Uuid) -> Result<Settings> {
    let row =
        sqlx::query("SELECT configuration,version FROM conversion_settings WHERE workspace_id=$1")
            .bind(workspace)
            .fetch_optional(conn)
            .await?;
    match row {
        None => Ok(Settings::default()),
        Some(r) => {
            let mut value: Settings = serde_json::from_value(r.get("configuration"))
                .map_err(|_| ValidationError("Invalid saved conversion settings"))?;
            value.version = r.get("version");
            Ok(value)
        }
    }
}
async fn record(
    conn: &mut PgConnection,
    workspace: Uuid,
    kind: &str,
    actor: &str,
    payload: Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,$2,$3,$4)",
    )
    .bind(workspace)
    .bind(kind)
    .bind(actor)
    .bind(payload)
    .execute(conn)
    .await?;
    Ok(())
}

// Event timestamps come from recorded transitions, never from preview/staging
// time. Older imported records without the necessary history remain blocked.
pub(crate) const CANDIDATES: &str = r#"
WITH candidates AS (
 SELECT 'qualified'::text AS kind,i.id::text AS entity_key,i.id AS inquiry_id,
 i.primary_lead_id AS lead_id,i.qualification,i.archived_at,
 NULL::bigint AS amount_minor,NULL::text AS booking_status,NULL::integer AS booking_revision,
 (SELECT min(e.created_at) FROM inquiry_events e WHERE e.workspace_id=i.workspace_id AND e.inquiry_id=i.id
   AND e.kind='review_updated' AND e.payload->>'qualification'='qualified') AS event_at
 FROM inquiries i WHERE i.workspace_id=$1 AND i.qualification='qualified'
 UNION ALL
 SELECT 'booking',o.external_id,i.id,o.lead_id,i.qualification,i.archived_at,o.amount_minor,o.status,o.revision,
 (SELECT min(e.created_at) FROM audit_events e WHERE e.workspace_id=o.workspace_id AND e.kind='outcome_updated'
   AND e.payload->>'external_id'=o.external_id AND e.payload->>'revision'='1')
 FROM outcomes o JOIN leads l ON l.workspace_id=o.workspace_id AND l.id=o.lead_id
 JOIN inquiries i ON i.workspace_id=l.workspace_id AND i.id=l.inquiry_id WHERE o.workspace_id=$1
)
SELECT c.*,l.channel,l.is_demo,l.source,l.name,
 COALESCE(ws.evidence,ds.evidence,'{}'::jsonb) AS evidence,
 COALESCE(ws.created_at,ds.created_at) AS visit_at,
 cc.evidence AS consent,cc.version AS consent_version,
 q.id AS job_id,q.version AS job_version,q.state AS job_state
FROM candidates c JOIN leads l ON l.workspace_id=$1 AND l.id=c.lead_id
LEFT JOIN LATERAL (SELECT s.evidence,s.created_at FROM website_submissions f
 JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id
 WHERE f.workspace_id=$1 AND f.lead_id=l.id ORDER BY f.created_at LIMIT 1) ws ON true
LEFT JOIN dni_call_evidence dc ON dc.workspace_id=$1 AND dc.lead_id=l.id AND dc.status='inferred_window'
LEFT JOIN dni_assignments da ON da.id=dc.assignment_id AND da.workspace_id=$1
LEFT JOIN website_sessions ds ON ds.site_id=da.site_id AND ds.id=da.session_id
LEFT JOIN conversion_consents cc ON cc.workspace_id=$1 AND cc.lead_id=l.id
LEFT JOIN conversion_outbox q ON q.workspace_id=$1 AND q.kind=c.kind AND q.entity_key=c.entity_key
WHERE ($2::text IS NULL OR (c.kind=$2 AND c.entity_key=$3))
ORDER BY c.event_at DESC NULLS LAST,c.kind,c.entity_key LIMIT 200
"#;
pub(crate) fn candidate(
    row: &sqlx::postgres::PgRow,
    config: &Settings,
    workspace: Uuid,
    now: DateTime<Utc>,
) -> Result<Value> {
    let kind: String = row.get("kind");
    let key: String = row.get("entity_key");
    let channel: String = row.get("channel");
    let event_at: Option<DateTime<Utc>> = row.get("event_at");
    let visit_at: Option<DateTime<Utc>> = row.get("visit_at");
    let evidence: Value = row.get("evidence");
    let mut consent: Consent = row
        .get::<Option<Value>, _>("consent")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ValidationError("Invalid saved consent evidence"))?
        .unwrap_or_default();
    consent.version = row.get::<Option<i32>, _>("consent_version").unwrap_or(0);
    // Explicit whitelist: no contact information, arbitrary form fields, or URLs.
    let mut identifiers = serde_json::Map::new();
    for key in ["gclid", "gbraid", "wbraid"] {
        if let Some(value) = evidence
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            identifiers.insert(key.into(), json!(value));
        }
    }
    let action = if kind == "qualified" {
        &config.qualified_action_id
    } else {
        &config.booking_action_id
    };
    let mut reasons = vec![];
    if config.version == 0 {
        reasons.push("Save a Google Ads destination first");
    }
    if action.is_empty() {
        reasons.push("No conversion action mapped for this event type");
    }
    if row.get::<bool, _>("is_demo") {
        reasons.push("Demo data cannot be staged");
    }
    if row.get::<Option<DateTime<Utc>>, _>("archived_at").is_some() {
        reasons.push("Inquiry was combined into another inquiry");
    }
    if ["spam", "unqualified"].contains(&row.get::<&str, _>("qualification")) {
        reasons.push("Inquiry is spam or unqualified");
    }
    if row.get::<Option<&str>, _>("booking_status") == Some("canceled") {
        reasons.push("Booking was canceled");
    }
    if event_at.is_none() {
        reasons.push("Original event time is missing from recorded history");
    }
    if event_at.is_some_and(|t| t > now) {
        reasons.push("Event time is in the future");
    }
    if visit_at.is_none() {
        reasons.push("No captured website visit is linked to this interaction");
    }
    if identifiers.is_empty() {
        reasons.push("No Google click identifier was captured");
    }
    if let (Some(event), Some(visit)) = (event_at, visit_at) {
        if event < visit {
            reasons.push("Event predates the captured visit");
        }
        // Conservative preparation policy only. Actual action windows and Google
        // eligibility must be validated when the provider adapter is connected.
        if event - visit > Duration::days(30) || now - event > Duration::days(30) {
            reasons.push("Outside the 30-day preparation window");
        }
    }
    if consent.ad_user_data != Decision::Granted {
        reasons.push("Advertising user-data consent is not recorded as granted");
    }
    if consent.ad_personalization == Decision::Unknown {
        reasons.push("Advertising personalization decision is unknown");
    }
    if consent.effective_at.is_none()
        || event_at
            .zip(consent.effective_at)
            .is_some_and(|(e, c)| c > e)
    {
        reasons.push("Consent evidence must be effective at the event time");
    }
    if channel == "call" && !config.allow_inferred_calls {
        reasons.push("Inferred call attribution requires explicit opt-in");
    }
    let snapshot = json!({
        "kind":kind,"entity_key":key,"inquiry_id":row.get::<Uuid,_>("inquiry_id"),
        "lead_id":row.get::<Uuid,_>("lead_id"),"channel":channel,"source":row.get::<String,_>("source"),
        "event_at":event_at,"visit_at":visit_at,"qualification":row.get::<String,_>("qualification"),
        "amount_minor":row.get::<Option<i64>,_>("amount_minor"),"currency":"USD",
        "booking_revision":row.get::<Option<i32>,_>("booking_revision"),
        "booking_status":row.get::<Option<String>,_>("booking_status"),
        "click_identifiers":identifiers,"consent":consent,"settings":config,
        "transaction_id":transaction_id(workspace,&kind,&key),
        "confidence":if channel == "call" {"inferred"} else {"browser-reported"}
    });
    Ok(
        json!({"kind":kind,"entity_key":key,"snapshot":snapshot,"fingerprint":digest(&snapshot),
        "display_name":row.get::<String,_>("name"),"eligible":reasons.is_empty(),"reasons":reasons,
        "job_id":row.get::<Option<Uuid>,_>("job_id"),"job_version":row.get::<Option<i32>,_>("job_version").unwrap_or(0),
        "job_state":row.get::<Option<String>,_>("job_state")}),
    )
}
impl Store {
    pub async fn google_conversion_settings(&self) -> Result<Settings> {
        let mut conn = self.pool.acquire().await?;
        settings(&mut conn, self.workspace).await
    }
    pub async fn conversion_preview(&self) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let config = settings(&mut tx, self.workspace).await?;
        let rows = sqlx::query(CANDIDATES)
            .bind(self.workspace)
            .bind(None::<&str>)
            .bind(None::<&str>)
            .fetch_all(&mut *tx)
            .await?;
        let now = Utc::now();
        let candidates = rows
            .iter()
            .map(|r| candidate(r, &config, self.workspace, now))
            .collect::<Result<Vec<_>>>()?;
        let jobs: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q) - 'workspace_id' FROM conversion_outbox q WHERE workspace_id=$1 ORDER BY updated_at DESC,id LIMIT 200")
            .bind(self.workspace).fetch_all(&mut *tx).await?;
        let history: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(e) - 'workspace_id' FROM conversion_events e WHERE workspace_id=$1 ORDER BY id DESC LIMIT 100")
            .bind(self.workspace).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(
            json!({"connected":false,"settings":config,"candidates":candidates,"jobs":jobs,"history":history,"limit":200}),
        )
    }
    pub async fn save_conversion_settings(&self, input: &Settings, actor: &str) -> Result<()> {
        self.save_conversion_settings_checked(input, actor, None)
            .await
    }
    pub async fn save_conversion_settings_checked(
        &self,
        input: &Settings,
        actor: &str,
        google_revision: Option<i64>,
    ) -> Result<()> {
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        if let Some(revision) = google_revision {
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM google_connections WHERE workspace_id=$1 AND revision=$2 AND encrypted_token IS NOT NULL)")
                .bind(self.workspace).bind(revision).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(Error::Conflict);
            }
        }
        let current = settings(&mut tx, self.workspace).await?;
        if input.version != current.version {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_settings(workspace_id,configuration,version) VALUES($1,$2,1) ON CONFLICT(workspace_id) DO UPDATE SET configuration=$2,version=conversion_settings.version+1")
            .bind(self.workspace).bind(json!(input)).execute(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "settings_saved",
            actor,
            json!({"version":current.version+1}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn save_conversion_consent(
        &self,
        lead: Uuid,
        input: &Consent,
        actor: &str,
    ) -> Result<()> {
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM leads WHERE workspace_id=$1 AND id=$2)",
        )
        .bind(self.workspace)
        .bind(lead)
        .fetch_one(&mut *tx)
        .await?;
        if !found {
            return Err(Error::NotFound);
        }
        let version: Option<i32> = sqlx::query_scalar(
            "SELECT version FROM conversion_consents WHERE workspace_id=$1 AND lead_id=$2",
        )
        .bind(self.workspace)
        .bind(lead)
        .fetch_optional(&mut *tx)
        .await?;
        if version.unwrap_or(0) != input.version {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_consents(workspace_id,lead_id,evidence,version) VALUES($1,$2,$3,1) ON CONFLICT(workspace_id,lead_id) DO UPDATE SET evidence=$3,version=conversion_consents.version+1")
            .bind(self.workspace).bind(lead).bind(json!(input)).execute(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "consent_recorded",
            actor,
            json!({"lead_id":lead,"evidence":input,"version":input.version+1}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn stage_conversion(&self, input: &StageInput, actor: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let config = settings(&mut tx, self.workspace).await?;
        let row = sqlx::query(CANDIDATES)
            .bind(self.workspace)
            .bind(&input.kind)
            .bind(&input.entity_key)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let preview = candidate(&row, &config, self.workspace, Utc::now())?;
        if preview["fingerprint"] != input.fingerprint {
            return Err(Error::Conflict);
        }
        if preview["eligible"] != true {
            return Err(ValidationError(
                "This event is not eligible for preparation; refresh its preview",
            )
            .into());
        }
        // Retry of the exact already-staged snapshot is safe, even if the first
        // response was lost. A canceled/invalidated row requires its new version.
        let existing: Option<Value>=sqlx::query_scalar("SELECT to_jsonb(q) - 'workspace_id' FROM conversion_outbox q WHERE workspace_id=$1 AND kind=$2 AND entity_key=$3")
            .bind(self.workspace).bind(&input.kind).bind(&input.entity_key).fetch_optional(&mut *tx).await?;
        if let Some(job) = &existing
            && job["state"] == "awaiting_connection"
            && job["fingerprint"] == input.fingerprint
        {
            return Ok(job.clone());
        }
        if preview["job_version"] != input.version {
            return Err(Error::Conflict);
        }
        let job:Value=sqlx::query_scalar("INSERT INTO conversion_outbox AS q(id,workspace_id,kind,entity_key,transaction_id,fingerprint,snapshot,state) VALUES($1,$2,$3,$4,$5,$6,$7,'awaiting_connection') ON CONFLICT(workspace_id,kind,entity_key) DO UPDATE SET fingerprint=EXCLUDED.fingerprint,snapshot=EXCLUDED.snapshot,state='awaiting_connection',version=q.version+1,updated_at=now() RETURNING to_jsonb(q) - 'workspace_id'")
            .bind(Uuid::new_v4()).bind(self.workspace).bind(&input.kind).bind(&input.entity_key)
            .bind(preview["snapshot"]["transaction_id"].as_str().unwrap_or_default()).bind(&input.fingerprint).bind(&preview["snapshot"]).fetch_one(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "staged",
            actor,
            json!({"id":job["id"],"version":job["version"],"fingerprint":job["fingerprint"]}),
        )
        .await?;
        tx.commit().await?;
        Ok(job)
    }
    pub async fn cancel_conversion(&self, id: Uuid, version: i32, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE conversion_outbox SET state='canceled',version=version+1,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND version=$3 AND state!='canceled'")
            .bind(self.workspace).bind(id).bind(version).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        record(
            &mut tx,
            self.workspace,
            "canceled",
            actor,
            json!({"id":id,"version":version+1}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_and_stable_ids() {
        let workspace = Uuid::new_v4();
        assert_eq!(
            transaction_id(workspace, "booking", "sale"),
            transaction_id(workspace, "booking", "sale")
        );
        assert_ne!(
            transaction_id(workspace, "booking", "sale"),
            transaction_id(Uuid::new_v4(), "booking", "sale")
        );
        assert_ne!(
            transaction_id(workspace, "booking", "sale"),
            transaction_id(workspace, "qualified", "sale")
        );
        assert!(Settings::default().validate().is_err());
        let mut s = Settings {
            customer_id: "1234567890".into(),
            qualified_action_id: "12".into(),
            ..Default::default()
        };
        assert!(s.validate().is_ok());
        s.booking_action_id = "12".into();
        assert!(s.validate().is_err());
        let mut c = Consent {
            ad_user_data: Decision::Granted,
            ..Default::default()
        };
        assert!(c.validate().is_err());
        c.reference = "CMP receipt 1".into();
        c.effective_at = Some(Utc::now() - Duration::hours(1));
        assert!(c.validate().is_ok());
        c.effective_at = Some(Utc::now() + Duration::hours(1));
        assert!(c.validate().is_err());
    }
}
