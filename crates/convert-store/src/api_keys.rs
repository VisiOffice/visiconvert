use crate::{Error, Result, Store};
use chrono::{DateTime, Utc};
use convert_domain::ValidationError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyInput {
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
#[derive(Clone)]
pub struct KeyAccess {
    pub store: Store,
    pub name: String,
    pub id: Uuid,
    pub actor: String,
    pub owner: Option<Uuid>,
    pub credential_version: i64,
    pub scopes: Vec<String>,
}
pub fn digest(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}
impl KeyInput {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
            || !self.scopes.iter().any(|s| s == "read")
            || self.scopes.len() > 5
            || self.scopes.iter().any(|s| {
                ![
                    "read",
                    "leads:write",
                    "bookings:write",
                    "numbers:write",
                    "setup:write",
                ]
                .contains(&s.as_str())
            })
            || self.expires_at.is_some_and(|d| d <= Utc::now())
        {
            return Err(ValidationError("Use a name, read permission, supported write permissions, and an optional future expiration").into());
        }
        Ok(())
    }
}
const METADATA: &str = "SELECT id,name,prefix,scopes,expires_at,revoked_at,created_at,last_used_at,version FROM workspace_api_keys WHERE workspace_id=$1 ORDER BY (revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())) DESC,created_at DESC,id LIMIT 200";
impl Store {
    pub async fn api_keys(&self) -> Result<Value> {
        let rows = sqlx::query(METADATA)
            .bind(self.workspace)
            .fetch_all(&self.pool)
            .await?;
        let keys:Vec<Value>=rows.iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"prefix":r.get::<String,_>("prefix"),"scopes":r.get::<Vec<String>,_>("scopes"),"expires_at":r.get::<Option<DateTime<Utc>>,_>("expires_at"),"revoked_at":r.get::<Option<DateTime<Utc>>,_>("revoked_at"),"created_at":r.get::<DateTime<Utc>,_>("created_at"),"last_used_at":r.get::<Option<DateTime<Utc>>,_>("last_used_at"),"version":r.get::<i32,_>("version")})).collect();
        let history:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(e)-'workspace_id' FROM api_key_events e WHERE workspace_id=$1 ORDER BY id DESC LIMIT 100").bind(self.workspace).fetch_all(&self.pool).await?;
        Ok(json!({"keys":keys,"history":history}))
    }
    /// Identity is rechecked under the identity lock also used by password reset.
    pub async fn issue_api_key(
        &self,
        input: &KeyInput,
        session: Option<&str>,
        rotate: Option<(Uuid, i32)>,
    ) -> Result<Value> {
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        let identity = if let Some(hash) = session {
            sqlx::query("SELECT u.user_id,u.credential_version FROM hosted_users u JOIN hosted_sessions s ON s.user_id=u.user_id AND s.credential_version=u.credential_version JOIN hosted_memberships m ON m.workspace_id=s.workspace_id AND m.user_id=s.user_id JOIN hosted_workspaces w ON w.workspace_id=m.workspace_id WHERE s.token_hash=$1 AND s.workspace_id=$2 AND s.expires_at>now() AND m.enabled AND m.role='owner' AND w.enabled FOR UPDATE OF u")
                .bind(hash).bind(self.workspace).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?.into()
        } else {
            None
        };
        self.lock_inquiry_writes(&mut tx).await?;
        let hosted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM hosted_workspaces WHERE workspace_id=$1)",
        )
        .bind(self.workspace)
        .fetch_one(&mut *tx)
        .await?;
        if hosted && identity.is_none() {
            return Err(Error::NotFound);
        }
        let owner: Option<Uuid> = identity
            .as_ref()
            .map(|r: &sqlx::postgres::PgRow| r.get("user_id"));
        let version = identity
            .as_ref()
            .map_or(0, |r| r.get::<i64, _>("credential_version"));
        let actor = owner.map_or("workspace operator".into(), |u| format!("hosted user {u}"));
        if let Some((id, rev)) = rotate {
            let changed=sqlx::query("UPDATE workspace_api_keys SET revoked_at=now(),version=version+1 WHERE workspace_id=$1 AND id=$2 AND version=$3 AND revoked_at IS NULL").bind(self.workspace).bind(id).bind(rev).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO api_key_events(workspace_id,key_id,kind,actor) VALUES($1,$2,'rotated',$3)").bind(self.workspace).bind(id).bind(&actor).execute(&mut *tx).await?;
        }
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM workspace_api_keys WHERE workspace_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())").bind(self.workspace).fetch_one(&mut *tx).await?;
        if count >= 50 {
            return Err(ValidationError(
                "Revoke an unused key before creating another (50 active keys maximum)",
            )
            .into());
        }
        let id = Uuid::new_v4();
        let secret = format!(
            "vc_key_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        sqlx::query("INSERT INTO workspace_api_keys(id,workspace_id,owner_id,credential_version,name,prefix,secret_hash,scopes,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(id).bind(self.workspace).bind(owner).bind(version).bind(input.name.trim()).bind(&secret[..15]).bind(digest(&secret)).bind(&input.scopes).bind(input.expires_at).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO api_key_events(workspace_id,key_id,kind,actor) VALUES($1,$2,'created',$3)",
        )
        .bind(self.workspace)
        .bind(id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(json!({"id":id,"secret":secret,"version":1}))
    }
    pub async fn revoke_api_key(&self, id: Uuid, version: i32, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let result=sqlx::query("UPDATE workspace_api_keys SET revoked_at=now(),version=version+1 WHERE workspace_id=$1 AND id=$2 AND version=$3 AND revoked_at IS NULL").bind(self.workspace).bind(id).bind(version).execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query(
            "INSERT INTO api_key_events(workspace_id,key_id,kind,actor) VALUES($1,$2,'revoked',$3)",
        )
        .bind(self.workspace)
        .bind(id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// No authentication cache: revocation, expiry, membership and password reset
    /// are consulted for each request. In-flight requests may finish.
    pub async fn authenticate_api_key(&self, secret: &str) -> Result<Option<KeyAccess>> {
        if !secret
            .strip_prefix("vc_key_")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Ok(None);
        }
        let row=sqlx::query("SELECT k.*,w.name AS workspace_name FROM workspace_api_keys k JOIN workspaces w ON w.id=k.workspace_id LEFT JOIN hosted_workspaces h ON h.workspace_id=k.workspace_id LEFT JOIN hosted_memberships m ON m.workspace_id=k.workspace_id AND m.user_id=k.owner_id LEFT JOIN hosted_users u ON u.user_id=k.owner_id WHERE k.secret_hash=$1 AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at>now()) AND ((k.owner_id IS NULL AND h.workspace_id IS NULL) OR (k.owner_id IS NOT NULL AND h.enabled AND m.enabled AND m.role='owner' AND u.credential_version=k.credential_version))")
            .bind(digest(secret)).fetch_optional(&self.pool).await?;
        let Some(r) = row else { return Ok(None) };
        let id: Uuid = r.get("id");
        let updated=sqlx::query("UPDATE workspace_api_keys SET last_used_at=now(),rate_count=CASE WHEN rate_started_at<=now()-interval '1 minute' THEN 1 ELSE rate_count+1 END,rate_started_at=CASE WHEN rate_started_at<=now()-interval '1 minute' THEN now() ELSE rate_started_at END WHERE id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now()) AND (rate_count<120 OR rate_started_at<=now()-interval '1 minute')")
            .bind(id).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(Error::RateLimited);
        }
        Ok(Some(KeyAccess {
            store: Store {
                pool: self.pool.clone(),
                workspace: r.get("workspace_id"),
            },
            name: r.get("workspace_name"),
            actor: format!("API key {id}"),
            id,
            owner: r.get("owner_id"),
            credential_version: r.get("credential_version"),
            scopes: r.get("scopes"),
        }))
    }
    pub async fn api_identity(&self, session: &str) -> Result<(Uuid, i64)> {
        let r=sqlx::query("SELECT user_id,credential_version FROM hosted_sessions WHERE token_hash=$1 AND workspace_id=$2 AND expires_at>now()")
            .bind(session).bind(self.workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)?;
        Ok((r.get("user_id"), r.get("credential_version")))
    }
}
