//! Explicit operator adoption, separate from provisioning or credential rotation.
use crate::{Error, Result, Store, service_policy::ServicePolicy};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

impl Store {
    async fn adoption_snapshot(&self, tx: &mut Transaction<'_, Postgres>) -> Result<Value> {
        let plan: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('workspace_id',workspace_id,'project_id',project_id,'space_host',space_host,'connection_revision',revision) FROM signalwire_connections WHERE workspace_id=$1 AND NOT managed AND last_webhook_at IS NOT NULL")
            .bind(self.workspace).fetch_optional(&mut **tx).await?;
        let mut plan = plan.ok_or(Error::Conflict)?;
        let project = Uuid::parse_str(plan["project_id"].as_str().ok_or(Error::Conflict)?)
            .map_err(|_| Error::Conflict)?;
        // Connection writers take this same project lock. A shared BYO project
        // cannot be converted into a managed project for just one tenant.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(project.to_string())
            .execute(&mut **tx)
            .await?;
        let unsafe_state: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM signalwire_connections WHERE project_id=$2 AND workspace_id<>$1) OR EXISTS(SELECT 1 FROM number_purchases WHERE workspace_id=$1 AND (status IN ('pending','cleanup','review') OR reservation_lookup_pending OR (reservation_id IS NOT NULL AND reservation_settled_at IS NULL))) OR EXISTS(SELECT 1 FROM managed_service_policies WHERE workspace_id=$1)")
            .bind(self.workspace).bind(project).fetch_one(&mut **tx).await?;
        if unsafe_state {
            return Err(Error::Conflict);
        }
        let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',n.id,'sid',n.carrier_sid,'phone',n.phone_number,'destination',n.destination,'request_id',p.request_id) FROM tracking_numbers n JOIN number_purchases p ON p.workspace_id=n.workspace_id AND p.number_id=n.id WHERE n.workspace_id=$1 AND n.provider='signalwire' AND NOT n.is_demo AND n.released_at IS NULL AND n.release_requested_at IS NULL AND n.release_started_at IS NULL AND n.carrier_sid IS NOT NULL AND n.purchased_at IS NOT NULL AND n.managed_adopted_at IS NULL AND p.project_id=$2 AND NOT p.managed AND p.status='registered' ORDER BY n.id")
            .bind(self.workspace).bind(project).fetch_all(&mut **tx).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tracking_numbers WHERE workspace_id=$1 AND provider='signalwire' AND NOT is_demo AND released_at IS NULL")
            .bind(self.workspace).fetch_one(&mut **tx).await?;
        let purchases: i64 = sqlx::query_scalar("SELECT count(*) FROM number_purchases WHERE workspace_id=$1 AND project_id=$2 AND status='registered'")
            .bind(self.workspace).bind(project).fetch_one(&mut **tx).await?;
        if rows.is_empty() || rows.len() as i64 != count || purchases != count {
            return Err(Error::Conflict);
        }
        plan["numbers"] = json!(rows);
        Ok(plan)
    }
    pub async fn adoption_plan(&self) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        self.adoption_snapshot(&mut tx).await
    }
    pub async fn adoption_receipt(&self, id: Uuid, plan: &Value) -> Result<Option<Value>> {
        let row: Option<Value> = sqlx::query_scalar("SELECT receipt FROM managed_carrier_adoptions WHERE workspace_id=$1 AND adoption_id=$2 AND plan=$3")
            .bind(self.workspace).bind(id).bind(plan).fetch_optional(&self.pool).await?;
        Ok(row)
    }
    pub async fn adopt_carrier(
        &self,
        id: Uuid,
        plan: &Value,
        policy: &ServicePolicy,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        if let Some(receipt) = sqlx::query_scalar::<_,Value>("SELECT receipt FROM managed_carrier_adoptions WHERE workspace_id=$1 AND adoption_id=$2 AND plan=$3 AND policy=$4")
            .bind(self.workspace).bind(id).bind(plan).bind(serde_json::to_value(policy).map_err(|_|Error::Conflict)?).fetch_optional(&mut *tx).await? { return Ok(receipt); }
        if self.adoption_snapshot(&mut tx).await? != *plan {
            return Err(Error::Conflict);
        }
        let now = Utc::now().timestamp();
        if plan["project_id"] != policy.project_id.to_string()
            || policy.revision != 1
            || policy.state != "active"
            || policy.reason != "paid"
            || policy.release_at.is_some()
            || policy.valid_until <= now
            || policy.valid_until > now + 86460
        {
            return Err(Error::Conflict);
        }
        let revision: i64 = sqlx::query_scalar("UPDATE workspaces SET signalwire_connection_revision=signalwire_connection_revision+1 WHERE id=$1 RETURNING signalwire_connection_revision")
            .bind(self.workspace).fetch_one(&mut *tx).await?;
        sqlx::query(
            "UPDATE signalwire_connections SET managed=true,revision=$2 WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
        // Keep purchased_at, encrypted secrets, webhook evidence, routing and leads.
        // Round activation UP to the next second so no pre-activation fraction bills.
        let activated = now + 1;
        sqlx::query("UPDATE tracking_numbers SET managed_adopted_at=to_timestamp($2) WHERE workspace_id=$1 AND provider='signalwire' AND released_at IS NULL AND NOT is_demo")
            .bind(self.workspace).bind(activated as f64).execute(&mut *tx).await?;
        sqlx::query("UPDATE number_purchases SET managed=true WHERE workspace_id=$1 AND project_id=$2 AND status='registered' AND number_id IN (SELECT id FROM tracking_numbers WHERE workspace_id=$1 AND managed_adopted_at IS NOT NULL)")
            .bind(self.workspace).bind(policy.project_id).execute(&mut *tx).await?;
        let payload = serde_json::to_value(policy).map_err(|_| Error::Conflict)?;
        sqlx::query("INSERT INTO managed_service_policies(workspace_id,project_id,revision,state,reason,valid_until,payload) VALUES($1,$2,1,'active','paid',to_timestamp($3),$4)")
            .bind(self.workspace).bind(policy.project_id).bind(policy.valid_until as f64).bind(&payload).execute(&mut *tx).await?;
        let receipt = json!({"workspace_id":self.workspace,"adoption_id":id,"project_id":policy.project_id,"activated_at":activated,"connection_revision":revision,"policy_revision":1});
        sqlx::query("INSERT INTO managed_carrier_adoptions(workspace_id,adoption_id,project_id,plan,policy,receipt,activated_at) VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7))")
            .bind(self.workspace).bind(id).bind(policy.project_id).bind(plan).bind(payload).bind(&receipt).bind(activated as f64).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}
