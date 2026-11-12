//! Microsoft Advertising as a second destination for pipeline conversions: the OAuth
//! connection, the chosen ad account, per-action goal mappings, and the planner and
//! durable delivery rows for offline conversion uploads and adjustments. No provider I/O.
//!
//! Microsoft state lives in its own tables, so every Google query, row and status is
//! unchanged. The delivery switch, automatic sending and the consent policy are the shared
//! `conversion_delivery_settings`.
use crate::{
    Error, Result, Store,
    conversions::{self, Consent, Decision},
    google::Attempt,
    pipeline::{CLICK, NEW_LEAD_HOLD, record, sql},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use convert_domain::ValidationError;
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Row, Transaction};
use uuid::Uuid;

/// Microsoft accepts offline conversions for a new goal about two hours after creation.
pub const GOAL_READY: Duration = Duration::hours(2);
/// Offline conversions older than this are rejected (ConversionTime within 90 days).
const MAX_AGE: Duration = Duration::days(90);
const MAX_WINDOW_MINUTES: i64 = 129_600;

// Never serialize/debug credentials or pending OAuth state.
pub struct Connection {
    pub revision: i64,
    pub encrypted_token: Option<String>,
    pub client_fingerprint: Option<String>,
    pub connected_at: Option<DateTime<Utc>>,
    pub customer_id: Option<String>,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub destination_version: i32,
    pub validation: Option<Value>,
    pub validated_version: Option<i32>,
    pub checked_at: Option<DateTime<Utc>>,
    /// The provider rejected the refresh token for this revision; the owner must reconnect.
    pub auth_failed_at: Option<DateTime<Utc>>,
}
impl Connection {
    /// The saved check covers the current destination and is under 24 hours old.
    pub fn check_current(&self) -> bool {
        self.validation.is_some()
            && self.validated_version == Some(self.destination_version)
            && self
                .checked_at
                .is_some_and(|t| t > Utc::now() - Duration::hours(24))
    }
}
fn numeric(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}
pub fn goal_name(s: &str) -> bool {
    let n = s.chars().count();
    (1..=100).contains(&n) && !s.chars().any(char::is_control) && s.trim() == s
}
/// Reconsider blocked Microsoft plans now (all, or one action's).
/// Microsoft accepts offline conversions for clicks up to 90 days old.
const INCLUDE_SINCE_MAX_DAYS: i64 = 90;

pub(crate) async fn wake_plans(
    conn: &mut PgConnection,
    workspace: Uuid,
    action: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE microsoft_event_plans p SET plan_after=now() FROM pipeline_events e WHERE p.workspace_id=$1 AND p.state='blocked' AND e.workspace_id=p.workspace_id AND e.id=p.event_id AND ($2::text IS NULL OR e.action_key=$2)")
        .bind(workspace).bind(action).execute(conn).await?;
    Ok(())
}

impl Store {
    /// Record (or clear) that the provider rejected this revision's refresh token. A
    /// reconnect bumps the revision, so a late failure never flags the new connection.
    pub async fn set_microsoft_auth_failed(&self, revision: i64, failed: bool) -> Result<()> {
        sqlx::query(if failed {
            "UPDATE microsoft_connections SET auth_failed_at=now() WHERE workspace_id=$1 AND revision=$2 AND auth_failed_at IS NULL"
        } else {
            "UPDATE microsoft_connections SET auth_failed_at=NULL WHERE workspace_id=$1 AND revision=$2 AND auth_failed_at IS NOT NULL"
        })
        .bind(self.workspace)
        .bind(revision)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn microsoft_connection(&self) -> Result<Connection> {
        let row = sqlx::query("SELECT * FROM microsoft_connections WHERE workspace_id=$1")
            .bind(self.workspace)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => Connection {
                revision: r.get("revision"),
                encrypted_token: r.get("encrypted_token"),
                client_fingerprint: r.get("client_fingerprint"),
                connected_at: r.get("connected_at"),
                customer_id: r.get("customer_id"),
                account_id: r.get("account_id"),
                account_name: r.get("account_name"),
                destination_version: r.get("destination_version"),
                validation: r.get("validation"),
                validated_version: r.get("validated_version"),
                checked_at: r.get("checked_at"),
                auth_failed_at: r.get("auth_failed_at"),
            },
            None => Connection {
                revision: 0,
                encrypted_token: None,
                client_fingerprint: None,
                connected_at: None,
                customer_id: None,
                account_id: None,
                account_name: None,
                destination_version: 0,
                validation: None,
                validated_version: None,
                checked_at: None,
                auth_failed_at: None,
            },
        })
    }
    /// Microsoft rotates refresh tokens: persist each replacement, only while the same
    /// connection still holds the token it replaces.
    pub async fn rotate_microsoft_refresh(
        &self,
        revision: i64,
        old: &str,
        new: &str,
    ) -> Result<()> {
        let result=sqlx::query("UPDATE microsoft_connections SET encrypted_token=$4 WHERE workspace_id=$1 AND revision=$2 AND encrypted_token=$3")
            .bind(self.workspace).bind(revision).bind(old).bind(new).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub async fn begin_microsoft_oauth(&self, a: &Attempt) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        sqlx::query(
            "INSERT INTO microsoft_connections(workspace_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(self.workspace)
        .execute(&mut *tx)
        .await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM microsoft_connections WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_one(&mut *tx)
                .await?;
        if a.workspace != self.workspace || a.expected_revision != revision {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE microsoft_connections SET pending_state_hash=$2 WHERE workspace_id=$1")
            .bind(self.workspace)
            .bind(&a.state_hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM microsoft_oauth_attempts WHERE expires_at<=now()")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO microsoft_oauth_attempts(workspace_id,state_hash,browser_hash,encrypted_verifier,client_fingerprint,session_hash,operator_hash,actor,expected_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(workspace_id) DO UPDATE SET state_hash=$2,browser_hash=$3,encrypted_verifier=$4,client_fingerprint=$5,session_hash=$6,operator_hash=$7,actor=$8,expected_revision=$9,expires_at=now()+interval '10 minutes'")
            .bind(self.workspace).bind(&a.state_hash).bind(&a.browser_hash).bind(&a.encrypted_verifier).bind(&a.client_fingerprint).bind(&a.session_hash).bind(&a.operator_hash).bind(&a.actor).bind(a.expected_revision).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn consume_microsoft_oauth(
        &self,
        state: &str,
        browser: &str,
    ) -> Result<Option<Attempt>> {
        let row=sqlx::query("DELETE FROM microsoft_oauth_attempts WHERE state_hash=$1 AND browser_hash=$2 AND expires_at>now() RETURNING *").bind(state).bind(browser).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| Attempt {
            workspace: r.get("workspace_id"),
            state_hash: r.get("state_hash"),
            browser_hash: r.get("browser_hash"),
            encrypted_verifier: r.get("encrypted_verifier"),
            client_fingerprint: r.get("client_fingerprint"),
            session_hash: r.get("session_hash"),
            operator_hash: r.get("operator_hash"),
            actor: r.get("actor"),
            expected_revision: r.get("expected_revision"),
        }))
    }
    /// Save the new credential. The chosen ad account is kept but must be checked again
    /// with the new authorization; requests Microsoft refused for authorization (never
    /// applied) are queued again.
    pub async fn finish_microsoft_oauth(&self, a: &Attempt, encrypted: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if self.workspace != a.workspace {
            return Err(Error::Conflict);
        }
        if let Some(hash) = &a.session_hash {
            sqlx::query("SELECT user_id FROM hosted_users WHERE user_id=(SELECT user_id FROM hosted_sessions WHERE token_hash=$1) FOR UPDATE")
                .bind(hash).fetch_optional(&mut *tx).await?;
            let allowed:Option<bool>=sqlx::query_scalar("SELECT true FROM hosted_sessions s JOIN hosted_memberships m USING(workspace_id,user_id) JOIN hosted_users u USING(user_id) JOIN hosted_workspaces w USING(workspace_id) WHERE s.token_hash=$1 AND s.workspace_id=$2 AND s.expires_at>now() AND s.credential_version=u.credential_version AND m.enabled AND w.enabled AND m.role='owner' FOR SHARE OF s,m,w").bind(hash).bind(self.workspace).fetch_optional(&mut *tx).await?;
            if allowed != Some(true) {
                return Err(Error::Conflict);
            }
        }
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE microsoft_connections SET revision=revision+1,encrypted_token=$3,client_fingerprint=$4,connected_at=now(),auth_failed_at=NULL,validation=NULL,validated_version=NULL,checked_at=NULL,pending_state_hash=NULL WHERE workspace_id=$1 AND revision=$2 AND pending_state_hash=$5")
            .bind(self.workspace).bind(a.expected_revision).bind(encrypted).bind(&a.client_fingerprint).bind(&a.state_hash).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE microsoft_deliveries SET state='queued',due_at=now(),diagnostic='Queued again after Microsoft Advertising was reconnected.',updated_at=now() WHERE workspace_id=$1 AND state='needs_review' AND attempted_at IS NULL")
            .bind(self.workspace).execute(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "microsoft_connected",
            &a.actor,
            json!({}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Remove the credential. The chosen ad account and goal mappings are kept; nothing
    /// is sent until Microsoft Advertising is connected again.
    pub async fn disconnect_microsoft(&self, revision: i64, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE microsoft_connections SET revision=revision+1,encrypted_token=NULL,client_fingerprint=NULL,connected_at=NULL,auth_failed_at=NULL,validation=NULL,validated_version=NULL,checked_at=NULL,pending_state_hash=NULL WHERE workspace_id=$1 AND revision=$2")
            .bind(self.workspace).bind(revision).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("DELETE FROM microsoft_oauth_attempts WHERE workspace_id=$1")
            .bind(self.workspace)
            .execute(&mut *tx)
            .await?;
        record(
            &mut tx,
            self.workspace,
            "microsoft_disconnected",
            actor,
            json!({}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// The ad account that receives Microsoft conversions. Changing it invalidates the
    /// check; events are planned against the new account once it is checked.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_microsoft_destination(
        &self,
        revision: i64,
        destination_version: i32,
        customer_id: &str,
        account_id: &str,
        account_name: &str,
        actor: &str,
    ) -> Result<i32> {
        if !numeric(customer_id) || !numeric(account_id) {
            return Err(ValidationError("Use numeric Microsoft customer and account IDs").into());
        }
        let name: String = account_name.chars().take(300).collect();
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let version: i32 = sqlx::query_scalar("UPDATE microsoft_connections SET customer_id=$4,account_id=$5,account_name=$6,destination_version=destination_version+1,validation=NULL,validated_version=NULL,checked_at=NULL WHERE workspace_id=$1 AND revision=$2 AND destination_version=$3 AND encrypted_token IS NOT NULL RETURNING destination_version")
            .bind(self.workspace).bind(revision).bind(destination_version).bind(customer_id).bind(account_id).bind(&name).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        wake_plans(&mut tx, self.workspace, None).await?;
        record(&mut tx, self.workspace, "microsoft_destination_saved", actor, json!({"customer_id":customer_id,"account_id":account_id,"destination_version":version})).await?;
        tx.commit().await?;
        Ok(version)
    }
    /// Record a read-only destination check against the connection and destination it
    /// covered, so a replaced connection or a changed account conflicts.
    pub async fn save_microsoft_validation(
        &self,
        revision: i64,
        destination_version: i32,
        report: &Value,
        actor: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE microsoft_connections SET validation=$4,validated_version=$3,checked_at=now() WHERE workspace_id=$1 AND revision=$2 AND destination_version=$3 AND encrypted_token IS NOT NULL AND account_id IS NOT NULL")
            .bind(self.workspace).bind(revision).bind(destination_version).bind(report).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        wake_plans(&mut tx, self.workspace, None).await?;
        record(
            &mut tx,
            self.workspace,
            "microsoft_destination_checked",
            actor,
            json!({"valid":report["valid"],"destination_version":destination_version}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// A goal VisiConvert created for this action in this account: (ID, name, created).
    pub async fn created_microsoft_goal(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<(String, String, DateTime<Utc>)>> {
        let row = sqlx::query("SELECT goal_id,goal_name,created_at FROM microsoft_created_goals WHERE workspace_id=$1 AND account_id=$2 AND action_key=$3")
            .bind(self.workspace).bind(account).bind(key).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| (r.get("goal_id"), r.get("goal_name"), r.get("created_at"))))
    }
    /// Receipt saved right after Microsoft confirms (or a retry recovers) a goal. Its
    /// time starts the two-hour wait before uploads.
    pub async fn remember_microsoft_goal(
        &self,
        account: &str,
        key: &str,
        id: &str,
        name: &str,
        actor: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query("INSERT INTO microsoft_created_goals(workspace_id,account_id,action_key,goal_id,goal_name) VALUES($1,$2,$3,$4,$5) ON CONFLICT(workspace_id,account_id,action_key) DO UPDATE SET goal_name=EXCLUDED.goal_name WHERE microsoft_created_goals.goal_id=EXCLUDED.goal_id")
            .bind(self.workspace).bind(account).bind(key).bind(id).bind(name).execute(&mut *tx).await?;
        if r.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        record(
            &mut tx,
            self.workspace,
            "microsoft_goal_confirmed",
            actor,
            json!({"account_id":account,"action":key,"goal_id":id,"goal_name":name}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Goals VisiConvert created in this account less than two hours ago: name → ready at.
    pub async fn microsoft_goal_waits(
        &self,
        account: &str,
    ) -> Result<Vec<(String, DateTime<Utc>)>> {
        let rows = sqlx::query("SELECT goal_name,created_at FROM microsoft_created_goals WHERE workspace_id=$1 AND account_id=$2 AND created_at>now()-interval '2 hours'")
            .bind(self.workspace).bind(account).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get("goal_name"),
                    r.get::<DateTime<Utc>, _>("created_at") + GOAL_READY,
                )
            })
            .collect())
    }
    /// Map a pipeline action to a Microsoft offline conversion goal by name (uploads
    /// reference goals by name), or clear the mapping. A new or changed mapping starts
    /// now: events recorded earlier are never sent to Microsoft, so switching from
    /// another uploader cannot count them twice. `include_since` (owner's choice, at
    /// most 90 days back, only with a goal) moves that start earlier so events and
    /// Microsoft-click leads since then are sent too, for clicks no other uploader saw.
    /// Shares the action's version.
    /// A Microsoft conversion goal paused, reactivated, or included in or excluded from
    /// bidding through VisiConvert.
    pub async fn log_microsoft_goal_change(&self, actor: &str, payload: &Value) -> Result<()> {
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'microsoft_goal_changed',$2,$3)")
            .bind(self.workspace).bind(actor).bind(payload).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn set_microsoft_action_goal(
        &self,
        key: &str,
        goal: Option<&str>,
        include_since: Option<DateTime<Utc>>,
        version: i32,
        actor: &str,
    ) -> Result<Value> {
        if let Some(since) = include_since {
            let now = Utc::now();
            if goal.is_none() {
                return Err(ValidationError("include_since needs a goal_name").into());
            }
            if since > now || since < now - Duration::days(INCLUDE_SINCE_MAX_DAYS) {
                return Err(ValidationError(
                    "include_since must be in the past and at most 90 days ago (Microsoft's click window)",
                )
                .into());
            }
        }
        if goal.is_some_and(|g| !goal_name(g)) {
            return Err(ValidationError(
                "goal_name must be the Microsoft goal's exact name (1–100 characters)",
            )
            .into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let saved: Value = sqlx::query_scalar("UPDATE conversion_actions a SET microsoft_mapped_at=CASE WHEN $3::text IS NULL THEN NULL WHEN $5::timestamptz IS NOT NULL THEN $5 WHEN a.microsoft_goal_name IS DISTINCT FROM $3 THEN now() ELSE a.microsoft_mapped_at END,microsoft_goal_name=$3,version=version+1,updated_at=now() WHERE workspace_id=$1 AND key=$2 AND version=$4 RETURNING to_jsonb(a)-'workspace_id'-'id'")
            .bind(self.workspace).bind(key).bind(goal).bind(version).bind(include_since).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        if let Some(since) = include_since {
            // Events already finalized only because they predated the mapping are planned
            // again; anything that has a Microsoft request is left alone.
            sqlx::query("DELETE FROM microsoft_event_plans p USING pipeline_events e WHERE p.workspace_id=$1 AND p.state='final' AND p.reason LIKE 'Recorded before this action was mapped%' AND e.workspace_id=p.workspace_id AND e.id=p.event_id AND e.action_key=$2 AND e.created_at>=$3 AND NOT EXISTS(SELECT 1 FROM microsoft_deliveries d WHERE d.workspace_id=p.workspace_id AND d.event_id=p.event_id)")
                .bind(self.workspace).bind(key).bind(since).execute(&mut *tx).await?;
        }
        wake_plans(&mut tx, self.workspace, Some(key)).await?;
        record(
            &mut tx,
            self.workspace,
            "microsoft_action_goal_saved",
            actor,
            json!({"key":key,"version":saved["version"],"goal_name":goal,"include_since":include_since}),
        )
        .await?;
        tx.commit().await?;
        Ok(saved)
    }
    /// Facts for the Microsoft section of the setup checklist.
    pub async fn microsoft_setup(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object(
            'enabled_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled),
            'mapped_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled AND microsoft_goal_name IS NOT NULL),
            'goals',COALESCE((SELECT jsonb_agg(jsonb_build_object('action',key,'goal_name',microsoft_goal_name,'mapped_at',microsoft_mapped_at) ORDER BY key) FROM conversion_actions WHERE workspace_id=$1 AND enabled AND microsoft_goal_name IS NOT NULL),'[]'::jsonb),
            'allow_inferred_calls',COALESCE((SELECT (configuration->>'allow_inferred_calls')::boolean FROM conversion_settings WHERE workspace_id=$1),false),
            'dni_calls',EXISTS(SELECT 1 FROM dni_pools WHERE workspace_id=$1) OR EXISTS(SELECT 1 FROM dni_call_evidence WHERE workspace_id=$1 AND status='inferred_window'),
            'events_sent',(SELECT count(*) FROM microsoft_deliveries WHERE workspace_id=$1 AND state='processed'),
            'events_needing_review',(SELECT count(*) FROM microsoft_deliveries WHERE workspace_id=$1 AND state IN ('failed','needs_review','uncertain')))")
            .bind(self.workspace).fetch_one(&self.pool).await?)
    }
    /// Recent Microsoft requests, newest first. Requests never contain credentials.
    pub async fn microsoft_deliveries(&self) -> Result<Vec<Value>> {
        Ok(sqlx::query_scalar("SELECT to_jsonb(d)-'workspace_id'-'snapshot'-'request' || jsonb_build_object('action',d.snapshot->'action_key','external_id',d.snapshot->'external_id','goal_name',d.request->'goal_name') FROM microsoft_deliveries d WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 200")
            .bind(self.workspace).fetch_all(&self.pool).await?)
    }
    /// Workspaces with Microsoft work: mapped actions, due plans or queued requests.
    pub async fn microsoft_workspaces(&self) -> Result<Vec<Uuid>> {
        Ok(sqlx::query_scalar("SELECT workspace_id FROM conversion_actions WHERE enabled AND microsoft_goal_name IS NOT NULL UNION SELECT workspace_id FROM microsoft_event_plans WHERE state='blocked' AND plan_after<=now() UNION SELECT e.workspace_id FROM microsoft_event_plans p JOIN pipeline_events e ON e.workspace_id=p.workspace_id AND e.id=p.event_id WHERE p.revision<>e.revision LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    /// Whether the worker should repeat the destination check: events or requests are
    /// waiting and the check is missing, older than 24 hours, or lacks a mapped goal.
    pub async fn microsoft_refresh_due(&self) -> Result<bool> {
        let c = self.microsoft_connection().await?;
        if c.encrypted_token.is_none() || c.account_id.is_none() {
            return Ok(false);
        }
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM microsoft_event_plans WHERE workspace_id=$1 AND state='blocked' AND plan_after<=now()) OR EXISTS(SELECT 1 FROM microsoft_deliveries WHERE workspace_id=$1 AND state IN ('queued','retry')) OR EXISTS(SELECT 1 FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key LEFT JOIN microsoft_event_plans p ON p.workspace_id=e.workspace_id AND p.event_id=e.id WHERE e.workspace_id=$1 AND a.microsoft_goal_name IS NOT NULL AND e.sendable AND p.event_id IS NULL AND e.created_at>=a.microsoft_mapped_at)")
            .bind(self.workspace).fetch_one(&self.pool).await?;
        if !waiting {
            return Ok(false);
        }
        let age = c.checked_at.map_or(Duration::days(365), |t| Utc::now() - t);
        let names: Vec<String> = sqlx::query_scalar("SELECT DISTINCT microsoft_goal_name FROM conversion_actions WHERE workspace_id=$1 AND enabled AND microsoft_goal_name IS NOT NULL")
            .bind(self.workspace).fetch_all(&self.pool).await?;
        let report = c.validation.clone().unwrap_or_default();
        let missing = names.iter().any(|n| {
            !report["goals"]
                .as_array()
                .is_some_and(|g| g.iter().any(|v| v["name"] == n.as_str()))
        });
        Ok(!c.check_current() || (missing && age > Duration::minutes(10)))
    }
}

/// `delivery.microsoft` for one event row of the pipeline event query, or null when the
/// event was never considered for Microsoft.
pub(crate) fn event_status(r: &sqlx::postgres::PgRow, revision: i32) -> Value {
    let Some(plan_revision) = r.get::<Option<i32>, _>("ms_plan_revision") else {
        return Value::Null;
    };
    let plan: String = r.get("ms_plan_state");
    let raw: Option<String> = r.get("ms_delivery_state");
    let kind: Option<String> = r.get("ms_kind");
    let diagnostic: Option<String> = r.get("ms_diagnostic");
    let (status, reason) = if plan_revision != revision {
        ("pending", Some("Waiting for the delivery worker".into()))
    } else if plan != "planned" {
        if r.get::<String, _>("status") == "canceled" && raw.as_deref() != Some("processed") {
            ("canceled", r.get("ms_reason"))
        } else {
            ("blocked", r.get("ms_reason"))
        }
    } else {
        match raw.as_deref() {
            Some("queued" | "retry" | "sending") => ("queued", diagnostic),
            Some("processed") if kind.as_deref() == Some("retract") => ("retracted", None),
            Some("processed") => ("delivered", diagnostic),
            Some("uncertain") => ("uncertain", diagnostic),
            Some("canceled") => ("canceled", diagnostic),
            None => ("blocked", r.get("ms_reason")),
            _ => ("failed", diagnostic),
        }
    };
    json!({"status":status,"reason":reason,"delivery_id":r.get::<Option<Uuid>,_>("ms_delivery_id"),"delivery_state":raw,"kind":kind,
        "delivery_revision":r.get::<Option<i32>,_>("ms_delivery_revision"),"tracking_id":r.get::<Option<String>,_>("ms_tracking_id")})
}

enum Plan {
    /// A new request was queued.
    Queued,
    /// Nothing new to send; the latest request stands.
    Unchanged,
    Final(String),
    Blocked(String, i32),
}
fn utc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn msclkid(evidence: &Value) -> Option<String> {
    evidence
        .get("msclkid")
        .and_then(Value::as_str)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 200
                && !v.chars().any(|c| c.is_control() || c.is_whitespace())
        })
        .map(str::to_string)
}
impl Store {
    /// Turn due event revisions into queued Microsoft requests (an upload, or a restate
    /// or retraction of an accepted upload), or record why not. Holds the workspace lock
    /// used by every edit. Independent of the Google plan for the same event.
    pub async fn plan_microsoft(&self, only: Option<Uuid>) -> Result<u64> {
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
        let connection = sqlx::query("SELECT revision,customer_id,account_id,destination_version,validation,validated_version,checked_at FROM microsoft_connections WHERE workspace_id=$1 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).fetch_optional(&mut *tx).await?;
        let waits: Vec<(String, DateTime<Utc>)> = match connection
            .as_ref()
            .and_then(|c| c.get::<Option<String>, _>("account_id"))
        {
            Some(account) => sqlx::query("SELECT goal_name,created_at FROM microsoft_created_goals WHERE workspace_id=$1 AND account_id=$2")
                .bind(self.workspace).bind(account).fetch_all(&mut *tx).await?
                .iter().map(|r| (r.get("goal_name"), r.get("created_at"))).collect(),
            None => vec![],
        };
        let rows = sqlx::query(sql(format!("SELECT e.id,e.action_key,e.external_id,e.lead_id,e.value_minor,e.occurred_at,e.created_at,e.revision,e.status,e.sendable,
            a.enabled AS action_enabled,a.microsoft_goal_name,a.microsoft_mapped_at,a.counting,a.trigger,l.channel,l.is_demo,i.qualification,
            COALESCE(ws.evidence,ds.evidence) AS evidence,COALESCE(ws.visit_at,ds.visit_at) AS visit_at,cc.evidence AS consent,cc.version AS consent_version,
            p.entity_key
            FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key
            LEFT JOIN microsoft_event_plans p ON p.workspace_id=e.workspace_id AND p.event_id=e.id
            LEFT JOIN leads l ON l.workspace_id=e.workspace_id AND l.id=e.lead_id
            LEFT JOIN inquiries i ON i.workspace_id=l.workspace_id AND i.id=l.inquiry_id
            {CLICK}
            LEFT JOIN conversion_consents cc ON cc.workspace_id=e.workspace_id AND cc.lead_id=e.lead_id
            WHERE e.workspace_id=$1 AND ($2::uuid IS NULL OR e.id=$2) AND (
              (p.event_id IS NULL AND a.microsoft_goal_name IS NOT NULL AND e.updated_at>=a.microsoft_mapped_at)
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
                waits: &waits,
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
            sqlx::query("INSERT INTO microsoft_event_plans(workspace_id,event_id,revision,state,reason,plan_after,entity_key) VALUES($1,$2,$3,$4,$5,now()+make_interval(mins=>$6),$7)
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
    waits: &'a [(String, DateTime<Utc>)],
    now: DateTime<Utc>,
}
/// One event revision. Returns the outcome and the entity key the event holds after it
/// (the click or event identity of a request that may have reached Microsoft).
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
    sqlx::query("UPDATE microsoft_deliveries SET state='canceled',due_at=NULL,diagnostic='Replaced by a newer revision of the event before it was sent.',updated_at=now() WHERE workspace_id=$1 AND event_id=$2 AND event_revision<$3 AND attempted_at IS NULL AND state IN ('queued','retry')")
        .bind(ws).bind(id).bind(revision).execute(&mut **tx).await?;
    let history = sqlx::query("SELECT kind,state,request,attempted_at FROM microsoft_deliveries WHERE workspace_id=$1 AND event_id=$2 AND state<>'canceled' ORDER BY created_at")
        .bind(ws).bind(id).fetch_all(&mut **tx).await?;
    let sent = history
        .iter()
        .any(|h| h.get::<Option<DateTime<Utc>>, _>("attempted_at").is_some());
    // The key stays with an event whose request may have reached Microsoft.
    let release = |plan: Plan| (plan, if sent { held_key.clone() } else { None });
    if history.iter().any(|h| {
        matches!(
            h.get::<String, _>("state").as_str(),
            "queued" | "retry" | "sending"
        )
    }) {
        return Ok((
            Plan::Blocked(
                "Waiting for the previous Microsoft request for this event to finish".into(),
                2,
            ),
            held_key,
        ));
    }
    let accepted: Vec<(String, Value)> = history
        .iter()
        .filter(|h| h.get::<String, _>("state") == "processed")
        .map(|h| (h.get("kind"), h.get("request")))
        .collect();
    let uncertain = history
        .iter()
        .any(|h| h.get::<String, _>("state") == "uncertain");
    let canceled = r.get::<String, _>("status") != "active";
    if accepted.iter().any(|(k, _)| k == "retract") {
        return Ok((
            Plan::Final(
                "Retracted from Microsoft Advertising; post a new external_id to count it again"
                    .into(),
            ),
            held_key,
        ));
    }
    if let Some((_, upload)) = accepted.iter().find(|(k, _)| k == "upload") {
        if uncertain {
            return Ok((Plan::Final("A later Microsoft request for this event may have been applied; it is not repeated automatically. Check the conversion in Microsoft Advertising.".into()), held_key));
        }
        return adjust(tx, c, r, upload, &accepted, canceled)
            .await
            .map(|p| (p, held_key));
    }
    if uncertain {
        return Ok((Plan::Final("An earlier upload of this event may have reached Microsoft; it is not sent again automatically. Check the conversion in Microsoft Advertising.".into()), held_key));
    }
    if canceled {
        return Ok(release(Plan::Final(
            "Canceled before it was sent to Microsoft".into(),
        )));
    }
    match eligible(c, r)? {
        Err(plan) => Ok(release(plan)),
        Ok(upload) => {
            let action_key: String = r.get("action_key");
            let entity_key = if r.get::<String, _>("counting") == "once_per_click" {
                format!(
                    "{action_key}:click:{}",
                    &conversions::digest(&json!(upload.click))[..32]
                )
            } else {
                format!("{action_key}:event:{}", r.get::<String, _>("external_id"))
            };
            if sent && held_key.as_deref() != Some(entity_key.as_str()) {
                return Ok((
                    Plan::Final(
                        "Already sent to Microsoft for a different click; its lead cannot change"
                            .into(),
                    ),
                    held_key,
                ));
            }
            let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM microsoft_event_plans WHERE workspace_id=$1 AND entity_key=$2 AND event_id<>$3)")
                .bind(ws).bind(&entity_key).bind(id).fetch_one(&mut **tx).await?;
            if taken {
                return Ok(release(Plan::Final(
                    "Already counted for this Microsoft ad click (counted once per click)".into(),
                )));
            }
            let snapshot = json!({"kind":"pipeline","provider":"microsoft","event_id":id,"action_key":action_key,"external_id":r.get::<String,_>("external_id"),
                "revision":revision,"lead_id":r.get::<Option<Uuid>,_>("lead_id"),"channel":r.get::<Option<String>,_>("channel"),"event_at":r.get::<DateTime<Utc>,_>("occurred_at"),
                "visit_at":upload.visit,"value_minor":r.get::<Option<i64>,_>("value_minor"),"currency":"USD","consent_policy":c.consent_policy,
                "consent_version":r.get::<Option<i32>,_>("consent_version").unwrap_or(0),"counting":r.get::<String,_>("counting"),
                "goal_name":upload.request["goal_name"],"customer_id":upload.request["customer_id"],"account_id":upload.request["account_id"],
                "confidence":if r.get::<Option<String>,_>("channel").as_deref() == Some("call") {"inferred"} else {"browser-reported"}});
            queue(tx, c, id, revision, "upload", &snapshot, &upload.request).await?;
            Ok((Plan::Queued, Some(entity_key)))
        }
    }
}
struct Upload {
    request: Value,
    click: String,
    visit: DateTime<Utc>,
}
/// Upload eligibility, mirroring the Google planner's rules for the same event.
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
    let (Some(goal), Some(mapped_at)) = (
        r.get::<Option<String>, _>("microsoft_goal_name"),
        r.get::<Option<DateTime<Utc>>, _>("microsoft_mapped_at"),
    ) else {
        return finished("This action is not mapped to a Microsoft goal");
    };
    if created_at < mapped_at
        || (r.get::<String, _>("trigger") == "new_lead" && event_at < mapped_at)
    {
        return finished(
            "Recorded before this action was mapped to its Microsoft goal; never sent to Microsoft, so an earlier uploader cannot be counted twice",
        );
    }
    if !r.get::<bool, _>("action_enabled") {
        return blocked("Action is disabled", 15);
    }
    if r.get::<Option<Uuid>, _>("lead_id").is_none() {
        return finished("No attributed lead matched this event");
    }
    if r.get::<Option<bool>, _>("is_demo") == Some(true) {
        return finished("Demo data is never sent");
    }
    if r.get::<Option<String>, _>("qualification").as_deref() == Some("spam") {
        return blocked("The inquiry is marked spam", 15);
    }
    if r.get::<String, _>("trigger") == "new_lead" {
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
    let evidence: Value = r.get::<Option<Value>, _>("evidence").unwrap_or_default();
    let visit_at: Option<DateTime<Utc>> = r.get("visit_at");
    let (Some(click), Some(visit)) = (msclkid(&evidence), visit_at) else {
        // Click evidence for a call can arrive shortly after the event; then stop.
        return if now - event_at > Duration::days(1) {
            finished("No Microsoft Advertising click (msclkid) on this lead's visit")
        } else {
            blocked(
                "No Microsoft Advertising click (msclkid) on this lead's visit",
                60,
            )
        };
    };
    if r.get::<Option<String>, _>("channel").as_deref() == Some("call")
        && !c.settings.allow_inferred_calls
    {
        return blocked(
            "Inferred call attribution requires explicit opt-in (allow_inferred_calls in the destination settings)",
            15,
        );
    }
    if event_at < visit {
        return finished("Event predates the ad visit");
    }
    if now - event_at > MAX_AGE {
        return finished("Microsoft accepts offline conversions from the last 90 days only");
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
        return blocked("Connect Microsoft Advertising to send", 15);
    };
    let (Some(customer), Some(account)) = (
        m.get::<Option<String>, _>("customer_id"),
        m.get::<Option<String>, _>("account_id"),
    ) else {
        return blocked("Choose the Microsoft Advertising account first", 15);
    };
    let report: Value = m.get::<Option<Value>, _>("validation").unwrap_or_default();
    let checked: Option<DateTime<Utc>> = m.get("checked_at");
    if m.get::<Option<i32>, _>("validated_version") != Some(m.get("destination_version"))
        || !checked.is_some_and(|t| t > now - Duration::hours(24))
    {
        return blocked("Waiting for the Microsoft destination check", 2);
    }
    if report["account"]["status"] != "Active" {
        return blocked(
            "The Microsoft destination check failed; see Conversions",
            10,
        );
    }
    let Some(found) = report["goals"]
        .as_array()
        .and_then(|g| g.iter().find(|v| v["name"] == goal.as_str()))
    else {
        return blocked(
            "The mapped Microsoft goal was not found in the ad account; check the goal name",
            15,
        );
    };
    if found["eligible"] != true {
        return blocked(
            "The mapped Microsoft goal is not an active offline conversion goal",
            15,
        );
    }
    if let Some((_, created)) = c.waits.iter().find(|(n, _)| *n == goal) {
        let wait = *created + GOAL_READY - now;
        if wait > Duration::zero() {
            let minutes = i32::try_from(wait.num_minutes() + 1).unwrap_or(i32::MAX);
            return blocked(
                "Microsoft accepts offline conversions for a new goal about 2 hours after it is created",
                minutes,
            );
        }
    }
    let window = found["window_minutes"]
        .as_i64()
        .filter(|w| *w > 0)
        .unwrap_or(MAX_WINDOW_MINUTES)
        .min(MAX_WINDOW_MINUTES);
    if event_at - visit > Duration::minutes(window) {
        return finished("Outside the Microsoft goal's conversion window");
    }
    let request = json!({"kind":"upload","customer_id":customer,"account_id":account,"goal_name":goal,"click":click,
        "conversion_time":utc(event_at),"value_minor":r.get::<Option<i64>,_>("value_minor"),"currency":"USD"});
    Ok(Ok(Upload {
        request,
        click,
        visit,
    }))
}
/// Restate the value of, or retract, an accepted upload. Adjustments always name the
/// original upload's account, goal, click and conversion time.
async fn adjust(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context<'_>,
    r: &sqlx::postgres::PgRow,
    upload: &Value,
    accepted: &[(String, Value)],
    canceled: bool,
) -> Result<Plan> {
    use Plan::*;
    let id: Uuid = r.get("id");
    let revision: i32 = r.get("revision");
    if !c.enabled || !c.auto_send {
        return Ok(Blocked("Automatic sending is off".into(), 60));
    }
    if c.connection.is_none() {
        return Ok(Blocked(
            "Connect Microsoft Advertising to update the sent conversion".into(),
            15,
        ));
    }
    let value: Option<i64> = r.get("value_minor");
    let latest = accepted
        .iter()
        .rev()
        .find(|(k, _)| k == "upload" || k == "restate")
        .map(|(_, v)| v["value_minor"].as_i64())
        .unwrap_or(None);
    let kind = if canceled {
        "retract"
    } else {
        let evidence: Value = r.get::<Option<Value>, _>("evidence").unwrap_or_default();
        let event_at: DateTime<Utc> = r.get("occurred_at");
        if msclkid(&evidence).as_deref() != upload["click"].as_str()
            || Some(utc(event_at).as_str()) != upload["conversion_time"].as_str()
        {
            return Ok(Final("Only the value of a conversion sent to Microsoft can change. Its time and click stay as first sent.".into()));
        }
        if value == latest {
            return Ok(Unchanged);
        }
        if value.is_none() {
            return Ok(Final(
                "Microsoft cannot restate a sent conversion to no value; set a value or cancel the event to retract it".into(),
            ));
        }
        "restate"
    };
    let first_time = upload["conversion_time"]
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
        .ok_or(Error::Conflict)?;
    // Not in the future, and not before the conversion it adjusts.
    let at = (c.now - Duration::seconds(60)).max(first_time);
    let mut request = upload.clone();
    request["kind"] = json!(kind);
    request["adjustment_time"] = json!(utc(at));
    request["value_minor"] = if canceled { Value::Null } else { json!(value) };
    let snapshot = json!({"kind":"pipeline","provider":"microsoft","event_id":id,"action_key":r.get::<String,_>("action_key"),
        "external_id":r.get::<String,_>("external_id"),"revision":revision,"value_minor":value,"currency":"USD",
        "goal_name":upload["goal_name"],"customer_id":upload["customer_id"],"account_id":upload["account_id"],"adjusts":kind});
    queue(tx, c, id, revision, kind, &snapshot, &request).await?;
    Ok(Queued)
}
async fn queue(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context<'_>,
    event: Uuid,
    revision: i32,
    kind: &str,
    snapshot: &Value,
    request: &Value,
) -> Result<()> {
    let connection: i64 = c.connection.map_or(0, |m| m.get("revision"));
    let delivery = Uuid::new_v4();
    sqlx::query("INSERT INTO microsoft_deliveries(id,workspace_id,event_id,event_revision,kind,connection_revision,snapshot,request,state,due_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'queued',now())")
        .bind(delivery).bind(c.workspace).bind(event).bind(revision).bind(kind).bind(connection).bind(snapshot).bind(request).execute(&mut **tx).await?;
    record(
        tx,
        c.workspace,
        "microsoft_delivery_queued",
        "planner",
        json!({"delivery_id":delivery,"event_id":event,"revision":revision,"kind":kind}),
    )
    .await
}

/// A claimed Microsoft request, holding the workspace lock used by every edit.
pub struct Guard {
    pub delivery: Value,
    tx: Transaction<'static, Postgres>,
    workspace: Uuid,
    id: Uuid,
}
impl Store {
    /// Claim one due request. A request interrupted after it was armed is uncertain and
    /// never replayed; one interrupted before it left is queued again.
    pub async fn claim_microsoft_delivery(&self) -> Result<Option<(Uuid, Uuid)>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE microsoft_deliveries SET state=CASE WHEN attempted_at IS NULL THEN 'retry' ELSE 'uncertain' END,diagnostic=CASE WHEN attempted_at IS NULL THEN diagnostic ELSE 'Worker interrupted while sending; check the conversion in Microsoft Advertising. It is not sent again automatically.' END,due_at=CASE WHEN attempted_at IS NULL THEN now() END,updated_at=now() WHERE state='sending' AND due_at<now()")
            .execute(&mut *tx).await?;
        // Requests wait, still queued, while their workspace has automatic sending off.
        let row=sqlx::query("SELECT d.id,d.workspace_id FROM microsoft_deliveries d WHERE d.state IN ('queued','retry') AND d.due_at<=now() AND EXISTS(SELECT 1 FROM conversion_delivery_settings s WHERE s.workspace_id=d.workspace_id AND s.enabled AND s.auto_send) ORDER BY d.due_at,d.id FOR UPDATE OF d SKIP LOCKED LIMIT 1")
            .fetch_optional(&mut *tx).await?;
        let Some(r) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let id: Uuid = r.get("id");
        sqlx::query("UPDATE microsoft_deliveries SET state='sending',attempts=attempts+1,due_at=now()+interval '2 minutes',updated_at=now() WHERE id=$1")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some((r.get("workspace_id"), id)))
    }
    pub async fn microsoft_guard(&self, id: Uuid) -> Result<Guard> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let delivery: Value = sqlx::query_scalar("SELECT to_jsonb(d) FROM microsoft_deliveries d WHERE workspace_id=$1 AND id=$2 AND state='sending' FOR UPDATE")
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
    /// Recheck the event, mapping, policy, consent, connection and destination check
    /// under the lock. `Err` carries why the request must not be sent now.
    pub async fn fresh(&mut self) -> Result<std::result::Result<(), &'static str>> {
        let d = &self.delivery;
        let s = &d["snapshot"];
        let q = &d["request"];
        let kind = d["kind"].as_str().unwrap_or("");
        let event = d["event_id"]
            .as_str()
            .and_then(|v| v.parse::<Uuid>().ok())
            .ok_or(Error::Conflict)?;
        let row = sqlx::query("SELECT e.revision,e.status,e.lead_id,a.enabled AS action_enabled,a.microsoft_goal_name,COALESCE(s.enabled AND s.auto_send,false) AS sending,COALESCE(s.consent_policy,'per_lead') AS consent_policy,
            (SELECT version FROM conversion_consents cc WHERE cc.workspace_id=e.workspace_id AND cc.lead_id=e.lead_id) AS consent_version,
            NOT EXISTS(SELECT 1 FROM hosted_workspaces h WHERE h.workspace_id=e.workspace_id AND NOT h.enabled) AS hosted,
            c.encrypted_token IS NOT NULL AS connected,c.account_id,c.validation,c.validated_version=c.destination_version AND c.checked_at>now()-interval '24 hours' AS checked
            FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key
            LEFT JOIN conversion_delivery_settings s ON s.workspace_id=e.workspace_id LEFT JOIN microsoft_connections c ON c.workspace_id=e.workspace_id
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
        if !row.get::<bool, _>("connected") {
            return Ok(Err("Microsoft Advertising is not connected."));
        }
        let status: String = row.get("status");
        if kind == "retract" {
            return Ok(if status == "canceled" {
                Ok(())
            } else {
                Err("The event is active again; nothing to retract.")
            });
        }
        if status != "active" {
            return Ok(Err("The event was canceled before it was sent."));
        }
        if kind == "restate" {
            return Ok(Ok(()));
        }
        let policy: String = row.get("consent_policy");
        if !row.get::<bool, _>("action_enabled")
            || row
                .get::<Option<String>, _>("microsoft_goal_name")
                .as_deref()
                != q["goal_name"].as_str()
            || json!(row.get::<Option<Uuid>, _>("lead_id")) != s["lead_id"]
            || policy != s["consent_policy"]
            || (policy == "per_lead"
                && json!(row.get::<Option<i32>, _>("consent_version").unwrap_or(0))
                    != s["consent_version"])
        {
            return Ok(Err(
                "The action, goal mapping, lead, consent or policy changed before it was sent; it will be planned again.",
            ));
        }
        let report: Value = row
            .get::<Option<Value>, _>("validation")
            .unwrap_or_default();
        if row.get::<Option<String>, _>("account_id").as_deref() != q["account_id"].as_str()
            || row.get::<Option<bool>, _>("checked") != Some(true)
            || report["account"]["status"] != "Active"
            || !report["goals"].as_array().is_some_and(|g| {
                g.iter()
                    .any(|v| v["name"] == q["goal_name"] && v["eligible"] == true)
            })
        {
            return Ok(Err(
                "The Microsoft destination changed or its check is not current; it will be planned again.",
            ));
        }
        Ok(Ok(()))
    }
    /// Commit an attempt marker before crossing the network. A crash afterwards is
    /// conservatively uncertain, never an automatic duplicate.
    pub async fn arm(mut self) -> Result<()> {
        sqlx::query("UPDATE microsoft_deliveries SET attempted_at=COALESCE(attempted_at,now()) WHERE workspace_id=$1 AND id=$2 AND state='sending'")
            .bind(self.workspace).bind(self.id).execute(&mut *self.tx).await?;
        self.tx.commit().await?;
        Ok(())
    }
    /// Not sent: cancel this request and let the planner decide again with current state.
    pub async fn stop(mut self, reason: &str) -> Result<()> {
        sqlx::query("UPDATE microsoft_deliveries SET state='canceled',attempted_at=NULL,due_at=NULL,diagnostic=$3,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND state='sending'")
            .bind(self.workspace).bind(self.id).bind(reason).execute(&mut *self.tx).await?;
        let event = self.delivery["event_id"]
            .as_str()
            .and_then(|v| v.parse::<Uuid>().ok());
        sqlx::query("UPDATE microsoft_event_plans SET state='blocked',reason=$3,plan_after=now(),updated_at=now() WHERE workspace_id=$1 AND event_id=$2 AND state='planned'")
            .bind(self.workspace).bind(event).bind(reason).execute(&mut *self.tx).await?;
        self.event(
            "microsoft_delivery_status",
            json!({"delivery_id":self.id,"state":"canceled","diagnostic":reason}),
        )
        .await?;
        self.tx.commit().await?;
        Ok(())
    }
    /// Record the outcome. `refused` clears the attempt marker: only for an explicit
    /// Microsoft refusal or a request that provably never left.
    pub async fn finish(
        mut self,
        state: &str,
        tracking: Option<&str>,
        diagnostic: &str,
        delay: Option<i64>,
        refused: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE microsoft_deliveries SET state=$3,tracking_id=COALESCE($4,tracking_id),diagnostic=$5,due_at=$6,attempted_at=CASE WHEN $7 THEN NULL ELSE attempted_at END,updated_at=now() WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace).bind(self.id).bind(state).bind(tracking).bind(diagnostic)
            .bind(delay.map(|s| Utc::now() + Duration::seconds(s))).bind(refused).execute(&mut *self.tx).await?;
        self.event(
            "microsoft_delivery_status",
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
