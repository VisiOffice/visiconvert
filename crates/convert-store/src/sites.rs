use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Serialize, sqlx::FromRow)]
pub struct Site {
    pub id: Uuid,
    pub name: String,
    pub origin: String,
    pub allowed_fields: Vec<String>,
    pub enabled: bool,
    pub version: i32,
    pub last_session_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_form_at: Option<chrono::DateTime<chrono::Utc>>,
    pub session_count: i64,
    pub form_count: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteInput {
    pub name: String,
    pub origin: String,
    pub allowed_fields: Vec<String>,
    pub enabled: bool,
    pub version: i32,
}
impl Store {
    pub async fn sites(&self) -> Result<Vec<Site>> {
        Ok(
            sqlx::query_as("SELECT * FROM sites WHERE workspace_id=$1 ORDER BY created_at")
                .bind(self.workspace)
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn save_site(&self, id: Option<Uuid>, input: &SiteInput) -> Result<Uuid> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        let id = if let Some(id) = id {
            let n = sqlx::query("UPDATE sites SET name=$3,origin=$4,allowed_fields=$5,enabled=$6,version=version+1 WHERE workspace_id=$1 AND id=$2 AND version=$7")
                .bind(self.workspace).bind(id).bind(&input.name).bind(&input.origin).bind(&input.allowed_fields).bind(input.enabled).bind(input.version).execute(&mut *tx).await?;
            if n.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
            id
        } else {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sites WHERE workspace_id=$1")
                .bind(self.workspace)
                .fetch_one(&mut *tx)
                .await?;
            if count >= 50 {
                return Err(ValidationError("A workspace supports up to 50 sites").into());
            }
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO sites(id,workspace_id,name,origin,allowed_fields,enabled) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(id).bind(self.workspace).bind(&input.name).bind(&input.origin).bind(&input.allowed_fields).bind(input.enabled).execute(&mut *tx).await?;
            id
        };
        tx.commit().await?;
        Ok(id)
    }
    // Public IDs grant only collection. They never select a private API workspace.
    pub async fn public_site(&self, id: Uuid, origin: &str) -> Result<Site> {
        sqlx::query_as("SELECT * FROM sites WHERE id=$1 AND origin=$2 AND enabled")
            .bind(id)
            .bind(origin)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn capture_session(
        &self,
        site: Uuid,
        origin: &str,
        id: Uuid,
        evidence: &Value,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        collection_site(&mut tx, site, origin).await?;
        let previous: Option<Value> =
            sqlx::query_scalar("SELECT evidence FROM website_sessions WHERE site_id=$1 AND id=$2")
                .bind(site)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(previous) = previous {
            return if previous == *evidence {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        rate_limit(&mut tx, site).await?;
        // Unconverted sessions expire from storage after 30 days. Bound each cleanup batch.
        sqlx::query("DELETE FROM website_sessions WHERE (site_id,id) IN (SELECT s.site_id,s.id FROM website_sessions s WHERE s.site_id=$1 AND s.created_at < now()-interval '30 days' AND NOT EXISTS(SELECT 1 FROM website_submissions f WHERE f.site_id=s.site_id AND f.session_id=s.id) LIMIT 100)")
            .bind(site).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO website_sessions(site_id,id,evidence) VALUES($1,$2,$3)")
            .bind(site)
            .bind(id)
            .bind(evidence)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE sites SET session_count=session_count+1,last_session_at=now() WHERE id=$1",
        )
        .bind(site)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn capture_form(
        &self,
        site: Uuid,
        origin: &str,
        session: Uuid,
        external_id: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let row = collection_site(&mut tx, site, origin).await?;
        let allowed: Vec<String> = row.get("allowed_fields");
        if fields.keys().any(|key| !allowed.contains(key)) {
            return Err(ValidationError("A submitted field is not allowed for this site").into());
        }
        let payload = json!({"session_id":session,"fields":fields});
        let previous: Option<Value> = sqlx::query_scalar(
            "SELECT payload FROM website_submissions WHERE site_id=$1 AND external_id=$2",
        )
        .bind(site)
        .bind(external_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(previous) = previous {
            return if previous == payload {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let evidence: Value = sqlx::query_scalar("SELECT evidence FROM website_sessions WHERE site_id=$1 AND id=$2 AND created_at > now()-interval '30 minutes'")
            .bind(site).bind(session).fetch_optional(&mut *tx).await?.ok_or(ValidationError("Session missing or expired; start a new session"))?;
        rate_limit(&mut tx, site).await?;
        let workspace: Uuid = row.get("workspace_id");
        let lead = Uuid::new_v4();
        let name = fields
            .get("name")
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str)
            .unwrap_or("Website inquiry");
        let contact = fields
            .get("email")
            .or_else(|| fields.get("phone"))
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str)
            .unwrap_or("Not supplied");
        sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,submission_payload) VALUES($1,$2,$3,$4,$5,'form',$6,$7,$8)")
            .bind(lead).bind(workspace).bind(format!("website:{site}:{external_id}"))
            .bind(name).bind(contact).bind(evidence["source"].as_str().unwrap_or("Unknown"))
            .bind(evidence["utm_campaign"].as_str().unwrap_or("")).bind(&payload).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO website_submissions(site_id,external_id,session_id,workspace_id,lead_id,payload) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(site).bind(external_id).bind(session).bind(workspace).bind(lead).bind(payload).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) VALUES($1,$2,'website_form_captured',$3)")
            .bind(workspace).bind(lead).bind(json!({"site_id":site,"session_id":session})).execute(&mut *tx).await?;
        sqlx::query("UPDATE sites SET form_count=form_count+1,last_form_at=now() WHERE id=$1")
            .bind(site)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn website_evidence(&self, lead: Uuid) -> Result<Option<Value>> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('site_name',s.name,'origin',s.origin,'session_id',v.id,'session_started_at',v.created_at,'submitted_at',f.created_at,'evidence',v.evidence,'fields',f.payload->'fields') FROM website_submissions f JOIN sites s ON s.id=f.site_id JOIN website_sessions v ON v.site_id=f.site_id AND v.id=f.session_id WHERE f.workspace_id=$1 AND f.lead_id=$2")
            .bind(self.workspace).bind(lead).fetch_optional(&self.pool).await?)
    }
}
async fn collection_site(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    site: Uuid,
    origin: &str,
) -> Result<sqlx::postgres::PgRow> {
    sqlx::query("SELECT * FROM sites WHERE id=$1 AND origin=$2 AND enabled FOR UPDATE")
        .bind(site)
        .bind(origin)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::NotFound)
}
async fn rate_limit(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, site: Uuid) -> Result<()> {
    let row = sqlx::query("UPDATE sites SET rate_count=CASE WHEN rate_started_at < now()-interval '1 minute' THEN 1 ELSE rate_count+1 END,rate_started_at=CASE WHEN rate_started_at < now()-interval '1 minute' THEN now() ELSE rate_started_at END WHERE id=$1 AND (rate_count<120 OR rate_started_at<now()-interval '1 minute')")
        .bind(site).execute(&mut **tx).await?;
    if row.rows_affected() == 0 {
        return Err(Error::RateLimited);
    }
    Ok(())
}
