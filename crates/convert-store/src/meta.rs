//! Meta (Facebook and Instagram ads) as a destination for pipeline conversions through the
//! Conversions API: the owner-supplied dataset and access token, matching and Limited
//! Data Use settings, per-action event mappings, and the planner and durable delivery
//! rows. No provider I/O.
//!
//! Meta state lives in its own tables, so every Google and Microsoft query, row and status
//! is unchanged. The delivery switch, automatic sending and the consent policy are the
//! shared `conversion_delivery_settings`. Customer information is normalized and hashed
//! by the planner, so delivery rows never hold a raw phone number or email address.
use crate::{
    Error, Result, Store,
    conversions::{self, Consent, Decision},
    pipeline::{NEW_LEAD_HOLD, record, sql},
};
use chrono::{DateTime, Duration, Utc};
use convert_domain::{ValidationError, meta as rules};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Row, Transaction};
use uuid::Uuid;

/// Meta rejects a whole request if any event is older than 7 days; leave an hour of margin
/// between planning and sending.
const MAX_AGE: Duration = Duration::hours(7 * 24 - 1);
/// Browser details kept for matching are cleared after this.
const VISIT_DETAIL_DAYS: i32 = 30;

/// The visit behind a lead (form session, or DNI session for a call), with the browser
/// details Meta uses for matching.
pub(crate) const META_VISIT: &str = "LEFT JOIN LATERAL (SELECT s.evidence,s.created_at AS visit_at,s.client_user_agent,host(s.client_ip) AS client_ip FROM website_submissions f JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id WHERE f.workspace_id=l.workspace_id AND f.lead_id=l.id ORDER BY f.created_at LIMIT 1) ws ON true
LEFT JOIN LATERAL (SELECT s.evidence,s.created_at AS visit_at,s.client_user_agent,host(s.client_ip) AS client_ip FROM dni_call_evidence dc JOIN dni_assignments da ON da.id=dc.assignment_id AND da.workspace_id=dc.workspace_id JOIN website_sessions s ON s.site_id=da.site_id AND s.id=da.session_id WHERE dc.workspace_id=l.workspace_id AND dc.lead_id=l.id AND dc.status='inferred_window' LIMIT 1) ds ON true";
/// A Meta ad click on the lead's visit: an `fbclid`, or a Meta campaign tag with the
/// Pixel's `_fbp` browser ID. Used with the `ws`/`ds` visit joins.
pub(crate) const HAS_META: &str = "(COALESCE(COALESCE(ws.evidence,ds.evidence)->>'fbclid','')<>'' OR (lower(COALESCE(COALESCE(ws.evidence,ds.evidence)->>'utm_source','')) IN ('facebook','fb','instagram','ig','meta') AND COALESCE(COALESCE(ws.evidence,ds.evidence)->>'fbp','')<>''))";

// Never serialize/debug the encrypted token.
pub struct Connection {
    pub revision: i64,
    pub dataset_id: Option<String>,
    pub dataset_name: Option<String>,
    pub encrypted_token: Option<String>,
    pub token_fingerprint: Option<String>,
    pub token_saved_at: Option<DateTime<Utc>>,
    pub match_mode: String,
    pub ldu: String,
    pub settings_version: i32,
    pub validation: Option<Value>,
    pub validated_revision: Option<i64>,
    pub checked_at: Option<DateTime<Utc>>,
}
impl Connection {
    /// The saved check covers the current dataset and token and is under 24 hours old.
    pub fn check_current(&self) -> bool {
        self.validation.is_some()
            && self.validated_revision == Some(self.revision)
            && self
                .checked_at
                .is_some_and(|t| t > Utc::now() - Duration::hours(24))
    }
    /// Public status: never the token, only its fingerprint and when it was saved.
    pub fn status(&self) -> Value {
        json!({"revision":self.revision,"connected":self.encrypted_token.is_some(),"dataset_id":self.dataset_id,"dataset_name":self.dataset_name,
            "token_fingerprint":self.token_fingerprint,"token_saved_at":self.token_saved_at,"match":self.match_mode,"ldu":self.ldu,
            "settings_version":self.settings_version,"validation":self.validation,"checked_at":self.checked_at,"check_current":self.check_current()})
    }
}
/// Reconsider blocked Meta plans now (all, or one action's).
pub(crate) async fn wake_plans(
    conn: &mut PgConnection,
    workspace: Uuid,
    action: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE meta_event_plans p SET plan_after=now() FROM pipeline_events e WHERE p.workspace_id=$1 AND p.state='blocked' AND e.workspace_id=p.workspace_id AND e.id=p.event_id AND ($2::text IS NULL OR e.action_key=$2)")
        .bind(workspace).bind(action).execute(conn).await?;
    Ok(())
}

impl Store {
    pub async fn meta_connection(&self) -> Result<Connection> {
        let row = sqlx::query("SELECT * FROM meta_connections WHERE workspace_id=$1")
            .bind(self.workspace)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => Connection {
                revision: r.get("revision"),
                dataset_id: r.get("dataset_id"),
                dataset_name: r.get("dataset_name"),
                encrypted_token: r.get("encrypted_token"),
                token_fingerprint: r.get("token_fingerprint"),
                token_saved_at: r.get("token_saved_at"),
                match_mode: r.get("match_mode"),
                ldu: r.get("ldu"),
                settings_version: r.get("settings_version"),
                validation: r.get("validation"),
                validated_revision: r.get("validated_revision"),
                checked_at: r.get("checked_at"),
            },
            None => Connection {
                revision: 0,
                dataset_id: None,
                dataset_name: None,
                encrypted_token: None,
                token_fingerprint: None,
                token_saved_at: None,
                match_mode: "meta_clicks".into(),
                ldu: "off".into(),
                settings_version: 0,
                validation: None,
                validated_revision: None,
                checked_at: None,
            },
        })
    }
    /// Save or replace the dataset and token at `revision` (0 before the first save). The
    /// caller has read the dataset with the token; that read is recorded as the check.
    pub async fn save_meta_connection(
        &self,
        revision: i64,
        dataset: (&str, &str),
        encrypted: &str,
        fingerprint: &str,
        actor: &str,
    ) -> Result<i64> {
        let (dataset_id, dataset_name) = dataset;
        if !rules_numeric(dataset_id) || fingerprint.len() != 12 {
            return Err(ValidationError("Use the numeric dataset (Pixel) ID").into());
        }
        let name: String = dataset_name.chars().take(300).collect();
        let report = json!({"valid":true,"dataset":{"id":dataset_id,"name":name},"message":"The token can use this dataset. Send a test event to confirm events arrive."});
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let next: i64 = sqlx::query_scalar("INSERT INTO meta_connections AS c(workspace_id,revision,dataset_id,dataset_name,encrypted_token,token_fingerprint,token_saved_at,validation,validated_revision,checked_at)
            SELECT $1,1,$3,$4,$5,$6,now(),$7,1,now() WHERE $2=0
            ON CONFLICT(workspace_id) DO UPDATE SET revision=c.revision+1,dataset_id=$3,dataset_name=$4,encrypted_token=$5,token_fingerprint=$6,token_saved_at=now(),validation=$7,validated_revision=c.revision+1,checked_at=now()
            WHERE c.revision=$2 RETURNING revision")
            .bind(self.workspace).bind(revision).bind(dataset_id).bind(&name).bind(encrypted).bind(fingerprint).bind(&report)
            .fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        // Requests held for a rejected token are tried again with the new one.
        sqlx::query("UPDATE meta_deliveries SET state='queued',due_at=now(),attempted_at=NULL,diagnostic='Requeued after a new access token was saved.',updated_at=now() WHERE workspace_id=$1 AND state='needs_review' AND attempted_at IS NULL")
            .bind(self.workspace).execute(&mut *tx).await?;
        wake_plans(&mut tx, self.workspace, None).await?;
        record(
            &mut tx,
            self.workspace,
            "meta_connection_saved",
            actor,
            json!({"revision":next,"dataset_id":dataset_id,"token_fingerprint":fingerprint}),
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }
    pub async fn disconnect_meta(&self, revision: i64, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE meta_connections SET revision=revision+1,dataset_id=NULL,dataset_name=NULL,encrypted_token=NULL,token_fingerprint=NULL,token_saved_at=NULL,validation=NULL,validated_revision=NULL,checked_at=NULL WHERE workspace_id=$1 AND revision=$2 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).bind(revision).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        // Browser details are only kept for Meta matching.
        sqlx::query("UPDATE website_sessions s SET client_user_agent=NULL,client_ip=NULL FROM sites t WHERE t.id=s.site_id AND t.workspace_id=$1 AND (s.client_user_agent IS NOT NULL OR s.client_ip IS NOT NULL)")
            .bind(self.workspace).execute(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "meta_disconnected",
            actor,
            json!({}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Which leads are sent and whether Limited Data Use is applied, at `version`.
    pub async fn save_meta_settings(
        &self,
        version: i32,
        match_mode: &str,
        ldu: &str,
        actor: &str,
    ) -> Result<i32> {
        if !matches!(match_mode, "meta_clicks" | "all_leads") {
            return Err(ValidationError("match must be meta_clicks or all_leads").into());
        }
        if !matches!(ldu, "off" | "geolocate") {
            return Err(ValidationError("ldu must be off or geolocate").into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let next: i32 = sqlx::query_scalar("INSERT INTO meta_connections AS c(workspace_id,match_mode,ldu,settings_version) SELECT $1,$3,$4,1 WHERE $2=0
            ON CONFLICT(workspace_id) DO UPDATE SET match_mode=$3,ldu=$4,settings_version=c.settings_version+1 WHERE c.settings_version=$2 RETURNING settings_version")
            .bind(self.workspace).bind(version).bind(match_mode).bind(ldu).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        wake_plans(&mut tx, self.workspace, None).await?;
        record(
            &mut tx,
            self.workspace,
            "meta_settings_saved",
            actor,
            json!({"settings_version":next,"match":match_mode,"ldu":ldu}),
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }
    /// Record a destination check (read of the dataset, optionally a test event).
    pub async fn save_meta_validation(
        &self,
        revision: i64,
        report: &Value,
        actor: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE meta_connections SET validation=$3,validated_revision=$2,checked_at=now() WHERE workspace_id=$1 AND revision=$2 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).bind(revision).bind(report).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        wake_plans(&mut tx, self.workspace, None).await?;
        record(
            &mut tx,
            self.workspace,
            "meta_destination_checked",
            actor,
            json!({"valid":report["valid"],"revision":revision,"test_event":report["test_event"]}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Map an action to a Meta event name, or clear it. A new or changed mapping applies
    /// to events recorded from now on, or since `include_since` (at most 7 days ago).
    pub async fn set_meta_action_event(
        &self,
        key: &str,
        event: Option<&str>,
        include_since: Option<DateTime<Utc>>,
        version: i32,
        actor: &str,
    ) -> Result<Value> {
        if let Some(since) = include_since {
            let now = Utc::now();
            if event.is_none() {
                return Err(ValidationError("include_since needs an event_name").into());
            }
            if since > now || since < now - MAX_AGE {
                return Err(ValidationError(
                    "include_since must be in the past and within the last 7 days (Meta's limit)",
                )
                .into());
            }
        }
        if event.is_some_and(|e| !rules::event_name(e)) {
            return Err(ValidationError(
                "event_name must be a Meta standard event (such as Lead or Purchase) or a custom name: a letter, then up to 39 letters, digits or underscores",
            )
            .into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let saved: Value = sqlx::query_scalar("UPDATE conversion_actions a SET meta_mapped_at=CASE WHEN $3::text IS NULL THEN NULL WHEN $5::timestamptz IS NOT NULL THEN $5 WHEN a.meta_event_name IS DISTINCT FROM $3 THEN now() ELSE a.meta_mapped_at END,meta_event_name=$3,version=version+1,updated_at=now() WHERE workspace_id=$1 AND key=$2 AND version=$4 RETURNING to_jsonb(a)-'workspace_id'-'id'")
            .bind(self.workspace).bind(key).bind(event).bind(version).bind(include_since).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        if let Some(since) = include_since {
            sqlx::query("DELETE FROM meta_event_plans p USING pipeline_events e WHERE p.workspace_id=$1 AND p.state='final' AND p.reason LIKE 'Recorded before this action was mapped%' AND e.workspace_id=p.workspace_id AND e.id=p.event_id AND e.action_key=$2 AND e.created_at>=$3 AND NOT EXISTS(SELECT 1 FROM meta_deliveries d WHERE d.workspace_id=p.workspace_id AND d.event_id=p.event_id)")
                .bind(self.workspace).bind(key).bind(since).execute(&mut *tx).await?;
        }
        wake_plans(&mut tx, self.workspace, Some(key)).await?;
        record(&mut tx, self.workspace, "meta_action_event_saved", actor, json!({"key":key,"version":saved["version"],"event_name":event,"include_since":include_since})).await?;
        tx.commit().await?;
        Ok(saved)
    }
    /// Facts for the Meta section of the setup checklist.
    pub async fn meta_setup(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object(
            'enabled_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled),
            'mapped_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled AND meta_event_name IS NOT NULL),
            'events',COALESCE((SELECT jsonb_agg(jsonb_build_object('action',key,'event_name',meta_event_name,'mapped_at',meta_mapped_at,'has_value',default_value_minor IS NOT NULL) ORDER BY key) FROM conversion_actions WHERE workspace_id=$1 AND enabled AND meta_event_name IS NOT NULL),'[]'::jsonb),
            'allow_inferred_calls',COALESCE((SELECT (configuration->>'allow_inferred_calls')::boolean FROM conversion_settings WHERE workspace_id=$1),false),
            'dni_calls',EXISTS(SELECT 1 FROM dni_pools WHERE workspace_id=$1) OR EXISTS(SELECT 1 FROM dni_call_evidence WHERE workspace_id=$1 AND status='inferred_window'),
            'events_sent',(SELECT count(*) FROM meta_deliveries WHERE workspace_id=$1 AND state='processed'),
            'events_needing_review',(SELECT count(*) FROM meta_deliveries WHERE workspace_id=$1 AND state IN ('failed','needs_review','uncertain')))")
            .bind(self.workspace).fetch_one(&self.pool).await?)
    }
    /// Workspaces with Meta work: mapped actions, due plans or changed events.
    pub async fn meta_workspaces(&self) -> Result<Vec<Uuid>> {
        Ok(sqlx::query_scalar("SELECT workspace_id FROM conversion_actions WHERE enabled AND meta_event_name IS NOT NULL UNION SELECT workspace_id FROM meta_event_plans WHERE state='blocked' AND plan_after<=now() UNION SELECT e.workspace_id FROM meta_event_plans p JOIN pipeline_events e ON e.workspace_id=p.workspace_id AND e.id=p.event_id WHERE p.revision<>e.revision LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    /// Whether the worker should repeat the dataset check: work is waiting and the check
    /// is missing or older than 24 hours.
    pub async fn meta_refresh_due(&self) -> Result<bool> {
        let c = self.meta_connection().await?;
        if c.encrypted_token.is_none() || c.check_current() {
            return Ok(false);
        }
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meta_event_plans WHERE workspace_id=$1 AND state='blocked' AND plan_after<=now()) OR EXISTS(SELECT 1 FROM meta_deliveries WHERE workspace_id=$1 AND state IN ('queued','retry'))")
            .bind(self.workspace).fetch_one(&self.pool).await?)
    }
    /// Clear browser details kept for matching after 30 days, from sessions and from the
    /// requests that carried them. Bounded batches; called by the worker.
    pub async fn clear_meta_visit_details(&self) -> Result<u64> {
        let a = sqlx::query("UPDATE website_sessions SET client_user_agent=NULL,client_ip=NULL WHERE (site_id,id) IN (SELECT site_id,id FROM website_sessions WHERE created_at<now()-make_interval(days=>$1) AND (client_user_agent IS NOT NULL OR client_ip IS NOT NULL) LIMIT 500)")
            .bind(VISIT_DETAIL_DAYS).execute(&self.pool).await?.rows_affected();
        let b = sqlx::query("UPDATE meta_deliveries SET request=request #- '{event,user_data,client_ip_address}' #- '{event,user_data,client_user_agent}' WHERE id IN (SELECT id FROM meta_deliveries WHERE created_at<now()-make_interval(days=>$1) AND request->'event'->'user_data' ?| array['client_ip_address','client_user_agent'] LIMIT 500)")
            .bind(VISIT_DETAIL_DAYS).execute(&self.pool).await?.rows_affected();
        Ok(a + b)
    }
}
fn rules_numeric(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// `delivery.meta` for one event row of the pipeline event query, or null when the event
/// was never considered for Meta.
pub(crate) fn event_status(r: &sqlx::postgres::PgRow, revision: i32) -> Value {
    let Some(plan_revision) = r.get::<Option<i32>, _>("mt_plan_revision") else {
        return Value::Null;
    };
    let plan: String = r.get("mt_plan_state");
    let raw: Option<String> = r.get("mt_delivery_state");
    let diagnostic: Option<String> = r.get("mt_diagnostic");
    let (status, reason) = if plan_revision != revision {
        ("pending", Some("Waiting for the delivery worker".into()))
    } else if plan != "planned" {
        if r.get::<String, _>("status") == "canceled" && raw.as_deref() != Some("processed") {
            ("canceled", r.get("mt_reason"))
        } else {
            ("blocked", r.get("mt_reason"))
        }
    } else {
        match raw.as_deref() {
            Some("queued" | "retry" | "sending") => ("queued", diagnostic),
            Some("processed") => ("delivered", diagnostic),
            Some("uncertain") => ("uncertain", diagnostic),
            Some("canceled") => ("canceled", diagnostic),
            None => ("blocked", r.get("mt_reason")),
            _ => ("failed", diagnostic),
        }
    };
    json!({"status":status,"reason":reason,"delivery_id":r.get::<Option<Uuid>,_>("mt_delivery_id"),"delivery_state":raw,
        "delivery_revision":r.get::<Option<i32>,_>("mt_delivery_revision"),"trace_id":r.get::<Option<String>,_>("mt_tracking_id")})
}

enum Plan {
    Queued,
    Unchanged,
    Final(String),
    Blocked(String, i32),
}
fn text(evidence: &Value, key: &str) -> Option<String> {
    evidence
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 600
                && !v.chars().any(|c| c.is_control() || c.is_whitespace())
        })
        .map(str::to_string)
}
/// Meta click evidence on a visit, matching `HAS_META`.
fn meta_click(evidence: &Value) -> bool {
    text(evidence, "fbclid").is_some()
        || (text(evidence, "fbp").is_some()
            && text(evidence, "utm_source").is_some_and(|s| {
                matches!(
                    s.to_lowercase().as_str(),
                    "facebook" | "fb" | "instagram" | "ig" | "meta"
                )
            }))
}
impl Store {
    /// Turn due event revisions into queued Meta requests, or record why not. Holds the
    /// workspace lock used by every edit. Independent of the Google and Microsoft plans.
    pub async fn plan_meta(&self, only: Option<Uuid>) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let settings = conversions::settings(&mut tx, self.workspace).await?;
        let policy = sqlx::query("SELECT enabled,auto_send,consent_policy FROM conversion_delivery_settings WHERE workspace_id=$1")
            .bind(self.workspace).fetch_optional(&mut *tx).await?;
        let (enabled, auto_send, consent_policy) =
            policy
                .as_ref()
                .map_or((false, false, "per_lead".to_string()), |r| {
                    (
                        r.get("enabled"),
                        r.get("auto_send"),
                        r.get("consent_policy"),
                    )
                });
        let connection = sqlx::query("SELECT revision,dataset_id,match_mode,ldu,validation,validated_revision,checked_at FROM meta_connections WHERE workspace_id=$1 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).fetch_optional(&mut *tx).await?;
        let mode: String =
            sqlx::query_scalar("SELECT match_mode FROM meta_connections WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or_else(|| "meta_clicks".into());
        let rows = sqlx::query(sql(format!("SELECT e.id,e.action_key,e.external_id,e.lead_id,e.value_minor,e.occurred_at,e.created_at,e.revision,e.status,e.sendable,
            a.enabled AS action_enabled,a.meta_event_name,a.meta_mapped_at,a.counting,a.trigger,l.channel,l.contact,l.is_demo,i.qualification,
            ws.evidence IS NOT NULL AS from_form,COALESCE(ws.evidence,ds.evidence) AS evidence,COALESCE(ws.visit_at,ds.visit_at) AS visit_at,
            COALESCE(ws.client_user_agent,ds.client_user_agent) AS client_user_agent,COALESCE(ws.client_ip,ds.client_ip) AS client_ip,
            cc.evidence AS consent,cc.version AS consent_version,p.entity_key
            FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key
            LEFT JOIN meta_event_plans p ON p.workspace_id=e.workspace_id AND p.event_id=e.id
            LEFT JOIN leads l ON l.workspace_id=e.workspace_id AND l.id=e.lead_id
            LEFT JOIN inquiries i ON i.workspace_id=l.workspace_id AND i.id=l.inquiry_id
            {META_VISIT}
            LEFT JOIN conversion_consents cc ON cc.workspace_id=e.workspace_id AND cc.lead_id=e.lead_id
            WHERE e.workspace_id=$1 AND ($2::uuid IS NULL OR e.id=$2) AND (
              (p.event_id IS NULL AND a.meta_event_name IS NOT NULL AND e.updated_at>=a.meta_mapped_at)
              OR p.revision<>e.revision OR (p.state='blocked' AND p.plan_after<=now()))
            ORDER BY e.created_at,e.id LIMIT 100 FOR UPDATE OF e")))
            .bind(self.workspace).bind(only).fetch_all(&mut *tx).await?;
        let now = Utc::now();
        let mut planned = 0;
        for r in rows {
            let id: Uuid = r.get("id");
            let ctx = Context {
                workspace: self.workspace,
                settings: &settings,
                enabled,
                auto_send,
                consent_policy: &consent_policy,
                connection: connection.as_ref(),
                mode: &mode,
                now,
            };
            let (outcome, key) = plan_one(&mut tx, &ctx, &r).await?;
            let (state, reason, delay) = match outcome {
                Plan::Queued => {
                    planned += 1;
                    ("planned", None, 0)
                }
                Plan::Unchanged => ("planned", None, 0),
                Plan::Final(reason) => ("final", Some(reason), 0),
                Plan::Blocked(reason, minutes) => ("blocked", Some(reason), minutes),
            };
            sqlx::query("INSERT INTO meta_event_plans(workspace_id,event_id,revision,state,reason,plan_after,entity_key) VALUES($1,$2,$3,$4,$5,now()+make_interval(mins=>$6),$7)
                ON CONFLICT(workspace_id,event_id) DO UPDATE SET revision=$3,state=$4,reason=$5,plan_after=now()+make_interval(mins=>$6),entity_key=$7,updated_at=now()")
                .bind(self.workspace).bind(id).bind(r.get::<i32,_>("revision")).bind(state).bind(reason).bind(delay).bind(key).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(planned)
    }
}
struct Context<'a> {
    workspace: Uuid,
    settings: &'a conversions::Settings,
    enabled: bool,
    auto_send: bool,
    consent_policy: &'a str,
    connection: Option<&'a sqlx::postgres::PgRow>,
    mode: &'a str,
    now: DateTime<Utc>,
}
/// One event revision. Returns the outcome and the entity key the event holds after it.
async fn plan_one(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context<'_>,
    r: &sqlx::postgres::PgRow,
) -> Result<(Plan, Option<String>)> {
    let ws = c.workspace;
    let id: Uuid = r.get("id");
    let revision: i32 = r.get("revision");
    let held_key: Option<String> = r.get("entity_key");
    // A newer revision replaces queued requests that never left.
    sqlx::query("UPDATE meta_deliveries SET state='canceled',due_at=NULL,diagnostic='Replaced by a newer revision of the event before it was sent.',updated_at=now() WHERE workspace_id=$1 AND event_id=$2 AND event_revision<$3 AND attempted_at IS NULL AND state IN ('queued','retry')")
        .bind(ws).bind(id).bind(revision).execute(&mut **tx).await?;
    let history = sqlx::query("SELECT state,attempted_at FROM meta_deliveries WHERE workspace_id=$1 AND event_id=$2 AND state<>'canceled' ORDER BY created_at")
        .bind(ws).bind(id).fetch_all(&mut **tx).await?;
    let sent = history
        .iter()
        .any(|h| h.get::<Option<DateTime<Utc>>, _>("attempted_at").is_some());
    let release = |plan: Plan| (plan, if sent { held_key.clone() } else { None });
    let state = |s: &str| history.iter().any(|h| h.get::<String, _>("state") == s);
    if ["queued", "retry", "sending"].iter().any(|s| state(s)) {
        return Ok((
            Plan::Blocked(
                "Waiting for the previous Meta request for this event to finish".into(),
                2,
            ),
            held_key,
        ));
    }
    if state("processed") {
        // Meta has no way to change or withdraw a sent event.
        return Ok((
            if r.get::<String, _>("status") != "active" {
                Plan::Final("Canceled after it was sent; Meta cannot withdraw a sent event".into())
            } else if history.len() == 1 && revision > 1 {
                Plan::Final("Changed after it was sent; Meta cannot restate a sent event, so the first version stands".into())
            } else {
                Plan::Unchanged
            },
            held_key,
        ));
    }
    if state("uncertain") {
        return Ok((Plan::Final("An earlier send of this event may have reached Meta; it is not sent again automatically. Check Events Manager.".into()), held_key));
    }
    if r.get::<String, _>("status") != "active" {
        return Ok(release(Plan::Final(
            "Canceled before it was sent to Meta".into(),
        )));
    }
    match eligible(c, r)? {
        Err(plan) => Ok(release(plan)),
        Ok(upload) => {
            let action_key: String = r.get("action_key");
            let entity_key = if r.get::<String, _>("counting") == "once_per_click" {
                match &upload.click {
                    Some(click) => format!(
                        "{action_key}:click:{}",
                        &conversions::digest(&json!(click))[..32]
                    ),
                    None => format!(
                        "{action_key}:lead:{}",
                        r.get::<Option<Uuid>, _>("lead_id").unwrap_or_default()
                    ),
                }
            } else {
                format!("{action_key}:event:{}", r.get::<String, _>("external_id"))
            };
            if sent && held_key.as_deref() != Some(entity_key.as_str()) {
                return Ok((
                    Plan::Final(
                        "Already sent to Meta for a different click or lead; it cannot change"
                            .into(),
                    ),
                    held_key,
                ));
            }
            let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meta_event_plans WHERE workspace_id=$1 AND entity_key=$2 AND event_id<>$3)")
                .bind(ws).bind(&entity_key).bind(id).fetch_one(&mut **tx).await?;
            if taken {
                return Ok(release(Plan::Final(
                    "Already counted for this Meta click or lead (counted once per click)".into(),
                )));
            }
            let snapshot = json!({"kind":"pipeline","provider":"meta","event_id":id,"action_key":action_key,"external_id":r.get::<String,_>("external_id"),
                "revision":revision,"lead_id":r.get::<Option<Uuid>,_>("lead_id"),"channel":r.get::<Option<String>,_>("channel"),"event_at":r.get::<DateTime<Utc>,_>("occurred_at"),
                "value_minor":r.get::<Option<i64>,_>("value_minor"),"currency":"USD","consent_policy":c.consent_policy,
                "consent_version":r.get::<Option<i32>,_>("consent_version").unwrap_or(0),"counting":r.get::<String,_>("counting"),
                "event_name":upload.request["event"]["event_name"],"dataset_id":upload.request["dataset_id"],"match":c.mode,
                "matched_by":upload.matched_by});
            queue(tx, c, id, revision, &snapshot, &upload.request).await?;
            Ok((Plan::Queued, Some(entity_key)))
        }
    }
}
struct Upload {
    request: Value,
    click: Option<String>,
    matched_by: Vec<&'static str>,
}
/// Upload eligibility, mirroring the Google and Microsoft planners' rules.
fn eligible(
    c: &Context<'_>,
    r: &sqlx::postgres::PgRow,
) -> Result<std::result::Result<Upload, Plan>> {
    use Plan::*;
    let now = c.now;
    let event_at: DateTime<Utc> = r.get("occurred_at");
    let created_at: DateTime<Utc> = r.get("created_at");
    let blocked = |m: &str, minutes: i32| Ok(Err(Blocked(m.into(), minutes)));
    let finished = |m: &str| Ok(Err(Final(m.into())));
    if !r.get::<bool, _>("sendable") {
        return finished(
            "Recorded while automatic sending was off; kept for reporting and never sent",
        );
    }
    let (Some(event_name), Some(mapped_at)) = (
        r.get::<Option<String>, _>("meta_event_name"),
        r.get::<Option<DateTime<Utc>>, _>("meta_mapped_at"),
    ) else {
        return finished("This action is not mapped to a Meta event");
    };
    let trigger: String = r.get("trigger");
    if created_at < mapped_at || (trigger == "new_lead" && event_at < mapped_at) {
        return finished(
            "Recorded before this action was mapped to its Meta event; never sent to Meta, so an earlier integration cannot be counted twice",
        );
    }
    if !r.get::<bool, _>("action_enabled") {
        return blocked("Action is disabled", 15);
    }
    let Some(lead) = r.get::<Option<Uuid>, _>("lead_id") else {
        return finished("No attributed lead matched this event");
    };
    if r.get::<Option<bool>, _>("is_demo") == Some(true) {
        return finished("Demo data is never sent");
    }
    if r.get::<Option<String>, _>("qualification").as_deref() == Some("spam") {
        return blocked("The inquiry is marked spam", 15);
    }
    if trigger == "new_lead" {
        if r.get::<Option<String>, _>("qualification").as_deref() == Some("unqualified") {
            return blocked("The inquiry is marked unqualified", 15);
        }
        let wait = event_at + NEW_LEAD_HOLD - now;
        if wait > Duration::zero() {
            let minutes = i32::try_from(wait.num_minutes() + 1).unwrap_or(i32::MAX);
            return blocked(
                "Held for 6 hours so the lead can be reviewed before sending",
                minutes,
            );
        }
    }
    let channel = r.get::<Option<String>, _>("channel").unwrap_or_default();
    let mut evidence: Option<Value> = r.get("evidence");
    let mut visit_at: Option<DateTime<Utc>> = r.get("visit_at");
    let mut ua: Option<String> = r.get("client_user_agent");
    let mut ip: Option<String> = r.get("client_ip");
    let all_leads = c.mode == "all_leads";
    // A call's visit is inferred from DNI timing; it needs the owner's opt-in.
    if channel == "call" && evidence.is_some() && !c.settings.allow_inferred_calls {
        if all_leads {
            (evidence, visit_at, ua, ip) = (None, None, None, None);
        } else {
            return blocked(
                "Inferred call attribution requires explicit opt-in (allow_inferred_calls in the destination settings)",
                15,
            );
        }
    }
    let ev = evidence.clone().unwrap_or_default();
    let click = meta_click(&ev);
    if !all_leads && !click {
        return if now - event_at > Duration::days(1) {
            finished(
                "No Meta ad click (fbclid) on this lead's visit; only Meta clicks are sent (match setting meta_clicks)",
            )
        } else {
            blocked("No Meta ad click (fbclid) on this lead's visit", 60)
        };
    }
    if let Some(visit) = visit_at
        && event_at < visit
    {
        return finished("Event predates the ad visit");
    }
    if now - event_at > MAX_AGE {
        return finished("Meta accepts events from the last 7 days only");
    }
    if event_at > now {
        let minutes = i32::try_from((event_at - now).num_minutes() + 1).unwrap_or(i32::MAX);
        return blocked("Waiting until the event time", minutes);
    }
    let mut consent: Consent = r
        .get::<Option<Value>, _>("consent")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ValidationError("Invalid saved consent evidence"))?
        .unwrap_or_default();
    consent.version = r.get::<Option<i32>, _>("consent_version").unwrap_or(0);
    if c.consent_policy == "per_lead"
        && (consent.ad_user_data != Decision::Granted
            || consent.ad_personalization == Decision::Unknown
            || consent.effective_at.is_none_or(|t| t > event_at))
    {
        return blocked("No advertising consent recorded for this lead", 15);
    }
    if !c.enabled || !c.auto_send {
        return blocked("Automatic sending is off", 60);
    }
    let Some(m) = c.connection else {
        return blocked("Save the Meta dataset and access token to send", 15);
    };
    let report: Value = m.get::<Option<Value>, _>("validation").unwrap_or_default();
    let checked: Option<DateTime<Utc>> = m.get("checked_at");
    if m.get::<Option<i64>, _>("validated_revision") != Some(m.get("revision"))
        || !checked.is_some_and(|t| t > now - Duration::hours(24))
    {
        return blocked("Waiting for the Meta destination check", 2);
    }
    if report["valid"] != true {
        return blocked("The Meta destination check failed; see Conversions", 10);
    }
    let value: Option<i64> = r.get("value_minor");
    if event_name == "Purchase" && value.is_none() {
        return finished(
            "Meta requires a value for Purchase events; post the event with value_minor or set the action's default value",
        );
    }
    // Customer information, normalized and hashed here so it is never stored raw.
    let contact: String = r.get::<Option<String>, _>("contact").unwrap_or_default();
    let mut user = serde_json::Map::new();
    let mut matched_by = Vec::new();
    let phone = rules::normalize_phone(&contact);
    if let Some(p) = &phone {
        user.insert("ph".into(), json!([rules::sha256_hex(p)]));
        matched_by.push("phone");
    }
    if let Some(e) = rules::normalize_email(&contact) {
        user.insert("em".into(), json!([rules::sha256_hex(&e)]));
        matched_by.push("email");
    }
    let fbclid = text(&ev, "fbclid");
    let fbc = text(&ev, "fbc")
        .filter(|v| rules::cookie_value(v))
        .or_else(|| {
            fbclid
                .as_deref()
                .zip(visit_at)
                .and_then(|(f, t)| rules::fbc(f, t.timestamp_millis()))
        });
    if let Some(v) = fbc {
        user.insert("fbc".into(), json!(v));
        matched_by.push("click");
    }
    if let Some(v) = text(&ev, "fbp").filter(|v| rules::cookie_value(v)) {
        user.insert("fbp".into(), json!(v));
        matched_by.push("browser");
    }
    if matched_by.is_empty() {
        return finished("No phone, email, Meta click or browser ID that Meta can match");
    }
    user.insert(
        "external_id".into(),
        json!([rules::sha256_hex(&lead.to_string())]),
    );
    if let Some(v) = &ip {
        user.insert("client_ip_address".into(), json!(v));
    }
    if let Some(v) = &ua {
        user.insert("client_user_agent".into(), json!(v));
    }
    let landing = ev.get("landing_url").and_then(Value::as_str);
    let action_source = if trigger != "new_lead" {
        "system_generated"
    } else if channel == "call" {
        "phone_call"
    } else if r.get::<bool, _>("from_form") && ua.is_some() && landing.is_some() {
        "website"
    } else {
        "other"
    };
    let mut event = json!({"event_name":event_name,"event_time":event_at.timestamp(),"event_id":format!("vc-{}",r.get::<Uuid,_>("id")),
        "action_source":action_source,"user_data":Value::Object(user)});
    if action_source == "website" {
        event["event_source_url"] = json!(landing);
    }
    if let Some(v) = value {
        // Minor units to a decimal amount; i64 cents fit f64 exactly up to 2^53.
        event["custom_data"] = json!({"value":v as f64 / 100.0,"currency":"USD"});
    }
    if m.get::<String, _>("ldu") == "geolocate" {
        if ip.is_some() {
            event["data_processing_options"] = json!(["LDU"]);
            event["data_processing_options_country"] = json!(0);
            event["data_processing_options_state"] = json!(0);
        } else if phone.as_deref().is_some_and(rules::california_phone) {
            event["data_processing_options"] = json!(["LDU"]);
            event["data_processing_options_country"] = json!(1);
            event["data_processing_options_state"] = json!(1000);
        }
    }
    let request =
        json!({"kind":"upload","dataset_id":m.get::<Option<String>,_>("dataset_id"),"event":event});
    Ok(Ok(Upload {
        request,
        click: fbclid,
        matched_by,
    }))
}
async fn queue(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context<'_>,
    event: Uuid,
    revision: i32,
    snapshot: &Value,
    request: &Value,
) -> Result<()> {
    let connection: i64 = c.connection.map_or(0, |m| m.get("revision"));
    let delivery = Uuid::new_v4();
    sqlx::query("INSERT INTO meta_deliveries(id,workspace_id,event_id,event_revision,kind,connection_revision,snapshot,request,state,due_at) VALUES($1,$2,$3,$4,'upload',$5,$6,$7,'queued',now())")
        .bind(delivery).bind(c.workspace).bind(event).bind(revision).bind(connection).bind(snapshot).bind(request).execute(&mut **tx).await?;
    record(
        tx,
        c.workspace,
        "meta_delivery_queued",
        "planner",
        json!({"delivery_id":delivery,"event_id":event,"revision":revision}),
    )
    .await
}

/// A claimed Meta request, holding the workspace lock used by every edit.
pub struct Guard {
    pub delivery: Value,
    tx: Transaction<'static, Postgres>,
    workspace: Uuid,
    id: Uuid,
}
impl Store {
    /// Claim one due request. A request interrupted after it was armed is uncertain and
    /// never replayed; one interrupted before it left is queued again.
    pub async fn claim_meta_delivery(&self) -> Result<Option<(Uuid, Uuid)>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE meta_deliveries SET state=CASE WHEN attempted_at IS NULL THEN 'retry' ELSE 'uncertain' END,diagnostic=CASE WHEN attempted_at IS NULL THEN diagnostic ELSE 'Worker interrupted while sending; check Events Manager. It is not sent again automatically.' END,due_at=CASE WHEN attempted_at IS NULL THEN now() END,updated_at=now() WHERE state='sending' AND due_at<now()")
            .execute(&mut *tx).await?;
        let row=sqlx::query("SELECT d.id,d.workspace_id FROM meta_deliveries d WHERE d.state IN ('queued','retry') AND d.due_at<=now() AND EXISTS(SELECT 1 FROM conversion_delivery_settings s WHERE s.workspace_id=d.workspace_id AND s.enabled AND s.auto_send) ORDER BY d.due_at,d.id FOR UPDATE OF d SKIP LOCKED LIMIT 1")
            .fetch_optional(&mut *tx).await?;
        let Some(r) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let id: Uuid = r.get("id");
        sqlx::query("UPDATE meta_deliveries SET state='sending',attempts=attempts+1,due_at=now()+interval '2 minutes',updated_at=now() WHERE id=$1")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some((r.get("workspace_id"), id)))
    }
    pub async fn meta_guard(&self, id: Uuid) -> Result<Guard> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let delivery: Value = sqlx::query_scalar("SELECT to_jsonb(d) FROM meta_deliveries d WHERE workspace_id=$1 AND id=$2 AND state='sending' FOR UPDATE")
            .bind(self.workspace).bind(id).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        Ok(Guard {
            delivery,
            tx,
            workspace: self.workspace,
            id,
        })
    }
}
impl Guard {
    /// The encrypted token of the connection this request was planned for, if current.
    pub async fn token(&mut self) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT encrypted_token FROM meta_connections WHERE workspace_id=$1 AND revision=$2 AND dataset_id=$3")
            .bind(self.workspace).bind(self.delivery["connection_revision"].as_i64()).bind(self.delivery["request"]["dataset_id"].as_str())
            .fetch_optional(&mut *self.tx).await?.flatten())
    }
    /// Recheck the event, mapping, policy, consent, connection, check and age under the
    /// lock. `Err` carries why the request must not be sent now.
    pub async fn fresh(&mut self) -> Result<std::result::Result<(), &'static str>> {
        let d = &self.delivery;
        let s = &d["snapshot"];
        let q = &d["request"];
        let event = d["event_id"]
            .as_str()
            .and_then(|v| v.parse::<Uuid>().ok())
            .ok_or(Error::Conflict)?;
        let row = sqlx::query("SELECT e.revision,e.status,e.lead_id,a.enabled AS action_enabled,a.meta_event_name,COALESCE(s.enabled AND s.auto_send,false) AS sending,COALESCE(s.consent_policy,'per_lead') AS consent_policy,
            (SELECT version FROM conversion_consents cc WHERE cc.workspace_id=e.workspace_id AND cc.lead_id=e.lead_id) AS consent_version,
            NOT EXISTS(SELECT 1 FROM hosted_workspaces h WHERE h.workspace_id=e.workspace_id AND NOT h.enabled) AS hosted,
            c.encrypted_token IS NOT NULL AS connected,c.revision AS connection_revision,c.dataset_id,c.validation->>'valid' AS valid,
            c.validated_revision=c.revision AND c.checked_at>now()-interval '24 hours' AS checked
            FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key
            LEFT JOIN conversion_delivery_settings s ON s.workspace_id=e.workspace_id LEFT JOIN meta_connections c ON c.workspace_id=e.workspace_id
            WHERE e.workspace_id=$1 AND e.id=$2")
            .bind(self.workspace).bind(event).fetch_one(&mut *self.tx).await?;
        if json!(row.get::<i32, _>("revision")) != d["event_revision"] {
            return Ok(Err(
                "Replaced by a newer revision of the event before it was sent.",
            ));
        }
        if !row.get::<bool, _>("sending") || !row.get::<bool, _>("hosted") {
            return Ok(Err("Automatic sending is off for this workspace."));
        }
        if row.get::<Option<bool>, _>("connected") != Some(true) {
            return Ok(Err("Meta is not connected."));
        }
        if row.get::<String, _>("status") != "active" {
            return Ok(Err("The event was canceled before it was sent."));
        }
        let policy: String = row.get("consent_policy");
        if !row.get::<bool, _>("action_enabled")
            || row.get::<Option<String>, _>("meta_event_name").as_deref()
                != q["event"]["event_name"].as_str()
            || json!(row.get::<Option<Uuid>, _>("lead_id")) != s["lead_id"]
            || policy != s["consent_policy"]
            || (policy == "per_lead"
                && json!(row.get::<Option<i32>, _>("consent_version").unwrap_or(0))
                    != s["consent_version"])
        {
            return Ok(Err(
                "The action, event mapping, lead, consent or policy changed before it was sent; it will be planned again.",
            ));
        }
        if json!(row.get::<Option<i64>, _>("connection_revision")) != d["connection_revision"]
            || row.get::<Option<String>, _>("dataset_id").as_deref() != q["dataset_id"].as_str()
            || row.get::<Option<bool>, _>("checked") != Some(true)
            || row.get::<Option<String>, _>("valid").as_deref() != Some("true")
        {
            return Ok(Err(
                "The Meta dataset or token changed, or its check is not current; it will be planned again.",
            ));
        }
        let at = q["event"]["event_time"].as_i64().unwrap_or(0);
        if Utc::now().timestamp() - at > MAX_AGE.num_seconds() {
            return Ok(Err(
                "Meta accepts events from the last 7 days only; it will be planned again.",
            ));
        }
        Ok(Ok(()))
    }
    /// Commit an attempt marker before crossing the network.
    pub async fn arm(mut self) -> Result<()> {
        sqlx::query("UPDATE meta_deliveries SET attempted_at=COALESCE(attempted_at,now()) WHERE workspace_id=$1 AND id=$2 AND state='sending'")
            .bind(self.workspace).bind(self.id).execute(&mut *self.tx).await?;
        self.tx.commit().await?;
        Ok(())
    }
    /// Not sent: cancel this request and let the planner decide again with current state.
    pub async fn stop(mut self, reason: &str) -> Result<()> {
        sqlx::query("UPDATE meta_deliveries SET state='canceled',attempted_at=NULL,due_at=NULL,diagnostic=$3,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND state='sending'")
            .bind(self.workspace).bind(self.id).bind(reason).execute(&mut *self.tx).await?;
        let event = self.delivery["event_id"]
            .as_str()
            .and_then(|v| v.parse::<Uuid>().ok());
        sqlx::query("UPDATE meta_event_plans SET state='blocked',reason=$3,plan_after=now(),updated_at=now() WHERE workspace_id=$1 AND event_id=$2 AND state='planned'")
            .bind(self.workspace).bind(event).bind(reason).execute(&mut *self.tx).await?;
        self.event(
            "meta_delivery_status",
            json!({"delivery_id":self.id,"state":"canceled","diagnostic":reason}),
        )
        .await?;
        self.tx.commit().await?;
        Ok(())
    }
    /// Record the outcome. `refused` clears the attempt marker: only for an explicit Meta
    /// refusal or a request that provably never left.
    pub async fn finish(
        mut self,
        state: &str,
        tracking: Option<&str>,
        diagnostic: &str,
        delay: Option<i64>,
        refused: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE meta_deliveries SET state=$3,tracking_id=COALESCE($4,tracking_id),diagnostic=$5,due_at=$6,attempted_at=CASE WHEN $7 THEN NULL ELSE attempted_at END,updated_at=now() WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace).bind(self.id).bind(state).bind(tracking).bind(diagnostic)
            .bind(delay.map(|s| Utc::now() + Duration::seconds(s))).bind(refused).execute(&mut *self.tx).await?;
        self.event(
            "meta_delivery_status",
            json!({"delivery_id":self.id,"state":state,"diagnostic":diagnostic}),
        )
        .await?;
        self.tx.commit().await?;
        Ok(())
    }
    async fn event(&mut self, kind: &str, payload: Value) -> Result<()> {
        record(&mut self.tx, self.workspace, kind, "worker", payload).await
    }
}
