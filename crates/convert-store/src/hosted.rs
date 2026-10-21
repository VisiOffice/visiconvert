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
    ) -> Result<bool> {
        sqlx::query("DELETE FROM hosted_launch_codes WHERE expires_at < now()")
            .execute(&self.pool)
            .await?;
        let inserted = sqlx::query("INSERT INTO hosted_launch_codes(code_hash,workspace_id,user_id,browser_challenge,expires_at) SELECT $4,m.workspace_id,m.user_id,$3,now()+interval '60 seconds' FROM hosted_memberships m JOIN hosted_workspaces w USING(workspace_id) WHERE m.workspace_id=$1 AND m.user_id=$2 AND m.enabled AND w.enabled")
            .bind(workspace).bind(user).bind(challenge).bind(code_hash).execute(&self.pool).await?;
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
        let row = sqlx::query("DELETE FROM hosted_launch_codes c USING hosted_memberships m, hosted_workspaces w WHERE c.code_hash=$1 AND c.browser_challenge=$2 AND c.expires_at>now() AND m.workspace_id=c.workspace_id AND m.user_id=c.user_id AND m.enabled AND w.workspace_id=m.workspace_id AND w.enabled RETURNING c.workspace_id,c.user_id")
            .bind(code_hash).bind(challenge).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            sqlx::query("DELETE FROM hosted_sessions WHERE expires_at<now()")
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO hosted_sessions(token_hash,workspace_id,user_id,expires_at) VALUES($1,$2,$3,now()+interval '15 minutes')")
                .bind(session_hash).bind(row.get::<Uuid,_>("workspace_id")).bind(row.get::<Uuid,_>("user_id")).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn hosted_session(&self, token_hash: &str) -> Result<Option<(Store, String)>> {
        let row = sqlx::query("SELECT s.workspace_id,w.name FROM hosted_sessions s JOIN hosted_memberships m ON m.workspace_id=s.workspace_id AND m.user_id=s.user_id JOIN hosted_workspaces h ON h.workspace_id=m.workspace_id JOIN workspaces w ON w.id=h.workspace_id WHERE s.token_hash=$1 AND s.expires_at>now() AND m.enabled AND h.enabled")
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
}
