//! Readiness is evidence, not a manually checked completion flag.
use super::*;
use serde_json::{Value, json};
impl Store {
    pub async fn setup_status(&self) -> Result<Value> {
        let sites: Vec<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(s)-'workspace_id'-'created_at' || jsonb_build_object(
              'pool_enabled',COALESCE(p.enabled,false), 'pool_configured',p.site_id IS NOT NULL,
              'pool_number_count',(SELECT count(*) FROM dni_pool_numbers n WHERE n.workspace_id=s.workspace_id AND n.site_id=s.id AND n.enabled),
              'assignment_count',(SELECT count(*) FROM dni_assignments a WHERE a.workspace_id=s.workspace_id AND a.site_id=s.id),
              'last_assignment_at',(SELECT max(a.assigned_at) FROM dni_assignments a WHERE a.workspace_id=s.workspace_id AND a.site_id=s.id))
             FROM sites s LEFT JOIN dni_pools p ON p.workspace_id=s.workspace_id AND p.site_id=s.id
             WHERE s.workspace_id=$1 ORDER BY s.created_at,s.id")
            .bind(self.workspace).fetch_all(&self.pool).await?;
        let numbers: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('id',n.id,'phone_number',n.phone_number,'provider',n.provider,
             'call_count',count(l.id),'last_call_at',max(l.created_at))
             FROM tracking_numbers n LEFT JOIN leads l ON l.workspace_id=n.workspace_id
             AND l.tracking_number=n.phone_number AND l.provider=n.provider AND l.channel='call' AND NOT l.is_demo
             WHERE n.workspace_id=$1 AND NOT n.is_demo AND n.released_at IS NULL GROUP BY n.id ORDER BY n.label,n.id")
            .bind(self.workspace).fetch_all(&self.pool).await?;
        Ok(
            json!({"sites":sites,"numbers":numbers,"google_uploads_enabled":self.delivery_settings().await?["enabled"],
            "evidence_note":"Activity is historical, not a live health guarantee. Website visits and form success are reported by your website. A received call does not prove the business line rang or the attribution is correct; verify a real call and its source in the inbox."}),
        )
    }
}
