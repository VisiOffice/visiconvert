//! Numbers VisiConvert buys and releases in a workspace's SignalWire project.
use crate::signalwire::SignalWireConnection;
use crate::{Error, Result, Store};
use convert_domain::{NewNumber, PhoneProvider};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Where an idempotent purchase request stands.
pub enum Purchase {
    /// A new request; the caller may now buy the number at the carrier.
    Started,
    /// The same request already completed.
    Registered(Uuid),
    /// The same request is unresolved or failed; inspect its status before taking further action.
    Unfinished,
}

/// A purchased number whose release hold has expired.
pub struct DueRelease {
    pub workspace_id: Uuid,
    pub number_id: Uuid,
    pub carrier_sid: String,
}

impl Store {
    /// Records a purchase request before any carrier call. `limit` caps live purchased
    /// numbers plus purchases still in flight.
    pub async fn begin_purchase(
        &self,
        request: Uuid,
        n: &NewNumber,
        connection: &SignalWireConnection,
        limit: i64,
    ) -> Result<Purchase> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        if let Some(row) = sqlx::query(
            "SELECT phone_number,status,number_id,request_payload FROM number_purchases WHERE workspace_id=$1 AND request_id=$2",
        )
        .bind(self.workspace)
        .bind(request)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.get::<String, _>("phone_number") != n.phone_number
                || row.get::<Option<Value>, _>("request_payload").is_some_and(|value| value != json!(n)) {
                return Err(Error::Conflict);
            }
            return Ok(match row.get::<Option<Uuid>, _>("number_id") {
                Some(id) if row.get::<String, _>("status") == "registered" => Purchase::Registered(id),
                _ => Purchase::Unfinished,
            });
        }
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM signalwire_connections WHERE workspace_id=$1 AND project_id=$2 AND space_host=$3 AND revision=$4)")
            .bind(self.workspace).bind(connection.project_id).bind(&connection.space_host).bind(connection.revision).fetch_one(&mut *tx).await?;
        if !current {
            return Err(Error::Conflict);
        }
        let unresolved: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM number_purchases WHERE workspace_id=$1 AND phone_number=$2 AND status IN ('pending','cleanup','review'))")
            .bind(self.workspace).bind(&n.phone_number).fetch_one(&mut *tx).await?;
        if unresolved {
            return Err(Error::Conflict);
        }
        let used: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM tracking_numbers WHERE workspace_id=$1 AND carrier_sid IS NOT NULL AND released_at IS NULL)
             + (SELECT count(*) FROM number_purchases WHERE workspace_id=$1 AND status IN ('pending','cleanup','review'))",
        )
        .bind(self.workspace)
        .fetch_one(&mut *tx)
        .await?;
        if used >= limit {
            return Err(Error::Limit);
        }
        sqlx::query("INSERT INTO number_purchases(workspace_id,request_id,phone_number,status,request_payload,space_host,project_id) VALUES($1,$2,$3,'pending',$4,$5,$6)")
            .bind(self.workspace).bind(request).bind(&n.phone_number).bind(json!(n)).bind(&connection.space_host).bind(connection.project_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Purchase::Started)
    }

    /// Registers a number the carrier has just sold to this workspace.
    pub async fn finish_purchase(
        &self,
        request: Uuid,
        n: &NewNumber,
        carrier_sid: &str,
    ) -> Result<Uuid> {
        debug_assert_eq!(n.provider, PhoneProvider::Signalwire);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let row = sqlx::query("SELECT status,number_id,carrier_sid,request_payload,space_host,project_id FROM number_purchases WHERE workspace_id=$1 AND request_id=$2 FOR UPDATE")
            .bind(self.workspace).bind(request).fetch_one(&mut *tx).await?;
        if row.get::<Option<Value>, _>("request_payload") != Some(json!(n)) {
            return Err(Error::Conflict);
        }
        if row.get::<String, _>("status") == "registered" {
            return Ok(row.get("number_id"));
        }
        if row.get::<String, _>("status") != "pending"
            || row.get::<Option<String>, _>("carrier_sid").as_deref() != Some(carrier_sid)
        {
            return Err(Error::Conflict);
        }
        // Credential rotation within the same project does not invalidate a
        // completed carrier purchase. Bind to project identity, then use its
        // current revision under the workspace lock for registration.
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM signalwire_connections WHERE workspace_id=$1 AND space_host=$2 AND project_id=$3")
            .bind(self.workspace).bind(row.get::<String,_>("space_host")).bind(row.get::<Uuid,_>("project_id"))
            .fetch_optional(&mut *tx).await?.ok_or(Error::Conflict)?;
        let id = self
            .insert_number(&mut tx, n, Some(revision), Some(carrier_sid))
            .await?;
        let updated = sqlx::query("UPDATE number_purchases SET status='registered',number_id=$3 WHERE workspace_id=$1 AND request_id=$2 AND status='pending'")
            .bind(self.workspace).bind(request).bind(id).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await?;
        Ok(id)
    }

    pub async fn fail_purchase(&self, request: Uuid) -> Result<()> {
        sqlx::query("UPDATE number_purchases SET status='failed' WHERE workspace_id=$1 AND request_id=$2 AND status='pending'")
            .bind(self.workspace).bind(request).execute(&self.pool).await?;
        Ok(())
    }

    /// Starts the release hold. The number keeps forwarding calls, but stops being
    /// handed to new website visitors.
    pub async fn request_release(&self, number: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(73581001)")
            .execute(&mut *tx)
            .await?;
        let fallback: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers n JOIN dni_pools p ON p.fallback_phone=n.phone_number WHERE n.workspace_id=$1 AND n.id=$2)")
            .bind(self.workspace).bind(number).fetch_one(&mut *tx).await?;
        if fallback {
            return Err(convert_domain::ValidationError(
                "Replace this number in website fallback settings before releasing it",
            )
            .into());
        }
        let updated = sqlx::query("UPDATE tracking_numbers SET release_requested_at=now() WHERE workspace_id=$1 AND id=$2 AND carrier_sid IS NOT NULL AND release_requested_at IS NULL")
            .bind(self.workspace).bind(number).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        // Bump the pool version so an open pool editor cannot re-enable it unnoticed.
        sqlx::query("UPDATE dni_pools SET version=version+1 WHERE workspace_id=$1 AND site_id IN (SELECT site_id FROM dni_pool_numbers WHERE workspace_id=$1 AND number_id=$2 AND enabled)")
            .bind(self.workspace).bind(number).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE dni_pool_numbers SET enabled=false WHERE workspace_id=$1 AND number_id=$2",
        )
        .bind(self.workspace)
        .bind(number)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Cancels a pending release. Pool membership stays disabled until re-enabled.
    pub async fn restore_number(&self, number: Uuid) -> Result<()> {
        let updated = sqlx::query("UPDATE tracking_numbers SET release_requested_at=NULL WHERE workspace_id=$1 AND id=$2 AND release_requested_at IS NOT NULL AND released_at IS NULL AND release_started_at IS NULL")
            .bind(self.workspace).bind(number).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// Atomically cross the irreversible boundary. A stale worker snapshot cannot
    /// delete a restored number. Retries keep this marker even after carrier errors.
    pub async fn claim_release(&self, number: Uuid, hold_days: i32) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(73581001)")
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("UPDATE tracking_numbers n SET release_started_at=COALESCE(release_started_at,now()) WHERE workspace_id=$1 AND id=$2 AND released_at IS NULL AND (release_started_at IS NOT NULL OR release_requested_at < now()-make_interval(days=>$3)) AND NOT EXISTS(SELECT 1 FROM dni_pools p WHERE p.fallback_phone=n.phone_number)")
            .bind(self.workspace).bind(number).bind(hold_days).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
    pub async fn mark_released(&self, number: Uuid) -> Result<()> {
        sqlx::query("UPDATE tracking_numbers SET released_at=COALESCE(released_at,now()) WHERE workspace_id=$1 AND id=$2 AND release_started_at IS NOT NULL")
            .bind(self.workspace).bind(number).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn purchase_status(&self, request: Uuid) -> Result<Value> {
        sqlx::query_scalar("SELECT jsonb_build_object('request_id',request_id,'phone_number',phone_number,'status',status,'number_id',number_id) FROM number_purchases WHERE workspace_id=$1 AND request_id=$2")
            .bind(self.workspace).bind(request).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn record_purchase_number(&self, request: Uuid, sid: &str) -> Result<()> {
        let result = sqlx::query("UPDATE number_purchases SET carrier_sid=$3 WHERE workspace_id=$1 AND request_id=$2 AND status='pending' AND (carrier_sid IS NULL OR carrier_sid=$3)")
            .bind(self.workspace).bind(request).bind(sid).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub async fn cleanup_purchase(&self, request: Uuid) -> Result<()> {
        sqlx::query("UPDATE number_purchases SET status='cleanup' WHERE workspace_id=$1 AND request_id=$2 AND status='pending' AND carrier_sid IS NOT NULL")
            .bind(self.workspace).bind(request).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn complete_cleanup(&self, request: Uuid) -> Result<()> {
        sqlx::query("UPDATE number_purchases SET status='failed' WHERE workspace_id=$1 AND request_id=$2 AND status='cleanup'")
            .bind(self.workspace).bind(request).execute(&self.pool).await?;
        Ok(())
    }
}

pub struct RecoverPurchase {
    pub workspace: Uuid,
    pub request: Uuid,
    pub number: NewNumber,
    pub space: String,
    pub project: Uuid,
    pub carrier_sid: Option<String>,
    pub cleanup: bool,
}
/// Rotate attempts fairly; an unavailable project must not starve other tenants.
pub async fn recoverable_purchases(pool: &PgPool) -> Result<Vec<RecoverPurchase>> {
    let rows=sqlx::query("UPDATE number_purchases SET last_attempt_at=now() WHERE (workspace_id,request_id) IN (SELECT workspace_id,request_id FROM number_purchases WHERE status IN ('pending','cleanup') AND request_payload IS NOT NULL AND last_attempt_at < now()-interval '1 minute' ORDER BY last_attempt_at LIMIT 100 FOR UPDATE SKIP LOCKED) RETURNING *")
        .fetch_all(pool).await?;
    rows.into_iter()
        .map(|r| {
            Ok(RecoverPurchase {
                workspace: r.get("workspace_id"),
                request: r.get("request_id"),
                number: serde_json::from_value(r.get("request_payload")).map_err(|_| {
                    convert_domain::ValidationError("Invalid persisted number purchase")
                })?,
                space: r.get("space_host"),
                project: r.get("project_id"),
                carrier_sid: r.get("carrier_sid"),
                cleanup: r.get::<String, _>("status") == "cleanup",
            })
        })
        .collect()
}

/// Purchased numbers across all workspaces whose hold began more than `hold_days` ago.
pub async fn due_releases(pool: &PgPool, hold_days: i32) -> Result<Vec<DueRelease>> {
    let rows = sqlx::query("SELECT workspace_id,id,carrier_sid FROM tracking_numbers WHERE release_requested_at IS NOT NULL AND released_at IS NULL AND carrier_sid IS NOT NULL AND (release_started_at IS NOT NULL OR release_requested_at < now() - make_interval(days => $1)) ORDER BY release_requested_at LIMIT 100")
        .bind(hold_days).fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| DueRelease {
            workspace_id: r.get("workspace_id"),
            number_id: r.get("id"),
            carrier_sid: r.get("carrier_sid"),
        })
        .collect())
}
