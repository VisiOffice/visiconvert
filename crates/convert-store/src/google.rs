//! Workspace-bound OAuth state and encrypted credentials. No provider I/O.
use crate::{Error, Result, Store};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

// Never serialize/debug credentials or pending OAuth state.
pub struct Connection {
    pub revision: i64,
    pub encrypted_token: Option<String>,
    pub client_fingerprint: Option<String>,
    pub connected_at: Option<DateTime<Utc>>,
    pub validation: Option<Value>,
    pub checked_at: Option<DateTime<Utc>>,
}
pub struct Attempt {
    pub workspace: Uuid,
    pub state_hash: String,
    pub browser_hash: String,
    pub encrypted_verifier: String,
    pub client_fingerprint: String,
    pub session_hash: Option<String>,
    pub operator_hash: Option<String>,
    pub actor: String,
    pub expected_revision: i64,
}
impl Store {
    pub async fn rotate_google_refresh(&self, revision: i64, old: &str, new: &str) -> Result<()> {
        let result=sqlx::query("UPDATE google_connections SET encrypted_token=$4 WHERE workspace_id=$1 AND revision=$2 AND encrypted_token=$3")
            .bind(self.workspace).bind(revision).bind(old).bind(new).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub async fn clear_google_validation(&self, revision: i64) -> Result<()> {
        let result=sqlx::query("UPDATE google_connections SET validation=NULL,validated_settings_version=NULL,checked_at=NULL WHERE workspace_id=$1 AND revision=$2")
            .bind(self.workspace).bind(revision).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub async fn google_connection(&self) -> Result<Connection> {
        let row = sqlx::query("SELECT * FROM google_connections WHERE workspace_id=$1")
            .bind(self.workspace)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => Connection {
                revision: r.get("revision"),
                encrypted_token: r.get("encrypted_token"),
                client_fingerprint: r.get("client_fingerprint"),
                connected_at: r.get("connected_at"),
                validation: r.get("validation"),
                checked_at: r.get("checked_at"),
            },
            None => Connection {
                revision: 0,
                encrypted_token: None,
                client_fingerprint: None,
                connected_at: None,
                validation: None,
                checked_at: None,
            },
        })
    }
    pub async fn begin_google_oauth(&self, a: &Attempt) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        sqlx::query(
            "INSERT INTO google_connections(workspace_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(self.workspace)
        .execute(&mut *tx)
        .await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM google_connections WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_one(&mut *tx)
                .await?;
        if a.workspace != self.workspace || a.expected_revision != revision {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE google_connections SET pending_state_hash=$2 WHERE workspace_id=$1")
            .bind(self.workspace)
            .bind(&a.state_hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM google_oauth_attempts WHERE expires_at<=now()")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO google_oauth_attempts(workspace_id,state_hash,browser_hash,encrypted_verifier,client_fingerprint,session_hash,operator_hash,actor,expected_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(workspace_id) DO UPDATE SET state_hash=$2,browser_hash=$3,encrypted_verifier=$4,client_fingerprint=$5,session_hash=$6,operator_hash=$7,actor=$8,expected_revision=$9,expires_at=now()+interval '10 minutes'")
            .bind(self.workspace).bind(&a.state_hash).bind(&a.browser_hash).bind(&a.encrypted_verifier).bind(&a.client_fingerprint).bind(&a.session_hash).bind(&a.operator_hash).bind(&a.actor).bind(a.expected_revision).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn consume_google_oauth(
        &self,
        state: &str,
        browser: &str,
    ) -> Result<Option<Attempt>> {
        let row=sqlx::query("DELETE FROM google_oauth_attempts WHERE state_hash=$1 AND browser_hash=$2 AND expires_at>now() RETURNING *").bind(state).bind(browser).fetch_optional(&self.pool).await?;
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
    pub async fn finish_google_oauth(&self, a: &Attempt, encrypted: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if self.workspace != a.workspace {
            return Err(Error::Conflict);
        }
        if let Some(hash) = &a.session_hash {
            // Use the same identity-first lock order as session revocation, and
            // retain authorization locks until the credential write commits.
            sqlx::query("SELECT user_id FROM hosted_users WHERE user_id=(SELECT user_id FROM hosted_sessions WHERE token_hash=$1) FOR UPDATE")
                .bind(hash).fetch_optional(&mut *tx).await?;
            let allowed:Option<bool>=sqlx::query_scalar("SELECT true FROM hosted_sessions s JOIN hosted_memberships m USING(workspace_id,user_id) JOIN hosted_users u USING(user_id) JOIN hosted_workspaces w USING(workspace_id) WHERE s.token_hash=$1 AND s.workspace_id=$2 AND s.expires_at>now() AND s.credential_version=u.credential_version AND m.enabled AND w.enabled AND m.role='owner' FOR SHARE OF s,m,w").bind(hash).bind(self.workspace).fetch_optional(&mut *tx).await?;
            if allowed != Some(true) {
                return Err(Error::Conflict);
            }
        }
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE google_connections SET revision=revision+1,encrypted_token=$3,client_fingerprint=$4,connected_at=now(),validation=NULL,validated_settings_version=NULL,checked_at=NULL,pending_state_hash=NULL WHERE workspace_id=$1 AND revision=$2 AND pending_state_hash=$5")
            .bind(self.workspace).bind(a.expected_revision).bind(encrypted).bind(&a.client_fingerprint).bind(&a.state_hash).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'google_connected',$2,'{}')").bind(self.workspace).bind(&a.actor).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn disconnect_google(&self, revision: i64, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let changed=sqlx::query("UPDATE google_connections SET revision=revision+1,encrypted_token=NULL,client_fingerprint=NULL,connected_at=NULL,validation=NULL,validated_settings_version=NULL,checked_at=NULL,pending_state_hash=NULL WHERE workspace_id=$1 AND revision=$2")
            .bind(self.workspace).bind(revision).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("DELETE FROM google_oauth_attempts WHERE workspace_id=$1")
            .bind(self.workspace)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'google_disconnected',$2,'{}')").bind(self.workspace).bind(actor).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn save_google_validation(
        &self,
        revision: i64,
        settings_version: i32,
        report: &Value,
        actor: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let current: Option<i32> =
            sqlx::query_scalar("SELECT version FROM conversion_settings WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_optional(&mut *tx)
                .await?;
        if current != Some(settings_version) {
            return Err(Error::Conflict);
        }
        let changed=sqlx::query("UPDATE google_connections SET validation=$3,validated_settings_version=$4,checked_at=now() WHERE workspace_id=$1 AND revision=$2 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).bind(revision).bind(report).bind(settings_version).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,'google_destination_checked',$2,$3)").bind(self.workspace).bind(actor).bind(json!({"valid":report["valid"],"settings_version":settings_version})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
