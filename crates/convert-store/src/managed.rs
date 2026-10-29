//! Numbers VisiConvert buys and releases in a workspace's SignalWire project.
use crate::{Error, Result, Store};
use convert_domain::{NewNumber, PhoneProvider};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Where an idempotent purchase request stands.
pub enum Purchase {
    /// A new request; the caller may now buy the number at the carrier.
    Started,
    /// The same request already completed.
    Registered(Uuid),
    /// The same request is still in flight or failed; retry with a new request ID.
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
    pub async fn begin_purchase(&self, request: Uuid, phone: &str, limit: i64) -> Result<Purchase> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        if let Some(row) = sqlx::query(
            "SELECT phone_number,status,number_id FROM number_purchases WHERE workspace_id=$1 AND request_id=$2",
        )
        .bind(self.workspace)
        .bind(request)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.get::<String, _>("phone_number") != phone {
                return Err(Error::Conflict);
            }
            return Ok(match row.get::<Option<Uuid>, _>("number_id") {
                Some(id) if row.get::<String, _>("status") == "registered" => Purchase::Registered(id),
                _ => Purchase::Unfinished,
            });
        }
        let used: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM tracking_numbers WHERE workspace_id=$1 AND carrier_sid IS NOT NULL AND released_at IS NULL)
             + (SELECT count(*) FROM number_purchases WHERE workspace_id=$1 AND status='pending')",
        )
        .bind(self.workspace)
        .fetch_one(&mut *tx)
        .await?;
        if used >= limit {
            return Err(Error::Limit);
        }
        sqlx::query("INSERT INTO number_purchases(workspace_id,request_id,phone_number,status) VALUES($1,$2,$3,'pending')")
            .bind(self.workspace).bind(request).bind(phone).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Purchase::Started)
    }

    /// Registers a number the carrier has just sold to this workspace.
    pub async fn finish_purchase(
        &self,
        request: Uuid,
        n: &NewNumber,
        revision: i64,
        carrier_sid: &str,
    ) -> Result<Uuid> {
        debug_assert_eq!(n.provider, PhoneProvider::Signalwire);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
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
        let updated = sqlx::query("UPDATE tracking_numbers SET release_requested_at=NULL WHERE workspace_id=$1 AND id=$2 AND release_requested_at IS NOT NULL AND released_at IS NULL")
            .bind(self.workspace).bind(number).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    pub async fn mark_released(&self, number: Uuid) -> Result<()> {
        sqlx::query("UPDATE tracking_numbers SET released_at=now() WHERE workspace_id=$1 AND id=$2 AND release_requested_at IS NOT NULL AND released_at IS NULL")
            .bind(self.workspace).bind(number).execute(&self.pool).await?;
        Ok(())
    }
}

/// Purchased numbers across all workspaces whose hold began more than `hold_days` ago.
pub async fn due_releases(pool: &PgPool, hold_days: i32) -> Result<Vec<DueRelease>> {
    let rows = sqlx::query("SELECT workspace_id,id,carrier_sid FROM tracking_numbers WHERE release_requested_at IS NOT NULL AND released_at IS NULL AND carrier_sid IS NOT NULL AND release_requested_at < now() - make_interval(days => $1) ORDER BY release_requested_at LIMIT 100")
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
