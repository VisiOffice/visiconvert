//! Durable delivery intents, workspace serialization and immutable provider evidence.
use crate::{Error, Result, Store, conversions};
use chrono::{Duration, Utc};
use convert_domain::ValidationError;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub struct Guard {
    pub job: Value,
    pub delivery: Option<Value>,
    tx: Transaction<'static, Postgres>,
    workspace: Uuid,
}
impl Guard {
    pub async fn validated(
        mut self,
        request: Value,
        revision: i64,
        warnings: bool,
        actor: &str,
    ) -> Result<Value> {
        let id = self.job["id"]
            .as_str()
            .ok_or(Error::Conflict)?
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        let version = self.job["version"].as_i64().ok_or(Error::Conflict)? as i32;
        // An accepted, uncertain, or in-flight revision cannot be overwritten.
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND (state IN ('sending','submitted','polling','uncertain','queued','retry') OR (attempted_at IS NOT NULL AND state <> 'processed')))")
            .bind(self.workspace).bind(id).fetch_one(&mut *self.tx).await?;
        if active {
            return Err(ValidationError(
                "Resolve the previous delivery before validating another revision",
            )
            .into());
        }
        let prior:Option<Value>=sqlx::query_scalar("SELECT request FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NOT NULL ORDER BY created_at LIMIT 1")
            .bind(self.workspace).bind(id).fetch_optional(&mut *self.tx).await?;
        // Mapping comparison is done in the provider adapter; retain evidence here.
        if prior.is_some() && self.job["kind"] != "booking" {
            return Err(ValidationError(
                "This inquiry was already sent. Review it in Google; it cannot be counted again.",
            )
            .into());
        }
        let v:Value=sqlx::query_scalar("INSERT INTO conversion_deliveries AS d(id,workspace_id,outbox_id,outbox_version,connection_revision,snapshot,request,state,warnings) VALUES($1,$2,$3,$4,$5,$6,$7,'validated',$8) ON CONFLICT(workspace_id,outbox_id,outbox_version) DO UPDATE SET state='validated',validated_at=now(),connection_revision=$5,request=$7,snapshot=$6,warnings=$8,diagnostic=NULL,review_required=false WHERE d.attempted_at IS NULL AND d.state NOT IN ('queued','sending','polling') RETURNING to_jsonb(d)-'workspace_id'-'request'-'snapshot'")
            .bind(Uuid::new_v4()).bind(self.workspace).bind(id).bind(version).bind(revision).bind(&self.job["snapshot"]).bind(request).bind(warnings).fetch_optional(&mut *self.tx).await?.ok_or(Error::Conflict)?;
        self.event("delivery_validated", actor, json!({"delivery_id":v["id"]}))
            .await?;
        self.tx.commit().await?;
        Ok(v)
    }
    /// Commit an attempt marker before crossing the network. A crash afterwards is
    /// conservatively uncertain, never an automatic duplicate upload.
    pub async fn arm(mut self) -> Result<()> {
        let id = self.delivery.as_ref().ok_or(Error::Conflict)?["id"]
            .as_str()
            .ok_or(Error::Conflict)?
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        sqlx::query("UPDATE conversion_deliveries SET attempted_at=COALESCE(attempted_at,now()) WHERE workspace_id=$1 AND id=$2 AND state='sending'").bind(self.workspace).bind(id).execute(&mut *self.tx).await?;
        self.tx.commit().await?;
        Ok(())
    }
    pub async fn previous_request(&mut self) -> Result<Option<Value>> {
        let id = self.job["id"]
            .as_str()
            .ok_or(Error::Conflict)?
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        Ok(sqlx::query_scalar("SELECT request FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NOT NULL ORDER BY created_at LIMIT 1").bind(self.workspace).bind(id).fetch_optional(&mut *self.tx).await?)
    }
    /// Only for a confirmed no-send or explicit HTTP rejection, never an
    /// ambiguous response or asynchronous processing error.
    pub async fn not_accepted(mut self, diagnostic: &str) -> Result<()> {
        let id = self.delivery.as_ref().ok_or(Error::Conflict)?["id"]
            .as_str()
            .ok_or(Error::Conflict)?
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        sqlx::query("UPDATE conversion_deliveries SET attempted_at=NULL WHERE workspace_id=$1 AND id=$2 AND state='sending' AND request_id IS NULL").bind(self.workspace).bind(id).execute(&mut *self.tx).await?;
        self.finish("needs_review", None, diagnostic, false, None)
            .await
    }
    pub async fn finish(
        mut self,
        state: &str,
        request_id: Option<&str>,
        diagnostic: &str,
        warnings: bool,
        delay: Option<i64>,
    ) -> Result<()> {
        let id = self.delivery.as_ref().ok_or(Error::Conflict)?["id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        sqlx::query("UPDATE conversion_deliveries SET state=$3,request_id=COALESCE($4,request_id),diagnostic=$5,warnings=warnings OR $6,due_at=$7,updated_at=now() WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace).bind(id).bind(state).bind(request_id).bind(diagnostic).bind(warnings).bind(delay.map(|s|Utc::now()+Duration::seconds(s))).execute(&mut *self.tx).await?;
        self.event(
            "delivery_status",
            "worker",
            json!({"delivery_id":id,"state":state,"diagnostic":diagnostic}),
        )
        .await?;
        self.tx.commit().await?;
        Ok(())
    }
    async fn event(&mut self, kind: &str, actor: &str, payload: Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,$2,$3,$4)",
        )
        .bind(self.workspace)
        .bind(kind)
        .bind(actor)
        .bind(payload)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }
    /// Recheck source, consent and credentials while holding the same lock as edits.
    pub async fn fresh(&mut self, revision: i64, require_enabled: bool) -> Result<Value> {
        let id = self.job["id"]
            .as_str()
            .ok_or(Error::Conflict)?
            .parse::<Uuid>()
            .map_err(|_| Error::Conflict)?;
        let current: Value = sqlx::query_scalar(
            "SELECT to_jsonb(q) FROM conversion_outbox q WHERE workspace_id=$1 AND id=$2",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_one(&mut *self.tx)
        .await?;
        if current["version"] != self.job["version"] || current["state"] != "awaiting_connection" {
            return Err(ValidationError("Preparation changed; stage and validate it again").into());
        }
        let settings = conversions::settings(&mut self.tx, self.workspace).await?;
        let row = sqlx::query(conversions::CANDIDATES)
            .bind(self.workspace)
            .bind(current["kind"].as_str())
            .bind(current["entity_key"].as_str())
            .fetch_optional(&mut *self.tx)
            .await?
            .ok_or(Error::Conflict)?;
        let candidate = conversions::candidate(&row, &settings, self.workspace, Utc::now())?;
        if candidate["eligible"] != true || candidate["fingerprint"] != current["fingerprint"] {
            return Err(ValidationError(
                "Event or consent is no longer eligible; review its current preview",
            )
            .into());
        }
        let g=sqlx::query("SELECT validation,checked_at FROM google_connections WHERE workspace_id=$1 AND revision=$2 AND encrypted_token IS NOT NULL AND validated_settings_version=$3").bind(self.workspace).bind(revision).bind(settings.version).fetch_optional(&mut *self.tx).await?.ok_or(Error::Conflict)?;
        let report: Value = g
            .get::<Option<Value>, _>("validation")
            .ok_or(Error::Conflict)?;
        let checked: Option<chrono::DateTime<Utc>> = g.get("checked_at");
        if report["valid"] != true || !checked.is_some_and(|t| t > Utc::now() - Duration::hours(24))
        {
            return Err(ValidationError(
                "Validate the saved Google destination again; checks expire after 24 hours",
            )
            .into());
        }
        let action = &current["snapshot"]["settings"][if current["kind"] == "booking" {
            "booking_action_id"
        } else {
            "qualified_action_id"
        }];
        let window = report["actions"]
            .as_array()
            .and_then(|xs| {
                xs.iter()
                    .find(|v| &v["id"] == action && v["eligible"] == true)
            })
            .and_then(|v| v["window_days"].as_i64())
            .ok_or(Error::Conflict)?;
        let event = chrono::DateTime::parse_from_rfc3339(
            current["snapshot"]["event_at"]
                .as_str()
                .ok_or(Error::Conflict)?,
        )
        .map_err(|_| Error::Conflict)?;
        let visit = chrono::DateTime::parse_from_rfc3339(
            current["snapshot"]["visit_at"]
                .as_str()
                .ok_or(Error::Conflict)?,
        )
        .map_err(|_| Error::Conflict)?;
        if window < 1 || event - visit > Duration::days(window.min(90)) {
            return Err(
                ValidationError("Event falls outside the selected action's click window").into(),
            );
        }
        if require_enabled {
            let enabled:bool=sqlx::query_scalar("SELECT COALESCE((SELECT enabled FROM conversion_delivery_settings WHERE workspace_id=$1),false) AND NOT EXISTS(SELECT 1 FROM hosted_workspaces WHERE workspace_id=$1 AND NOT enabled)").bind(self.workspace).fetch_one(&mut *self.tx).await?;
            if !enabled {
                return Err(ValidationError("Google delivery is paused for this workspace").into());
            }
        }
        if let Some(d) = &self.delivery
            && (d["review_required"] == true
                || d["connection_revision"] != revision
                || d["outbox_version"] != current["version"])
        {
            return Err(Error::Conflict);
        }
        Ok(report)
    }
}
impl Store {
    pub async fn delivery_settings(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('enabled',enabled,'version',version) FROM conversion_delivery_settings WHERE workspace_id=$1").bind(self.workspace).fetch_optional(&self.pool).await?.unwrap_or(json!({"enabled":false,"version":0})))
    }
    pub async fn set_delivery_enabled(
        &self,
        enabled: bool,
        version: i32,
        actor: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let current: Option<i32> = sqlx::query_scalar(
            "SELECT version FROM conversion_delivery_settings WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .fetch_optional(&mut *tx)
        .await?;
        if current.unwrap_or(0) != version {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_delivery_settings(workspace_id,enabled) VALUES($1,$2) ON CONFLICT(workspace_id) DO UPDATE SET enabled=$2,version=conversion_delivery_settings.version+1").bind(self.workspace).bind(enabled).execute(&mut *tx).await?;
        if !enabled {
            sqlx::query("UPDATE conversion_deliveries SET state='needs_review',review_required=true,diagnostic='Delivery paused. Validate and queue again to resume.',due_at=NULL WHERE workspace_id=$1 AND state IN ('queued','retry')").bind(self.workspace).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'delivery_enabled',$2,$3)").bind(self.workspace).bind(actor).bind(json!({"enabled":enabled})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn delivery_list(&self) -> Result<Vec<Value>> {
        Ok(sqlx::query_scalar("SELECT to_jsonb(d)-'workspace_id'-'snapshot'-'request' FROM conversion_deliveries d WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 200").bind(self.workspace).fetch_all(&self.pool).await?)
    }
    pub async fn preparation_guard(&self, id: Uuid, version: i32) -> Result<Guard> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let job:Value=sqlx::query_scalar("SELECT to_jsonb(q) FROM conversion_outbox q WHERE workspace_id=$1 AND id=$2 AND version=$3 AND state='awaiting_connection'").bind(self.workspace).bind(id).bind(version).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        Ok(Guard {
            job,
            delivery: None,
            tx,
            workspace: self.workspace,
        })
    }
    pub async fn queue_delivery(&self, id: Uuid, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let d:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM conversion_deliveries d WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(self.workspace).bind(id).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        if d["state"] != "validated" || d["review_required"] == true {
            return Err(Error::Conflict);
        }
        let job: Value = sqlx::query_scalar(
            "SELECT to_jsonb(q) FROM conversion_outbox q WHERE workspace_id=$1 AND id=$2",
        )
        .bind(self.workspace)
        .bind(
            d["outbox_id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .map_err(|_| Error::Conflict)?,
        )
        .fetch_one(&mut *tx)
        .await?;
        let revision = d["connection_revision"].as_i64().ok_or(Error::Conflict)?;
        let mut guard = Guard {
            job,
            delivery: Some(d),
            tx,
            workspace: self.workspace,
        };
        guard.fresh(revision, true).await?;
        let changed=sqlx::query("UPDATE conversion_deliveries SET state='queued',due_at=now(),updated_at=now() WHERE workspace_id=$1 AND id=$2 AND validated_at>now()-interval '1 hour'").bind(self.workspace).bind(id).execute(&mut *guard.tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ValidationError("Event validation expired; validate again").into());
        }
        guard
            .event("delivery_queued", actor, json!({"delivery_id":id}))
            .await?;
        guard.tx.commit().await?;
        Ok(())
    }
    /// Poll an existing receipt only. Never replay the upload itself.
    pub async fn recheck_delivery(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let n=sqlx::query("UPDATE conversion_deliveries SET state='submitted',polls=0,due_at=now(),connection_revision=(SELECT revision FROM google_connections WHERE workspace_id=$1) WHERE workspace_id=$1 AND id=$2 AND request_id IS NOT NULL AND state IN ('uncertain','needs_review','failed') AND EXISTS(SELECT 1 FROM google_connections WHERE workspace_id=$1 AND encrypted_token IS NOT NULL)").bind(self.workspace).bind(id).execute(&mut *tx).await?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'delivery_recheck','owner',$2)").bind(self.workspace).bind(json!({"delivery_id":id})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn claim_delivery(&self) -> Result<Option<(Uuid, Uuid)>> {
        let mut tx = self.pool.begin().await?;
        // Ambiguous sends never silently restart. Polls are safe to resume.
        sqlx::query("UPDATE conversion_deliveries SET state=CASE WHEN state='sending' THEN 'uncertain' ELSE 'submitted' END,diagnostic='Worker interrupted; inspect delivery status.',due_at=CASE WHEN state='polling' THEN now() ELSE NULL END WHERE state IN ('sending','polling') AND due_at<now()").execute(&mut *tx).await?;
        let row=sqlx::query("SELECT id,workspace_id,state FROM conversion_deliveries WHERE state IN ('queued','retry','submitted') AND due_at<=now() ORDER BY due_at,id FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
        let Some(r) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let id: Uuid = r.get("id");
        let workspace: Uuid = r.get("workspace_id");
        let poll = r.get::<String, _>("state") == "submitted";
        sqlx::query("UPDATE conversion_deliveries SET state=$2,attempts=attempts+CASE WHEN $3 THEN 0 ELSE 1 END,polls=polls+CASE WHEN $3 THEN 1 ELSE 0 END,due_at=now()+interval '2 minutes',updated_at=now() WHERE id=$1").bind(id).bind(if poll {"polling"} else {"sending"}).bind(poll).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some((workspace, id)))
    }
    pub async fn claimed_guard(&self, id: Uuid) -> Result<Guard> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let d:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM conversion_deliveries d WHERE workspace_id=$1 AND id=$2 AND state IN ('sending','polling') FOR UPDATE").bind(self.workspace).bind(id).fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        let job =
            json!({"id":d["outbox_id"],"version":d["outbox_version"],"snapshot":d["snapshot"]});
        Ok(Guard {
            job,
            delivery: Some(d),
            tx,
            workspace: self.workspace,
        })
    }
}
