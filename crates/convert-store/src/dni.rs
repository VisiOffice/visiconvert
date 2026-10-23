use super::*;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolInput {
    pub fallback_phone: String,
    pub enabled: bool,
    pub version: i32,
    pub number_ids: Vec<Uuid>,
    pub dedicated_numbers: bool,
}
impl Store {
    pub async fn dni_pool(&self, site: Uuid) -> Result<Value> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sites WHERE workspace_id=$1 AND id=$2)",
        )
        .bind(self.workspace)
        .bind(site)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(Error::NotFound);
        }
        let config: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(p) - 'workspace_id' FROM dni_pools p WHERE workspace_id=$1 AND site_id=$2").bind(self.workspace).bind(site).fetch_optional(&self.pool).await?;
        let numbers: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',n.id,'label',n.label,'phone_number',n.phone_number,'destination',n.destination,'site_id',p.site_id,'enabled',COALESCE(p.enabled,false),'reserved',EXISTS(SELECT 1 FROM dni_assignments a WHERE a.number_id=n.id AND a.reusable_at>clock_timestamp())) FROM tracking_numbers n LEFT JOIN dni_pool_numbers p ON p.number_id=n.id WHERE n.workspace_id=$1 AND NOT n.is_demo ORDER BY n.label,n.id").bind(self.workspace).fetch_all(&self.pool).await?;
        let history: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'phone_number',n.phone_number,'assigned_at',a.assigned_at,'expires_at',a.expires_at,'reusable_at',a.reusable_at,'revoked_at',a.revoked_at,'source',a.evidence->>'source','campaign',a.evidence->>'utm_campaign') FROM dni_assignments a JOIN tracking_numbers n ON n.id=a.number_id WHERE a.workspace_id=$1 AND a.site_id=$2 ORDER BY a.assigned_at DESC,a.id LIMIT 50").bind(self.workspace).bind(site).fetch_all(&self.pool).await?;
        Ok(
            json!({"pool":config,"numbers":numbers,"history":history,"lease_minutes":30,"reuse_delay_minutes":30}),
        )
    }
    pub async fn save_dni_pool(&self, site: Uuid, input: &PoolInput) -> Result<()> {
        us_number(&input.fallback_phone)?;
        let mut ids = input.number_ids.clone();
        ids.sort();
        ids.dedup();
        if ids.len() != input.number_ids.len()
            || ids.len() > 50
            || input.version < 0
            || (input.enabled && (ids.is_empty() || !input.dedicated_numbers))
        {
            return Err(ValidationError(
                "Select 1–50 dedicated numbers to enable DNI and supply a valid version",
            )
            .into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        sqlx::query("SELECT id FROM sites WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
            .bind(self.workspace)
            .bind(site)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let old: Option<i32> = sqlx::query_scalar(
            "SELECT version FROM dni_pools WHERE workspace_id=$1 AND site_id=$2",
        )
        .bind(self.workspace)
        .bind(site)
        .fetch_optional(&mut *tx)
        .await?;
        if old.unwrap_or(0) != input.version {
            return Err(Error::Conflict);
        }
        let fallback_is_tracking: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE phone_number=$1)",
        )
        .bind(&input.fallback_phone)
        .fetch_one(&mut *tx)
        .await?;
        if fallback_is_tracking {
            return Err(ValidationError(
                "Fallback must be the business destination, not a registered tracking number",
            )
            .into());
        }
        for id in &ids {
            let valid: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers n LEFT JOIN dni_pool_numbers p ON p.number_id=n.id WHERE n.workspace_id=$1 AND n.id=$2 AND NOT n.is_demo AND n.destination=$3 AND (p.site_id IS NULL OR p.site_id=$4))").bind(self.workspace).bind(id).bind(&input.fallback_phone).bind(site).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(ValidationError("Pool numbers must belong to this workspace, forward to the fallback, and be dedicated to this site").into());
            }
        }
        sqlx::query("INSERT INTO dni_pools(site_id,workspace_id,fallback_phone,enabled) VALUES($1,$2,$3,$4) ON CONFLICT(site_id) DO UPDATE SET fallback_phone=$3,enabled=$4,version=dni_pools.version+1").bind(site).bind(self.workspace).bind(&input.fallback_phone).bind(input.enabled).execute(&mut *tx).await?;
        sqlx::query("UPDATE dni_pool_numbers SET enabled=false WHERE site_id=$1")
            .bind(site)
            .execute(&mut *tx)
            .await?;
        for id in &ids {
            sqlx::query("INSERT INTO dni_pool_numbers(number_id,site_id,workspace_id,enabled) VALUES($1,$2,$3,true) ON CONFLICT(number_id) DO UPDATE SET enabled=true").bind(id).bind(site).bind(self.workspace).execute(&mut *tx).await?;
        }
        // Revocation never frees a number early; a visitor may have saved it.
        sqlx::query("UPDATE dni_assignments SET revoked_at=clock_timestamp() WHERE site_id=$1 AND revoked_at IS NULL AND (NOT $2 OR NOT (number_id=ANY($3)))").bind(site).bind(input.enabled).bind(&ids).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn allocate_dni(&self, site: Uuid, origin: &str, session: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        // All allocation/configuration paths lock this site's row. A number is
        // permanently associated with one site, so concurrent leases cannot overlap.
        super::sites::collection_site(&mut tx, site, origin).await?;
        let context=sqlx::query("SELECT evidence,created_at+interval '30 minutes' AS expires_at FROM website_sessions WHERE site_id=$1 AND id=$2 AND created_at>clock_timestamp()-interval '29 minutes' AND starts_with(evidence->>'landing_url',$3)").bind(site).bind(session).bind(format!("{origin}/")).fetch_optional(&mut *tx).await?.ok_or(ValidationError("Session missing or expired; start a new session"))?;
        let pool = sqlx::query("SELECT * FROM dni_pools WHERE site_id=$1")
            .bind(site)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(pool) = pool else {
            return Ok(json!({"status":"not_configured"}));
        };
        let fallback: String = pool.get("fallback_phone");
        if !pool.get::<bool, _>("enabled") {
            return Ok(json!({"status":"disabled","fallback_phone":fallback}));
        }
        if let Some(row)=sqlx::query("SELECT a.*,n.phone_number FROM dni_assignments a JOIN tracking_numbers n ON n.id=a.number_id WHERE a.site_id=$1 AND a.session_id=$2").bind(site).bind(session).fetch_optional(&mut *tx).await? {
            if row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("revoked_at").is_none() && row.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")>chrono::Utc::now() {
                return Ok(lease(&row,&fallback));
            }
            return Ok(json!({"status":"expired","fallback_phone":fallback}));
        }
        super::sites::rate_limit(&mut tx, site).await?;
        let number=sqlx::query("SELECT n.id,n.phone_number FROM dni_pool_numbers p JOIN tracking_numbers n ON n.id=p.number_id WHERE p.site_id=$1 AND p.enabled AND NOT EXISTS(SELECT 1 FROM dni_assignments a WHERE a.number_id=p.number_id AND a.reusable_at>clock_timestamp()) ORDER BY (SELECT max(a.assigned_at) FROM dni_assignments a WHERE a.number_id=p.number_id) ASC NULLS FIRST,n.id LIMIT 1").bind(site).fetch_optional(&mut *tx).await?;
        let Some(number) = number else {
            sqlx::query("UPDATE dni_pools SET fallback_count=fallback_count+1 WHERE site_id=$1")
                .bind(site)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(json!({"status":"exhausted","fallback_phone":fallback}));
        };
        let row=sqlx::query("INSERT INTO dni_assignments(id,workspace_id,site_id,session_id,number_id,expires_at,reusable_at,evidence) VALUES($1,$2,$3,$4,$5,$6,$6+interval '30 minutes',$7) RETURNING *, $8::text AS phone_number")
            .bind(Uuid::new_v4()).bind(pool.get::<Uuid,_>("workspace_id")).bind(site).bind(session).bind(number.get::<Uuid,_>("id"))
            .bind(context.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")).bind(context.get::<Value,_>("evidence")).bind(number.get::<String,_>("phone_number")).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(lease(&row, &fallback))
    }
    pub async fn dni_evidence(&self, lead: Uuid) -> Result<Option<Value>> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('status',status,'assignment_id',assignment_id,'evidence',payload) FROM dni_call_evidence WHERE workspace_id=$1 AND lead_id=$2").bind(self.workspace).bind(lead).fetch_optional(&self.pool).await?)
    }
    pub(crate) async fn attribute_dni_call(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        lead: Uuid,
        number: Uuid,
        voice: bool,
    ) -> Result<()> {
        // A single statement snapshot captures the observed assignment. No carrier
        // network request or pool allocation is performed in the forwarding path.
        let row=sqlx::query("SELECT p.site_id,s.name,s.origin,p.enabled AND pool.enabled AND s.enabled AS enabled,a.id,a.session_id,a.assigned_at,a.expires_at,a.evidence,EXISTS(SELECT 1 FROM dni_assignments old WHERE old.number_id=p.number_id AND old.id<>a.id) AS reused FROM dni_pool_numbers p JOIN dni_pools pool ON pool.site_id=p.site_id JOIN sites s ON s.id=p.site_id LEFT JOIN dni_assignments a ON a.number_id=p.number_id AND a.assigned_at<=clock_timestamp() AND a.expires_at>clock_timestamp() AND a.revoked_at IS NULL WHERE p.workspace_id=$1 AND p.number_id=$2 ORDER BY a.assigned_at DESC LIMIT 1").bind(self.workspace).bind(number).fetch_optional(&mut **tx).await?;
        let Some(row) = row else {
            return Ok(());
        };
        let assignment: Option<Uuid> = row.get("id");
        let matched = voice && row.get::<bool, _>("enabled") && assignment.is_some();
        let status = if !voice {
            "late_or_out_of_order"
        } else if !row.get::<bool, _>("enabled") {
            "pool_disabled"
        } else if assignment.is_none() {
            "no_active_assignment"
        } else {
            "inferred_window"
        };
        let mut payload = json!({"model":"dni-observed-window-v1","confidence":if matched {"inferred"} else {"unknown"},"site_name":row.get::<String,_>("name"),"origin":row.get::<String,_>("origin"),"observed_at":chrono::Utc::now(),"warning":"Timing is not proof of caller identity. Saved numbers and delayed webhooks can belong to an earlier visitor."});
        let (source, campaign) = if matched {
            let evidence: Value = row.get("evidence");
            payload["session_id"] = json!(row.get::<Option<Uuid>, _>("session_id"));
            payload["assigned_at"] =
                json!(row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("assigned_at"));
            payload["expires_at"] =
                json!(row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("expires_at"));
            payload["number_reused"] = json!(row.get::<bool, _>("reused"));
            payload["session_entry"] = evidence.clone();
            (
                evidence["source"]
                    .as_str()
                    .unwrap_or("Direct / unknown")
                    .to_owned(),
                evidence["utm_campaign"].as_str().unwrap_or("").to_owned(),
            )
        } else {
            ("Unknown / pooled call".to_owned(), String::new())
        };
        sqlx::query("UPDATE leads SET source=$3,campaign=$4 WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace)
            .bind(lead)
            .bind(source)
            .bind(campaign)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO dni_call_evidence(workspace_id,lead_id,assignment_id,status,payload) VALUES($1,$2,$3,$4,$5)").bind(self.workspace).bind(lead).bind(if matched {assignment} else {None}).bind(status).bind(payload).execute(&mut **tx).await?;
        Ok(())
    }
}
fn lease(row: &sqlx::postgres::PgRow, fallback: &str) -> Value {
    json!({"status":"assigned","phone_number":row.get::<String,_>("phone_number"),"fallback_phone":fallback,"expires_at":row.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")})
}
