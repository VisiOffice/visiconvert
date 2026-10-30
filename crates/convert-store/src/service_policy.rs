use crate::{Error, Result, Store};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePolicy {
    pub project_id: Uuid,
    pub revision: i64,
    pub state: String,
    pub reason: String,
    pub valid_until: i64,
    pub release_at: Option<i64>,
}
impl Store {
    pub async fn apply_service_policy(&self, policy: &ServicePolicy) -> Result<()> {
        let payload = serde_json::to_value(policy).map_err(|_| Error::Conflict)?;
        let mut tx = self.pool.begin().await?;
        // Same lock as connection changes and the irreversible release claim.
        self.lock_inquiry_writes(&mut tx).await?;
        let connected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM signalwire_connections WHERE workspace_id=$1 AND project_id=$2 AND managed)")
            .bind(self.workspace).bind(policy.project_id).fetch_one(&mut *tx).await?;
        if !connected {
            return Err(Error::Conflict);
        }
        if let Some(row) = sqlx::query(
            "SELECT revision,payload FROM managed_service_policies WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .fetch_optional(&mut *tx)
        .await?
        {
            let revision: i64 = row.get("revision");
            if revision >= policy.revision {
                if revision == policy.revision
                    && row.get::<serde_json::Value, _>("payload") == payload
                {
                    return Ok(());
                }
                return Err(Error::Conflict);
            }
        }
        let now = Utc::now().timestamp();
        if policy.revision <= 0
            || policy.valid_until <= now
            || policy.valid_until > now + 86460
            || !["active", "grace", "suspended"].contains(&policy.state.as_str())
            || !["paid", "payment_required", "canceled", "review"].contains(&policy.reason.as_str())
            || policy.release_at.is_some_and(|at| {
                at <= 0
                    || policy.state != "suspended"
                    || !["canceled", "payment_required"].contains(&policy.reason.as_str())
            })
        {
            return Err(convert_domain::ValidationError("Invalid managed service policy").into());
        }
        sqlx::query("INSERT INTO managed_service_policies(workspace_id,project_id,revision,state,reason,valid_until,release_at,payload) VALUES($1,$2,$3,$4,$5,to_timestamp($6),to_timestamp($7),$8) ON CONFLICT(workspace_id) DO UPDATE SET project_id=$2,revision=$3,state=$4,reason=$5,valid_until=to_timestamp($6),release_at=to_timestamp($7),payload=$8,received_at=now()")
            .bind(self.workspace).bind(policy.project_id).bind(policy.revision).bind(&policy.state).bind(&policy.reason)
            .bind(policy.valid_until as f64).bind(policy.release_at.map(|v|v as f64)).bind(payload).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn managed_voice_allowed(&self, phone: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT managed_forwarding_allowed($1) AND NOT EXISTS(SELECT 1 FROM tracking_numbers WHERE workspace_id=$1 AND phone_number=$2 AND provider='signalwire' AND release_started_at IS NOT NULL AND released_at IS NULL)")
            .bind(self.workspace).bind(phone).fetch_one(&self.pool).await?)
    }
    pub async fn managed_forwarding_allowed(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT managed_forwarding_allowed($1)")
            .bind(self.workspace)
            .fetch_one(&self.pool)
            .await?)
    }
}
