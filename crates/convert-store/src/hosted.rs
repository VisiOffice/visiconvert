use crate::{Error, Result, Store};
use sqlx::Row;
use uuid::Uuid;

impl Store {
    /// Idempotent creation; never adopts an existing self-hosted workspace or changes its owner.
    pub async fn provision_hosted(&self, workspace: Uuid, owner: Uuid, name: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(73581002)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO hosted_users(user_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query("SELECT w.name,h.owner_id FROM workspaces w LEFT JOIN hosted_workspaces h ON h.workspace_id=w.id WHERE w.id=$1")
            .bind(workspace).fetch_optional(&mut *tx).await?;
        if let Some(row) = existing {
            if row.get::<Option<Uuid>, _>("owner_id") != Some(owner)
                || row.get::<String, _>("name") != name
            {
                return Err(Error::Conflict);
            }
        } else {
            sqlx::query("INSERT INTO workspaces(id,name) VALUES($1,$2)")
                .bind(workspace)
                .bind(name)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO hosted_workspaces(workspace_id,owner_id) VALUES($1,$2)")
                .bind(workspace)
                .bind(owner)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO hosted_memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')",
            )
            .bind(workspace)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn hosted_exists(&self, workspace: Uuid) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM hosted_workspaces WHERE workspace_id=$1 AND enabled)",
        )
        .bind(workspace)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn issue_hosted_code(
        &self,
        workspace: Uuid,
        user: Uuid,
        challenge: &str,
        code_hash: &str,
        credential_version: i64,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT credential_version FROM hosted_users WHERE user_id=$1 FOR UPDATE",
        )
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if current != Some(credential_version) {
            return Ok(false);
        }
        sqlx::query("DELETE FROM hosted_launch_codes WHERE expires_at < now()")
            .execute(&mut *tx)
            .await?;
        let inserted = sqlx::query("INSERT INTO hosted_launch_codes(code_hash,workspace_id,user_id,browser_challenge,expires_at,credential_version) SELECT $4,m.workspace_id,m.user_id,$3,now()+interval '60 seconds',$5 FROM hosted_memberships m JOIN hosted_workspaces w USING(workspace_id) WHERE m.workspace_id=$1 AND m.user_id=$2 AND m.enabled AND w.enabled")
            .bind(workspace).bind(user).bind(challenge).bind(code_hash).bind(credential_version).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// DELETE ... RETURNING serializes concurrent exchanges. A wrong browser never consumes a code.
    pub async fn exchange_hosted_code(
        &self,
        code_hash: &str,
        challenge: &str,
        session_hash: &str,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        // Serialize issuance/exchange/revocation on the identity, always before touching codes.
        sqlx::query("SELECT user_id FROM hosted_users WHERE user_id=(SELECT user_id FROM hosted_launch_codes WHERE code_hash=$1) FOR UPDATE")
            .bind(code_hash).fetch_optional(&mut *tx).await?;
        let row = sqlx::query("DELETE FROM hosted_launch_codes c USING hosted_memberships m, hosted_workspaces w, hosted_users u WHERE c.code_hash=$1 AND c.browser_challenge=$2 AND c.expires_at>now() AND m.workspace_id=c.workspace_id AND m.user_id=c.user_id AND m.enabled AND w.workspace_id=m.workspace_id AND w.enabled AND u.user_id=c.user_id AND u.credential_version=c.credential_version RETURNING c.workspace_id,c.user_id,c.credential_version")
            .bind(code_hash).bind(challenge).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            sqlx::query("DELETE FROM hosted_sessions WHERE expires_at<now()")
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO hosted_sessions(token_hash,workspace_id,user_id,expires_at,credential_version) VALUES($1,$2,$3,now()+interval '15 minutes',$4)")
                .bind(session_hash).bind(row.get::<Uuid,_>("workspace_id")).bind(row.get::<Uuid,_>("user_id")).bind(row.get::<i64,_>("credential_version")).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn hosted_session(&self, token_hash: &str) -> Result<Option<(Store, String)>> {
        let row = sqlx::query("SELECT s.workspace_id,w.name FROM hosted_sessions s JOIN hosted_users u ON u.user_id=s.user_id AND u.credential_version=s.credential_version JOIN hosted_memberships m ON m.workspace_id=s.workspace_id AND m.user_id=s.user_id JOIN hosted_workspaces h ON h.workspace_id=m.workspace_id JOIN workspaces w ON w.id=h.workspace_id WHERE s.token_hash=$1 AND s.expires_at>now() AND m.enabled AND h.enabled")
            .bind(token_hash).fetch_optional(&self.pool).await?;
        Ok(row.map(|row| {
            (
                Store {
                    pool: self.pool.clone(),
                    workspace: row.get("workspace_id"),
                },
                row.get("name"),
            )
        }))
    }

    pub async fn revoke_hosted_session(&self, token_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM hosted_sessions WHERE token_hash=$1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Monotonic and idempotent: delayed requests can never restore an old credential version.
    pub async fn revoke_hosted_user(&self, user: Uuid, version: i64) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let current: i64 = sqlx::query_scalar("INSERT INTO hosted_users(user_id,credential_version) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET credential_version=GREATEST(hosted_users.credential_version,EXCLUDED.credential_version) RETURNING credential_version")
            .bind(user).bind(version).fetch_one(&mut *tx).await?;
        sqlx::query("DELETE FROM hosted_launch_codes WHERE user_id=$1 AND credential_version<$2")
            .bind(user)
            .bind(current)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM hosted_sessions WHERE user_id=$1 AND credential_version<$2")
            .bind(user)
            .bind(current)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(current)
    }
}

impl Store {
    /// Remote MCP delegation must satisfy the same engine-side membership and
    /// credential-version fences as a durable hosted API key.
    pub async fn mcp_identity(
        &self,
        workspace: Uuid,
        user: Uuid,
        version: i64,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT w.name FROM workspaces w JOIN hosted_workspaces h ON h.workspace_id=w.id JOIN hosted_memberships m ON m.workspace_id=w.id JOIN hosted_users u ON u.user_id=m.user_id WHERE w.id=$1 AND m.user_id=$2 AND u.credential_version=$3 AND h.enabled AND m.enabled AND m.role='owner'")
            .bind(workspace).bind(user).bind(version).fetch_optional(&self.pool).await?)
    }
}
