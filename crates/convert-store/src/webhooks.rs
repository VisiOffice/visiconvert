//! Signed outbound webhooks: endpoints, the trigger-fed outbox and delivery state.
use crate::{Error, Result, Store, public_api::Page};
use chrono::{DateTime, Utc};
use convert_domain::ValidationError;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// Event types an endpoint can subscribe to. `webhook.test` is sent on request only.
pub const EVENT_TYPES: &[&str] = &[
    "interaction.created",
    "call.completed",
    "inquiry.reviewed",
    "booking.recorded",
    "booking.revised",
    "booking.canceled",
    "conversion.delivered",
    "conversion.failed",
];
pub const MAX_ENDPOINTS: i64 = 10;
/// Total attempts before a delivery fails.
pub const MAX_ATTEMPTS: i32 = 8;
/// Delay before attempt n+1 after a retryable failure on attempt n (1-based).
pub const RETRY_SECONDS: [i64; 7] = [5, 300, 1800, 7200, 18000, 36000, 36000];
pub const DISABLE_AFTER_DAYS: i64 = 5;
pub const RETENTION_DAYS: i64 = 30;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointInput {
    pub url: String,
    #[serde(default)]
    pub description: String,
    pub event_types: Vec<String>,
    #[serde(default = "yes")]
    pub include_contact: bool,
    /// `enabled` or `paused`. Creation always starts enabled.
    pub state: Option<String>,
    /// 0 for creation; the current version for updates.
    #[serde(default)]
    pub version: i32,
}
fn yes() -> bool {
    true
}
impl EndpointInput {
    fn validate(&self) -> Result<()> {
        let mut types = self.event_types.clone();
        types.sort();
        types.dedup();
        if types.is_empty()
            || types.len() != self.event_types.len()
            || types.iter().any(|t| !EVENT_TYPES.contains(&t.as_str()))
            || self.description.chars().count() > 200
            || self.description.chars().any(char::is_control)
            || self.url.len() > 2000
            || self
                .state
                .as_ref()
                .is_some_and(|s| s != "enabled" && s != "paused")
        {
            return Err(ValidationError(
                "Use distinct supported event types, a description up to 200 characters, and state enabled or paused",
            )
            .into());
        }
        Ok(())
    }
}

/// A delivery leased to this worker.
#[derive(Clone, Debug)]
pub struct Claim {
    pub id: Uuid,
    pub workspace: Uuid,
    pub event_id: Uuid,
    pub event_type: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub resource_version: Option<i32>,
    pub occurred_at: DateTime<Utc>,
    pub endpoint_id: Uuid,
    pub url: String,
    pub include_contact: bool,
    pub secret: String,
    pub previous_secret: Option<String>,
    pub payload: Option<Value>,
    pub attempts: i32,
}

/// The result of one HTTP attempt.
pub enum Attempt {
    Succeeded(u16),
    /// Retry later; the optional delay comes from Retry-After.
    Retry {
        status: Option<u16>,
        error: String,
        after: Option<i64>,
    },
    /// Do not retry this delivery.
    Failed {
        status: Option<u16>,
        error: String,
    },
    /// 410 Gone: fail and disable the endpoint.
    Gone,
    /// Not sent, e.g. a lead later marked as demo data. Not an endpoint failure.
    Skipped(String),
}

macro_rules! endpoint_json {
    () => {
        "jsonb_build_object('id',id,'url',url,'description',description,'event_types',event_types,'include_contact',include_contact,'state',state,'disabled_reason',disabled_reason,'failing_since',failing_since,'secret_rotating_until',CASE WHEN previous_secret_expires_at>now() THEN previous_secret_expires_at END,'version',version,'created_at',created_at,'updated_at',updated_at)"
    };
}

impl Store {
    pub async fn webhook_endpoints(&self) -> Result<Value> {
        let endpoints: Vec<Value> = sqlx::query_scalar(concat!(
            "SELECT ",
            endpoint_json!(),
            " FROM webhook_endpoints WHERE workspace_id=$1 ORDER BY created_at,id"
        ))
        .bind(self.workspace)
        .fetch_all(&self.pool)
        .await?;
        let history: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(e)-'workspace_id' FROM webhook_endpoint_events e WHERE workspace_id=$1 ORDER BY id DESC LIMIT 100")
            .bind(self.workspace)
            .fetch_all(&self.pool)
            .await?;
        Ok(json!({"endpoints":endpoints,"event_types":EVENT_TYPES,"history":history}))
    }
    pub async fn webhook_endpoint(&self, id: Uuid) -> Result<Value> {
        sqlx::query_scalar(concat!(
            "SELECT ",
            endpoint_json!(),
            " FROM webhook_endpoints WHERE workspace_id=$1 AND id=$2"
        ))
        .bind(self.workspace)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)
    }
    async fn record(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        endpoint: Uuid,
        kind: &str,
        actor: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO webhook_endpoint_events(workspace_id,endpoint_id,kind,actor) VALUES($1,$2,$3,$4)")
            .bind(self.workspace)
            .bind(endpoint)
            .bind(kind)
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    /// `sealed_secret` is encrypted by the caller, bound to this workspace and `id`.
    pub async fn create_webhook_endpoint(
        &self,
        id: Uuid,
        input: &EndpointInput,
        sealed_secret: &str,
        actor: &str,
    ) -> Result<Value> {
        input.validate()?;
        if input.version != 0 || input.state.as_deref() == Some("paused") {
            return Err(ValidationError("New endpoints use version 0 and start enabled").into());
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT 1 FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .execute(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM webhook_endpoints WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_one(&mut *tx)
                .await?;
        if count >= MAX_ENDPOINTS {
            return Err(
                ValidationError("A workspace can have at most 10 webhook endpoints").into(),
            );
        }
        sqlx::query("INSERT INTO webhook_endpoints(id,workspace_id,url,description,event_types,include_contact,encrypted_secret) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(id)
            .bind(self.workspace)
            .bind(&input.url)
            .bind(input.description.trim())
            .bind(&input.event_types)
            .bind(input.include_contact)
            .bind(sealed_secret)
            .execute(&mut *tx)
            .await?;
        self.record(&mut tx, id, "created", actor).await?;
        tx.commit().await?;
        self.webhook_endpoint(id).await
    }
    pub async fn update_webhook_endpoint(
        &self,
        id: Uuid,
        input: &EndpointInput,
        actor: &str,
    ) -> Result<Value> {
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT state FROM webhook_endpoints WHERE workspace_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let current = current.ok_or(Error::NotFound)?;
        let state = input.state.clone().unwrap_or_else(|| {
            if current == "disabled" {
                "disabled".into()
            } else {
                current.clone()
            }
        });
        let n = sqlx::query("UPDATE webhook_endpoints SET url=$3,description=$4,event_types=$5,include_contact=$6,state=$7,disabled_reason=CASE WHEN $7='disabled' THEN disabled_reason END,failing_since=CASE WHEN $7='enabled' AND state<>'enabled' THEN NULL ELSE failing_since END,version=version+1,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND version=$8")
            .bind(self.workspace)
            .bind(id)
            .bind(&input.url)
            .bind(input.description.trim())
            .bind(&input.event_types)
            .bind(input.include_contact)
            .bind(&state)
            .bind(input.version)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::Conflict);
        }
        let kind = match (current.as_str(), state.as_str()) {
            (a, b) if a == b => "updated",
            (_, "paused") => "paused",
            (_, "enabled") => "enabled",
            _ => "updated",
        };
        self.record(&mut tx, id, kind, actor).await?;
        tx.commit().await?;
        self.webhook_endpoint(id).await
    }
    pub async fn delete_webhook_endpoint(&self, id: Uuid, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query("DELETE FROM webhook_endpoints WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::NotFound);
        }
        self.record(&mut tx, id, "deleted", actor).await?;
        tx.commit().await?;
        Ok(())
    }
    /// The current secret keeps signing for 24 hours alongside the new one.
    pub async fn rotate_webhook_secret(
        &self,
        id: Uuid,
        version: i32,
        sealed_secret: &str,
        actor: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query("UPDATE webhook_endpoints SET previous_encrypted_secret=encrypted_secret,previous_secret_expires_at=now()+interval '24 hours',encrypted_secret=$4,version=version+1,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND version=$3")
            .bind(self.workspace)
            .bind(id)
            .bind(version)
            .bind(sealed_secret)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(Error::Conflict);
        }
        self.record(&mut tx, id, "secret_rotated", actor).await?;
        tx.commit().await?;
        self.webhook_endpoint(id).await
    }
    /// Queue a `webhook.test` event for one endpoint, regardless of its subscriptions.
    pub async fn queue_webhook_test(&self, id: Uuid) -> Result<Uuid> {
        let mut tx = self.pool.begin().await?;
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM webhook_endpoints WHERE workspace_id=$1 AND id=$2",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if state.ok_or(Error::NotFound)? != "enabled" {
            return Err(ValidationError("Enable the endpoint before sending a test").into());
        }
        let event = Uuid::new_v4();
        sqlx::query("INSERT INTO webhook_events(id,workspace_id,type,resource_kind,resource_id) VALUES($1,$2,'webhook.test','test',$3)")
            .bind(event)
            .bind(self.workspace)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        let delivery = Uuid::new_v4();
        sqlx::query("INSERT INTO webhook_deliveries(id,workspace_id,event_id,endpoint_id) VALUES($1,$2,$3,$4)")
            .bind(delivery)
            .bind(self.workspace)
            .bind(event)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_notify('visiconvert_webhooks','')")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(delivery)
    }
    pub async fn webhook_deliveries(&self, endpoint: Uuid, page: &Page) -> Result<Value> {
        let limit = page.limit.unwrap_or(50);
        if !(1..=100).contains(&limit)
            || page.since.is_some()
            || page.source.is_some()
            || page.qualification.is_some()
        {
            return Err(ValidationError("Deliveries accept limit 1–100 and cursor only").into());
        }
        let cursor = match &page.cursor {
            None => None,
            Some(c) => {
                let (time, id) = c
                    .split_once('|')
                    .ok_or(ValidationError("Invalid page cursor"))?;
                Some((
                    time.parse::<DateTime<Utc>>()
                        .map_err(|_| ValidationError("Invalid page cursor"))?,
                    id.parse::<Uuid>()
                        .map_err(|_| ValidationError("Invalid page cursor"))?,
                ))
            }
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM webhook_endpoints WHERE workspace_id=$1 AND id=$2)",
        )
        .bind(self.workspace)
        .bind(endpoint)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(Error::NotFound);
        }
        let rows = sqlx::query("SELECT d.id,d.created_at,jsonb_build_object('id',d.id,'event_id',d.event_id,'type',v.type,'state',d.state,'attempts',d.attempts,'last_status',d.last_status,'last_error',d.last_error,'last_attempt_at',d.last_attempt_at,'next_attempt_at',CASE WHEN d.state IN ('pending','retry') THEN d.due_at END,'occurred_at',v.occurred_at,'created_at',d.created_at) AS data FROM webhook_deliveries d JOIN webhook_events v ON v.workspace_id=d.workspace_id AND v.id=d.event_id WHERE d.workspace_id=$1 AND d.endpoint_id=$2 AND ($3::timestamptz IS NULL OR (d.created_at,d.id)<($3,$4)) ORDER BY d.created_at DESC,d.id DESC LIMIT $5")
            .bind(self.workspace)
            .bind(endpoint)
            .bind(cursor.map(|c| c.0))
            .bind(cursor.map(|c| c.1))
            .bind(limit + 1)
            .fetch_all(&self.pool)
            .await?;
        let next = (rows.len() > limit as usize).then(|| {
            let r = &rows[limit as usize - 1];
            format!(
                "{}|{}",
                r.get::<DateTime<Utc>, _>("created_at").to_rfc3339(),
                r.get::<Uuid, _>("id")
            )
        });
        let data: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|r| r.get("data"))
            .collect();
        Ok(json!({"data":data,"next_cursor":next}))
    }
    /// Resend a finished delivery with its original payload and event id.
    pub async fn redeliver_webhook(&self, id: Uuid, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let endpoint: Option<Uuid> = sqlx::query_scalar("UPDATE webhook_deliveries d SET state='pending',attempts=0,due_at=now(),last_error=NULL,updated_at=now() FROM webhook_endpoints e WHERE d.workspace_id=$1 AND d.id=$2 AND e.workspace_id=d.workspace_id AND e.id=d.endpoint_id AND e.state='enabled' AND d.state IN ('succeeded','failed','canceled') RETURNING d.endpoint_id")
            .bind(self.workspace)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(endpoint) = endpoint else {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM webhook_deliveries WHERE workspace_id=$1 AND id=$2)",
            )
            .bind(self.workspace)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            return Err(if exists {
                ValidationError(
                    "Only finished deliveries on an enabled endpoint can be redelivered",
                )
                .into()
            } else {
                Error::NotFound
            });
        };
        self.record(&mut tx, endpoint, "redelivered", actor).await?;
        sqlx::query("SELECT pg_notify('visiconvert_webhooks','')")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ---- Worker (all workspaces) ----

    /// Lease one due delivery across all workspaces, at most two in flight per endpoint.
    pub async fn claim_webhook(&self) -> Result<Option<Claim>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE webhook_deliveries SET state='retry',last_error='Worker stopped before a response; retrying',due_at=now(),updated_at=now() WHERE state='sending' AND due_at<now()")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query("SELECT d.id FROM webhook_deliveries d JOIN webhook_endpoints e ON e.workspace_id=d.workspace_id AND e.id=d.endpoint_id WHERE d.state IN ('pending','retry') AND d.due_at<=now() AND e.state='enabled' AND (SELECT count(*) FROM webhook_deliveries s WHERE s.endpoint_id=d.endpoint_id AND s.state='sending')<2 ORDER BY d.due_at,d.id FOR UPDATE OF d SKIP LOCKED LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let id: Uuid = row.get("id");
        let r = sqlx::query("UPDATE webhook_deliveries d SET state='sending',attempts=d.attempts+1,due_at=now()+interval '2 minutes',last_attempt_at=now(),updated_at=now() FROM webhook_events v, webhook_endpoints e WHERE d.id=$1 AND v.workspace_id=d.workspace_id AND v.id=d.event_id AND e.workspace_id=d.workspace_id AND e.id=d.endpoint_id RETURNING d.id,d.workspace_id,d.event_id,v.type,v.resource_kind,v.resource_id,v.resource_version,v.occurred_at,e.id AS endpoint_id,e.url,e.include_contact,e.encrypted_secret,CASE WHEN e.previous_secret_expires_at>now() THEN e.previous_encrypted_secret END AS previous_secret,d.payload,d.attempts")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(Claim {
            id: r.get("id"),
            workspace: r.get("workspace_id"),
            event_id: r.get("event_id"),
            event_type: r.get("type"),
            resource_kind: r.get("resource_kind"),
            resource_id: r.get("resource_id"),
            resource_version: r.get("resource_version"),
            occurred_at: r.get("occurred_at"),
            endpoint_id: r.get("endpoint_id"),
            url: r.get("url"),
            include_contact: r.get("include_contact"),
            secret: r.get("encrypted_secret"),
            previous_secret: r.get("previous_secret"),
            payload: r.get("payload"),
            attempts: r.get("attempts"),
        }))
    }
    /// Build the payload once and keep it, so retries send identical bytes.
    pub async fn webhook_payload(&self, claim: &Claim) -> Result<Value> {
        if let Some(payload) = &claim.payload {
            return Ok(payload.clone());
        }
        let mut data = match claim.resource_kind.as_str() {
            "interaction" | "inquiry" => {
                let id = claim
                    .resource_id
                    .parse::<Uuid>()
                    .map_err(|_| Error::NotFound)?;
                let grouped = claim.resource_kind == "inquiry";
                let page = self.public_leads(&Page::default(), grouped, Some(id)).await?;
                let mut item = page["data"].get(0).cloned().unwrap_or(Value::Null);
                if item.is_object() && claim.include_contact {
                    let lead = if grouped {
                        item["primary_lead_id"].as_str().and_then(|s| s.parse::<Uuid>().ok())
                    } else {
                        Some(id)
                    };
                    let fields: Option<Value> = sqlx::query_scalar("SELECT submission_payload FROM leads WHERE workspace_id=$1 AND id=$2")
                        .bind(self.workspace)
                        .bind(lead)
                        .fetch_optional(&self.pool)
                        .await?
                        .flatten();
                    item["fields"] = fields.unwrap_or_else(|| json!({}));
                }
                item
            }
            "booking" => sqlx::query_scalar("SELECT jsonb_build_object('external_id',o.external_id,'lead_id',o.lead_id,'inquiry_id',l.inquiry_id,'amount_minor',o.amount_minor,'currency',o.currency,'status',o.status,'revision',o.revision) FROM outcomes o JOIN leads l ON l.id=o.lead_id AND l.workspace_id=o.workspace_id WHERE o.workspace_id=$1 AND o.external_id=$2")
                .bind(self.workspace)
                .bind(&claim.resource_id)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or(Value::Null),
            "conversion" => {
                let id = claim
                    .resource_id
                    .parse::<Uuid>()
                    .map_err(|_| Error::NotFound)?;
                sqlx::query_scalar("SELECT jsonb_build_object('id',id,'outbox_id',outbox_id,'outbox_version',outbox_version,'provider',provider,'state',state,'request_id',request_id,'diagnostic',diagnostic,'attempts',attempts,'updated_at',updated_at) FROM conversion_deliveries WHERE workspace_id=$1 AND id=$2")
                    .bind(self.workspace)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .unwrap_or(Value::Null)
            }
            _ => json!({"message":"Test event from VisiConvert. No action is needed.","endpoint_id":claim.endpoint_id}),
        };
        if let Some(object) = data.as_object_mut() {
            if !claim.include_contact {
                for field in ["name", "contact", "notes", "fields"] {
                    object.remove(field);
                }
            }
            object.insert("object".into(), json!(claim.resource_kind));
        }
        let payload = json!({
            "id": claim.event_id,
            "type": claim.event_type,
            "occurred_at": claim.occurred_at,
            "workspace_id": claim.workspace,
            "resource_version": claim.resource_version,
            "data": data,
        });
        // Another attempt may have stored it first; keep whichever was stored.
        let stored: Value = sqlx::query_scalar("UPDATE webhook_deliveries SET payload=COALESCE(payload,$2) WHERE id=$1 RETURNING payload")
            .bind(claim.id)
            .bind(&payload)
            .fetch_one(&self.pool)
            .await?;
        Ok(stored)
    }
    /// Record an attempt. Returns true when the endpoint was disabled as a result.
    pub async fn finish_webhook(&self, claim: &Claim, attempt: Attempt) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let (state, status, error, delay, gone) = match attempt {
            Attempt::Succeeded(status) => ("succeeded", Some(status), None, None, false),
            Attempt::Retry {
                status,
                error,
                after,
            } if claim.attempts < MAX_ATTEMPTS => {
                let base =
                    RETRY_SECONDS[(claim.attempts as usize - 1).min(RETRY_SECONDS.len() - 1)];
                let delay = after.map_or(base, |a| a.clamp(base, 3600.max(base)));
                ("retry", status, Some(error), Some(delay), false)
            }
            Attempt::Retry { status, error, .. } => ("failed", status, Some(error), None, false),
            Attempt::Failed { status, error } => ("failed", status, Some(error), None, false),
            Attempt::Skipped(reason) => ("canceled", None, Some(reason), None, false),
            Attempt::Gone => (
                "failed",
                Some(410),
                Some("Endpoint returned 410 Gone".into()),
                None,
                true,
            ),
        };
        let error = error.map(|e| e.chars().take(300).collect::<String>());
        sqlx::query("UPDATE webhook_deliveries SET state=$2,last_status=$3,last_error=$4,due_at=CASE WHEN $5::bigint IS NULL THEN NULL ELSE now()+make_interval(secs=>$5::bigint) END,updated_at=now() WHERE id=$1 AND state='sending'")
            .bind(claim.id)
            .bind(state)
            .bind(status.map(i32::from))
            .bind(&error)
            .bind(delay)
            .execute(&mut *tx)
            .await?;
        let reason = if state == "canceled" {
            None
        } else if gone {
            Some("The endpoint returned 410 Gone")
        } else if state == "succeeded" {
            sqlx::query("UPDATE webhook_endpoints SET failing_since=NULL WHERE id=$1 AND failing_since IS NOT NULL")
                .bind(claim.endpoint_id)
                .execute(&mut *tx)
                .await?;
            None
        } else {
            let since: Option<DateTime<Utc>> = sqlx::query_scalar("UPDATE webhook_endpoints SET failing_since=COALESCE(failing_since,now()) WHERE id=$1 RETURNING failing_since")
                .bind(claim.endpoint_id)
                .fetch_optional(&mut *tx)
                .await?;
            since
                .filter(|s| *s < Utc::now() - chrono::Duration::days(DISABLE_AFTER_DAYS))
                .map(|_| "Every delivery failed for 5 days")
        };
        let disabled = if let Some(reason) = reason {
            let n = sqlx::query("UPDATE webhook_endpoints SET state='disabled',disabled_reason=$2,version=version+1,updated_at=now() WHERE id=$1 AND state='enabled'")
                .bind(claim.endpoint_id)
                .bind(reason)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if n == 1 {
                sqlx::query("UPDATE webhook_deliveries SET state='canceled',due_at=NULL,last_error='Endpoint disabled',updated_at=now() WHERE endpoint_id=$1 AND state IN ('pending','retry')")
                    .bind(claim.endpoint_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO webhook_endpoint_events(workspace_id,endpoint_id,kind,actor) VALUES($1,$2,'disabled','VisiConvert')")
                    .bind(claim.workspace)
                    .bind(claim.endpoint_id)
                    .execute(&mut *tx)
                    .await?;
            }
            n == 1
        } else {
            false
        };
        tx.commit().await?;
        Ok(disabled)
    }
    /// Delete finished events older than the retention window.
    pub async fn prune_webhooks(&self) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM webhook_events v WHERE occurred_at<now()-make_interval(days=>$1::int) AND NOT EXISTS(SELECT 1 FROM webhook_deliveries d WHERE d.workspace_id=v.workspace_id AND d.event_id=v.id AND d.state IN ('pending','retry','sending'))")
            .bind(RETENTION_DAYS as i32)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
    /// Wakes the worker when a trigger or request queues a delivery.
    pub async fn webhook_listener(&self) -> Result<sqlx::postgres::PgListener> {
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool).await?;
        listener.listen("visiconvert_webhooks").await?;
        Ok(listener)
    }
    /// A store for another workspace sharing this pool (used by the worker).
    pub fn for_workspace(&self, workspace: Uuid) -> Store {
        Store {
            pool: self.pool.clone(),
            workspace,
        }
    }
}
