use super::*;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolInput {
    pub fallback_phone: String,
    /// Omitted by older clients: preserve an existing destination, or use the
    /// published fallback when creating a legacy pool.
    #[serde(default)]
    pub destination_phone: Option<String>,
    pub enabled: bool,
    pub version: i32,
    pub number_ids: Vec<Uuid>,
    pub dedicated_numbers: bool,
    /// Edit this pool. Omitted with `new_pool` false: the site's first pool,
    /// which is what single-pool clients mean.
    #[serde(default)]
    pub pool_id: Option<Uuid>,
    /// Add another pool to the site. Each pool replaces a different published number.
    #[serde(default)]
    pub new_pool: bool,
}
/// Most published numbers one page can ask to swap in a single request.
pub const MAX_TARGETS: usize = 10;
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
        let pools: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(p) - 'workspace_id' || jsonb_build_object('number_ids',COALESCE((SELECT jsonb_agg(n.number_id ORDER BY n.number_id) FROM dni_pool_numbers n WHERE n.pool_id=p.id AND n.enabled),'[]'::jsonb)) FROM dni_pools p WHERE workspace_id=$1 AND site_id=$2 ORDER BY created_at,id").bind(self.workspace).bind(site).fetch_all(&self.pool).await?;
        let numbers: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',n.id,'label',n.label,'phone_number',n.phone_number,'destination',n.destination,'site_id',p.site_id,'pool_id',p.pool_id,'enabled',COALESCE(p.enabled,false),'used_as_fallback',EXISTS(SELECT 1 FROM dni_pools f WHERE f.fallback_phone=n.phone_number),'reserved',EXISTS(SELECT 1 FROM dni_assignments a WHERE a.number_id=n.id AND a.reusable_at>clock_timestamp())) FROM tracking_numbers n LEFT JOIN dni_pool_numbers p ON p.number_id=n.id WHERE n.workspace_id=$1 AND NOT n.is_demo ORDER BY n.label,n.id").bind(self.workspace).fetch_all(&self.pool).await?;
        let history: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'pool_id',a.pool_id,'fallback_phone',pool.fallback_phone,'phone_number',n.phone_number,'assigned_at',a.assigned_at,'expires_at',a.expires_at,'reusable_at',a.reusable_at,'revoked_at',a.revoked_at,'source',a.evidence->>'source','campaign',a.evidence->>'utm_campaign') FROM dni_assignments a JOIN tracking_numbers n ON n.id=a.number_id JOIN dni_pools pool ON pool.id=a.pool_id WHERE a.workspace_id=$1 AND a.site_id=$2 ORDER BY a.assigned_at DESC,a.id LIMIT 50").bind(self.workspace).bind(site).fetch_all(&self.pool).await?;
        // `pool` is the first pool, for single-pool clients.
        Ok(
            json!({"pool":pools.first(),"pools":pools,"numbers":numbers,"history":history,"lease_minutes":30,"reuse_delay_minutes":30}),
        )
    }
    pub async fn save_dni_pool(&self, site: Uuid, input: &PoolInput) -> Result<Uuid> {
        us_number(&input.fallback_phone)?;
        let mut ids = input.number_ids.clone();
        ids.sort();
        ids.dedup();
        if ids.len() != input.number_ids.len()
            || ids.len() > 50
            || input.version < 0
            || (input.enabled && (ids.is_empty() || !input.dedicated_numbers))
            || (input.new_pool && (input.pool_id.is_some() || input.version != 0))
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
        let old = if input.new_pool {
            None
        } else {
            sqlx::query(
                "SELECT id,version,fallback_phone,destination_phone FROM dni_pools WHERE workspace_id=$1 AND site_id=$2 AND ($3::uuid IS NULL OR id=$3) ORDER BY created_at,id LIMIT 1",
            )
            .bind(self.workspace)
            .bind(site)
            .bind(input.pool_id)
            .fetch_optional(&mut *tx)
            .await?
        };
        if input.pool_id.is_some() && old.is_none() {
            return Err(Error::NotFound);
        }
        let pool_id = old
            .as_ref()
            .map_or_else(Uuid::new_v4, |r| r.get::<Uuid, _>("id"));
        if old.as_ref().map_or(0, |r| r.get::<i32, _>("version")) != input.version {
            return Err(Error::Conflict);
        }
        // Use the same lock as number registration for cross-workspace checks.
        sqlx::query("SELECT pg_advisory_xact_lock(73581001)")
            .execute(&mut *tx)
            .await?;
        let destination = input
            .destination_phone
            .clone()
            .or_else(|| {
                old.as_ref()
                    .map(|r| r.get::<String, _>("destination_phone"))
            })
            .unwrap_or_else(|| input.fallback_phone.clone());
        us_number(&destination)?;
        let destination_is_tracking: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE phone_number=$1 AND released_at IS NULL)",
        )
        .bind(&destination)
        .fetch_one(&mut *tx)
        .await?;
        if destination_is_tracking {
            return Err(ValidationError(
                "Forwarding destination must be a business line, not a registered tracking number",
            )
            .into());
        }
        // A published static number can be replaced, but a pooled number must
        // never be the fallback. Do not borrow another tenant's static number.
        let invalid_fallback: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers n LEFT JOIN dni_pool_numbers p ON p.number_id=n.id WHERE n.phone_number=$1 AND n.released_at IS NULL AND (n.workspace_id<>$2 OR n.is_demo OR n.release_requested_at IS NOT NULL OR p.number_id IS NOT NULL OR n.destination<>$3 OR n.id=ANY($4)))")
            .bind(&input.fallback_phone).bind(self.workspace).bind(&destination).bind(&ids).fetch_one(&mut *tx).await?;
        if invalid_fallback {
            return Err(ValidationError("The website number must be outside all pools; a registered static number must belong to this workspace and use the same forwarding destination").into());
        }
        let duplicate: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM dni_pools WHERE site_id=$1 AND fallback_phone=$2 AND id<>$3)",
        )
        .bind(site)
        .bind(&input.fallback_phone)
        .bind(pool_id)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            return Err(ValidationError(
                "Another pool on this site already replaces that website number",
            )
            .into());
        }
        for id in &ids {
            let valid: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers n LEFT JOIN dni_pool_numbers p ON p.number_id=n.id WHERE n.workspace_id=$1 AND n.id=$2 AND NOT n.is_demo AND n.release_requested_at IS NULL AND n.destination=$3 AND (p.site_id IS NULL OR (p.site_id=$4 AND (p.pool_id=$5 OR NOT p.enabled))) AND NOT EXISTS(SELECT 1 FROM dni_pools f WHERE f.fallback_phone=n.phone_number))").bind(self.workspace).bind(id).bind(&destination).bind(site).bind(pool_id).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(ValidationError("Pool numbers must belong to this workspace, use the selected forwarding destination, be dedicated to this site, and be removed from any other pool first").into());
            }
        }
        let changed = old.as_ref().is_some_and(|r| {
            r.get::<String, _>("fallback_phone") != input.fallback_phone
                || r.get::<String, _>("destination_phone") != destination
        });
        sqlx::query("INSERT INTO dni_pools(id,site_id,workspace_id,fallback_phone,destination_phone,enabled) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET fallback_phone=$4,destination_phone=$5,enabled=$6,version=dni_pools.version+1").bind(pool_id).bind(site).bind(self.workspace).bind(&input.fallback_phone).bind(&destination).bind(input.enabled).execute(&mut *tx).await?;
        sqlx::query("UPDATE dni_pool_numbers SET enabled=false WHERE pool_id=$1")
            .bind(pool_id)
            .execute(&mut *tx)
            .await?;
        for id in &ids {
            sqlx::query("INSERT INTO dni_pool_numbers(number_id,site_id,workspace_id,pool_id,enabled) VALUES($1,$2,$3,$4,true) ON CONFLICT(number_id) DO UPDATE SET enabled=true,pool_id=$4").bind(id).bind(site).bind(self.workspace).bind(pool_id).execute(&mut *tx).await?;
        }
        // Revocation never frees a number early; a visitor may have saved it.
        sqlx::query("UPDATE dni_assignments SET revoked_at=clock_timestamp() WHERE pool_id=$1 AND revoked_at IS NULL AND (NOT $2 OR NOT (number_id=ANY($3)) OR $4)").bind(pool_id).bind(input.enabled).bind(&ids).bind(changed).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(pool_id)
    }
    /// Single-pool clients (no `targets`): lease from the site's first pool and
    /// answer in the original shape. With `targets` (the published numbers marked
    /// on the page): lease one number from each pool replacing one of them.
    pub async fn allocate_dni(
        &self,
        site: Uuid,
        origin: &str,
        session: Uuid,
        targets: Option<&[String]>,
    ) -> Result<Value> {
        let mut wanted = Vec::new();
        if let Some(targets) = targets {
            for target in targets {
                us_number(target)?;
                if !wanted.contains(target) {
                    wanted.push(target.clone());
                }
            }
            if wanted.len() > MAX_TARGETS {
                return Err(ValidationError("Too many website numbers on one page").into());
            }
        }
        let mut tx = self.pool.begin().await?;
        // All allocation/configuration paths lock this site's row. A number is
        // permanently associated with one site, so concurrent leases cannot overlap.
        super::sites::collection_site(&mut tx, site, origin).await?;
        let context=sqlx::query("SELECT evidence,created_at+interval '30 minutes' AS expires_at FROM website_sessions WHERE site_id=$1 AND id=$2 AND created_at>clock_timestamp()-interval '29 minutes' AND starts_with(evidence->>'landing_url',$3)").bind(site).bind(session).bind(format!("{origin}/")).fetch_optional(&mut *tx).await?.ok_or(ValidationError("Session missing or expired; start a new session"))?;
        let pools = sqlx::query(
            "SELECT * FROM dni_pools WHERE site_id=$1 AND ($2::text[] IS NULL OR fallback_phone=ANY($2)) ORDER BY created_at,id",
        )
        .bind(site)
        .bind(targets.map(|_| wanted.clone()))
        .fetch_all(&mut *tx)
        .await?;
        let legacy = targets.is_none();
        let pools = if legacy {
            pools.into_iter().take(1).collect()
        } else {
            pools
        };
        if legacy && pools.is_empty() {
            return Ok(json!({"status":"not_configured"}));
        }
        let forwarding: bool = sqlx::query_scalar(
            "SELECT managed_forwarding_allowed(workspace_id) FROM sites WHERE id=$1",
        )
        .bind(site)
        .fetch_one(&mut *tx)
        .await?;
        let mut assignments = Vec::new();
        for pool in &pools {
            let mut assignment = self
                .lease_from(&mut tx, site, session, &context, pool, forwarding)
                .await?;
            if !legacy {
                assignment["replaces"] = json!(pool.get::<String, _>("fallback_phone"));
            }
            assignments.push(assignment);
        }
        tx.commit().await?;
        if legacy {
            return Ok(assignments
                .pop()
                .unwrap_or_else(|| json!({"status":"not_configured"})));
        }
        Ok(json!({"status":"ok","assignments":assignments}))
    }
    async fn lease_from(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        site: Uuid,
        session: Uuid,
        context: &sqlx::postgres::PgRow,
        pool: &sqlx::postgres::PgRow,
        forwarding: bool,
    ) -> Result<Value> {
        let pool_id: Uuid = pool.get("id");
        let fallback: String = pool.get("fallback_phone");
        if !forwarding {
            // Do not return an existing lease or a suspended tracking fallback.
            let destination: String = pool.get("destination_phone");
            return Ok(json!({"status":"disabled","fallback_phone":destination}));
        }
        if !pool.get::<bool, _>("enabled") {
            return Ok(json!({"status":"disabled","fallback_phone":fallback}));
        }
        if let Some(row)=sqlx::query("SELECT a.*,n.phone_number FROM dni_assignments a JOIN tracking_numbers n ON n.id=a.number_id WHERE a.site_id=$1 AND a.session_id=$2 AND a.pool_id=$3").bind(site).bind(session).bind(pool_id).fetch_optional(&mut **tx).await? {
            if row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("revoked_at").is_none() && row.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")>chrono::Utc::now() {
                return Ok(lease(&row,&fallback));
            }
            return Ok(json!({"status":"expired","fallback_phone":fallback}));
        }
        super::sites::rate_limit(tx, site).await?;
        let number=sqlx::query("SELECT n.id,n.phone_number FROM dni_pool_numbers p JOIN tracking_numbers n ON n.id=p.number_id WHERE p.pool_id=$1 AND p.enabled AND n.release_requested_at IS NULL AND NOT EXISTS(SELECT 1 FROM dni_assignments a WHERE a.number_id=p.number_id AND a.reusable_at>clock_timestamp()) ORDER BY (SELECT max(a.assigned_at) FROM dni_assignments a WHERE a.number_id=p.number_id) ASC NULLS FIRST,n.id LIMIT 1").bind(pool_id).fetch_optional(&mut **tx).await?;
        let Some(number) = number else {
            sqlx::query("UPDATE dni_pools SET fallback_count=fallback_count+1 WHERE id=$1")
                .bind(pool_id)
                .execute(&mut **tx)
                .await?;
            return Ok(json!({"status":"exhausted","fallback_phone":fallback}));
        };
        let row=sqlx::query("INSERT INTO dni_assignments(id,workspace_id,site_id,session_id,number_id,pool_id,expires_at,reusable_at,evidence) VALUES($1,$2,$3,$4,$5,$6,$7,$7+interval '30 minutes',$8) RETURNING *, $9::text AS phone_number")
            .bind(Uuid::new_v4()).bind(pool.get::<Uuid,_>("workspace_id")).bind(site).bind(session).bind(number.get::<Uuid,_>("id")).bind(pool_id)
            .bind(context.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")).bind(context.get::<Value,_>("evidence")).bind(number.get::<String,_>("phone_number")).fetch_one(&mut **tx).await?;
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
        let row=sqlx::query("SELECT p.site_id,s.name,s.origin,p.enabled AND pool.enabled AND s.enabled AS enabled,a.id,a.session_id,a.assigned_at,a.expires_at,a.evidence,EXISTS(SELECT 1 FROM dni_assignments old WHERE old.number_id=p.number_id AND old.id<>a.id) AS reused FROM dni_pool_numbers p JOIN dni_pools pool ON pool.id=p.pool_id JOIN sites s ON s.id=p.site_id LEFT JOIN dni_assignments a ON a.number_id=p.number_id AND a.assigned_at<=clock_timestamp() AND a.expires_at>clock_timestamp() AND a.revoked_at IS NULL WHERE p.workspace_id=$1 AND p.number_id=$2 ORDER BY a.assigned_at DESC LIMIT 1").bind(self.workspace).bind(number).fetch_optional(&mut **tx).await?;
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
