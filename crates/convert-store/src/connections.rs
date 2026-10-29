use crate::{Error, Result, Store};
use sqlx::Row;

// Deliberately not serializable or Debug: ciphertext never belongs in an API response/log.
pub struct TwilioConnection {
    pub account_sid: String,
    pub encrypted_token: String,
    pub revision: i64,
}

impl Store {
    pub async fn twilio_connection(&self) -> Result<Option<TwilioConnection>> {
        let row = sqlx::query("SELECT account_sid,encrypted_token,revision FROM twilio_connections WHERE workspace_id=$1")
            .bind(self.workspace).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| TwilioConnection {
            account_sid: r.get("account_sid"),
            encrypted_token: r.get("encrypted_token"),
            revision: r.get("revision"),
        }))
    }

    pub async fn save_twilio_connection(
        &self,
        account: &str,
        encrypted: &str,
        expected: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let existing = sqlx::query(
            "SELECT account_sid,revision FROM twilio_connections WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .fetch_optional(&mut *tx)
        .await?;
        if existing.as_ref().map_or(0, |r| r.get::<i64, _>("revision")) != expected {
            return Err(Error::Conflict);
        }
        if existing
            .as_ref()
            .is_some_and(|r| r.get::<String, _>("account_sid") != account)
        {
            let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE workspace_id=$1 AND NOT is_demo AND provider='twilio' AND released_at IS NULL)")
                .bind(self.workspace).fetch_one(&mut *tx).await?;
            if used {
                return Err(Error::Conflict);
            }
        }
        let revision: i64 = sqlx::query_scalar("UPDATE workspaces SET twilio_connection_revision=twilio_connection_revision+1 WHERE id=$1 RETURNING twilio_connection_revision")
            .bind(self.workspace).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO twilio_connections(workspace_id,account_sid,encrypted_token,revision) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id) DO UPDATE SET encrypted_token=$3,account_sid=$2,revision=$4,verified_at=now()")
            .bind(self.workspace).bind(account).bind(encrypted).bind(revision).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_twilio_connection(&self, expected: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE workspace_id=$1 AND NOT is_demo AND provider='twilio' AND released_at IS NULL)",
        )
        .bind(self.workspace)
        .fetch_one(&mut *tx)
        .await?;
        if used {
            return Err(Error::Conflict);
        }
        let result =
            sqlx::query("DELETE FROM twilio_connections WHERE workspace_id=$1 AND revision=$2")
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

    pub async fn manages_connection(&self, session_hash: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hosted_sessions s JOIN hosted_memberships m USING(workspace_id,user_id) JOIN hosted_users u USING(user_id) JOIN hosted_workspaces w USING(workspace_id) WHERE s.token_hash=$1 AND s.workspace_id=$2 AND s.expires_at>now() AND s.credential_version=u.credential_version AND m.enabled AND w.enabled AND m.role='owner')")
            .bind(session_hash).bind(self.workspace).fetch_one(&self.pool).await?)
    }
}
