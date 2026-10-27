use crate::{Error, Result, Store};
use sqlx::Row;

// Deliberately not serializable or Debug: ciphertext never belongs in an API response/log.
pub struct SignalWireConnection {
    pub project_id: uuid::Uuid,
    pub space_host: String,
    pub encrypted_signing_key: String,
    pub last_webhook_at: Option<chrono::DateTime<chrono::Utc>>,
    pub encrypted_token: String,
    pub revision: i64,
}

impl Store {
    pub async fn record_signalwire_webhook(&self, revision: i64) -> Result<()> {
        sqlx::query("UPDATE signalwire_connections SET last_webhook_at=now() WHERE workspace_id=$1 AND revision=$2")
            .bind(self.workspace).bind(revision).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn signalwire_connection(&self) -> Result<Option<SignalWireConnection>> {
        let row = sqlx::query("SELECT space_host,project_id,encrypted_token,encrypted_signing_key,last_webhook_at,revision FROM signalwire_connections WHERE workspace_id=$1")
            .bind(self.workspace).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| SignalWireConnection {
            project_id: r.get("project_id"),
            space_host: r.get("space_host"),
            encrypted_signing_key: r.get("encrypted_signing_key"),
            last_webhook_at: r.get("last_webhook_at"),
            encrypted_token: r.get("encrypted_token"),
            revision: r.get("revision"),
        }))
    }

    pub async fn save_signalwire_connection(
        &self,
        space: &str,
        account: uuid::Uuid,
        signing: &str,
        encrypted: &str,
        expected: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let existing = sqlx::query(
            "SELECT space_host,project_id,revision FROM signalwire_connections WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .fetch_optional(&mut *tx)
        .await?;
        if existing.as_ref().map_or(0, |r| r.get::<i64, _>("revision")) != expected {
            return Err(Error::Conflict);
        }
        if existing.as_ref().is_some_and(|r| {
            r.get::<uuid::Uuid, _>("project_id") != account
                || r.get::<String, _>("space_host") != space
        }) {
            let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE workspace_id=$1 AND NOT is_demo AND provider='signalwire')")
                .bind(self.workspace).fetch_one(&mut *tx).await?;
            if used {
                return Err(Error::Conflict);
            }
        }
        let revision: i64 = sqlx::query_scalar("UPDATE workspaces SET signalwire_connection_revision=signalwire_connection_revision+1 WHERE id=$1 RETURNING signalwire_connection_revision")
            .bind(self.workspace).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO signalwire_connections(workspace_id,project_id,encrypted_token,revision,space_host,encrypted_signing_key) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(workspace_id) DO UPDATE SET encrypted_token=$3,project_id=$2,revision=$4,verified_at=now(),space_host=$5,encrypted_signing_key=$6,last_webhook_at=NULL")
            .bind(self.workspace).bind(account).bind(encrypted).bind(revision).bind(space).bind(signing).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_signalwire_connection(&self, expected: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE workspace_id=$1 AND NOT is_demo AND provider='signalwire')",
        )
        .bind(self.workspace)
        .fetch_one(&mut *tx)
        .await?;
        if used {
            return Err(Error::Conflict);
        }
        let result =
            sqlx::query("DELETE FROM signalwire_connections WHERE workspace_id=$1 AND revision=$2")
                .bind(self.workspace)
                .bind(expected)
                .execute(&mut *tx)
                .await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
