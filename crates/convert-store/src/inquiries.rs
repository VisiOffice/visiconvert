use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize)]
pub struct Inquiry {
    #[serde(flatten)]
    pub lead: Lead,
    pub primary_lead_id: Uuid,
    pub interaction_count: i64,
    pub search_text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombineInput {
    pub target_id: Uuid,
    pub version: i32,
    pub target_version: i32,
    pub reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitInput {
    pub interaction_id: Uuid,
    pub version: i32,
    pub reason: String,
}
const INQUIRY_SELECT: &str = "SELECT i.id,p.name,p.contact,p.channel,p.source,p.campaign,i.qualification,i.notes,i.version,i.created_at,p.is_demo,p.call_status,p.duration_seconds,p.tracking_number,p.destination,i.primary_lead_id, (SELECT count(*) FROM leads m WHERE m.workspace_id=i.workspace_id AND m.inquiry_id=i.id) AS interaction_count, (SELECT string_agg(concat_ws(' ',m.name,m.contact,m.source,m.campaign),' ') FROM leads m WHERE m.workspace_id=i.workspace_id AND m.inquiry_id=i.id) AS search_text, COALESCE((SELECT SUM(o.amount_minor)::bigint FROM outcomes o JOIN leads m ON m.workspace_id=o.workspace_id AND m.id=o.lead_id WHERE m.workspace_id=i.workspace_id AND m.inquiry_id=i.id AND o.status='booked'),0)::bigint AS booked_minor FROM inquiries i JOIN leads p ON p.workspace_id=i.workspace_id AND p.id=i.primary_lead_id WHERE i.workspace_id=$1 AND i.archived_at IS NULL AND ($2::uuid IS NULL OR i.id=$2) ORDER BY i.created_at DESC,i.id LIMIT 500";
fn inquiry(r: &sqlx::postgres::PgRow) -> Inquiry {
    Inquiry {
        lead: lead_from_row(r),
        primary_lead_id: r.get("primary_lead_id"),
        interaction_count: r.get("interaction_count"),
        search_text: r.get("search_text"),
    }
}
pub(crate) fn lead_from_row(r: &sqlx::postgres::PgRow) -> Lead {
    Lead {
        id: r.get("id"),
        inquiry_id: Some(r.get("id")),
        name: r.get("name"),
        contact: r.get("contact"),
        channel: r.get("channel"),
        source: r.get("source"),
        campaign: r.get("campaign"),
        qualification: r.get("qualification"),
        notes: r.get("notes"),
        version: r.get("version"),
        created_at: r.get("created_at"),
        is_demo: r.get("is_demo"),
        call_status: r.get("call_status"),
        duration_seconds: r.get("duration_seconds"),
        tracking_number: r.get("tracking_number"),
        destination: r.get("destination"),
        booked_minor: r.get("booked_minor"),
    }
}
impl Store {
    pub async fn inquiries(&self) -> Result<Vec<Inquiry>> {
        let rows = sqlx::query(INQUIRY_SELECT)
            .bind(self.workspace)
            .bind(None::<Uuid>)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(inquiry).collect())
    }
    pub async fn inquiry(&self, id: Uuid) -> Result<Inquiry> {
        let r = sqlx::query(INQUIRY_SELECT)
            .bind(self.workspace)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(inquiry(&r))
    }
    pub async fn inquiry_detail(&self, id: Uuid) -> Result<Value> {
        // One repeatable-read snapshot: membership and history must agree with the
        // version displayed to an operator while another operator combines/splits.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let r = sqlx::query(INQUIRY_SELECT)
            .bind(self.workspace)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let members: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(l) - 'submission_payload' - 'parent_sequence' - 'dial_terminal' - 'parent_status' || jsonb_build_object('booked_minor', COALESCE((SELECT SUM(o.amount_minor)::bigint FROM outcomes o WHERE o.workspace_id=l.workspace_id AND o.lead_id=l.id AND o.status='booked'),0)) FROM leads l WHERE l.workspace_id=$1 AND l.inquiry_id=$2 ORDER BY l.created_at,l.id")
            .bind(self.workspace).bind(id).fetch_all(&mut *tx).await?;
        let history: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'kind',kind,'actor',actor,'payload',payload,'created_at',created_at) FROM inquiry_events WHERE workspace_id=$1 AND inquiry_id=$2 ORDER BY id DESC LIMIT 100")
            .bind(self.workspace).bind(id).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"inquiry":inquiry(&r),"interactions":members,"history":history}))
    }
    pub async fn inquiry_history(&self, id: Uuid) -> Result<Vec<Value>> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inquiries WHERE workspace_id=$1 AND id=$2)",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(Error::NotFound);
        }
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'kind',kind,'actor',actor,'payload',payload,'created_at',created_at) FROM inquiry_events WHERE workspace_id=$1 AND inquiry_id=$2 ORDER BY id DESC LIMIT 100").bind(self.workspace).bind(id).fetch_all(&self.pool).await?)
    }
    pub async fn inquiry_actor(&self, session: Option<&str>) -> Result<String> {
        match session {
            None => Ok("workspace operator".into()),
            Some(hash) => {
                let user: Uuid = sqlx::query_scalar("SELECT user_id FROM hosted_sessions WHERE token_hash=$1 AND workspace_id=$2 AND expires_at>now()")
                    .bind(hash).bind(self.workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)?;
                Ok(format!("hosted user {user}"))
            }
        }
    }
    pub(crate) async fn lock_inquiry_writes(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<()> {
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut **tx)
            .await?;
        Ok(())
    }
    pub(crate) async fn group_for_lead(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        lead: Uuid,
    ) -> Result<Uuid> {
        sqlx::query_scalar("SELECT inquiry_id FROM leads WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace)
            .bind(lead)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn update_inquiry(&self, id: Uuid, input: &LeadUpdate, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        self.update_inquiry_tx(&mut tx, id, input, actor).await?;
        sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) SELECT workspace_id,primary_lead_id,'lead_updated',$3 FROM inquiries WHERE workspace_id=$1 AND id=$2").bind(self.workspace).bind(id).bind(json!(input)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn update_inquiry_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        input: &LeadUpdate,
        actor: &str,
    ) -> Result<()> {
        if input.notes.chars().count() > 4000 || input.version < 1 {
            return Err(ValidationError("Invalid notes or version").into());
        }
        let n = sqlx::query("UPDATE inquiries SET qualification=$3,notes=$4,version=version+1 WHERE workspace_id=$1 AND id=$2 AND version=$5 AND archived_at IS NULL")
            .bind(self.workspace).bind(id).bind(input.qualification.as_str()).bind(&input.notes).bind(input.version).execute(&mut **tx).await?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        event(
            tx,
            self.workspace,
            id,
            "review_updated",
            actor,
            json!(input),
        )
        .await
    }
    pub async fn combine_inquiries(
        &self,
        source: Uuid,
        input: &CombineInput,
        actor: &str,
    ) -> Result<Uuid> {
        reason(&input.reason)?;
        if source == input.target_id {
            return Err(ValidationError("Choose a different inquiry").into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let from = active(&mut tx, self.workspace, source, input.version).await?;
        let to = active(
            &mut tx,
            self.workspace,
            input.target_id,
            input.target_version,
        )
        .await?;
        let members: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM leads WHERE workspace_id=$1 AND inquiry_id=$2 ORDER BY id",
        )
        .bind(self.workspace)
        .bind(source)
        .fetch_all(&mut *tx)
        .await?;
        let counts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM leads WHERE workspace_id=$1 AND inquiry_id IN ($2,$3)",
        )
        .bind(self.workspace)
        .bind(source)
        .bind(input.target_id)
        .fetch_one(&mut *tx)
        .await?;
        if counts > 100 {
            return Err(ValidationError("An inquiry supports up to 100 interactions").into());
        }
        let demo: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM leads a JOIN leads b ON b.workspace_id=a.workspace_id WHERE a.workspace_id=$1 AND a.inquiry_id=$2 AND b.inquiry_id=$3 AND a.is_demo<>b.is_demo)").bind(self.workspace).bind(source).bind(input.target_id).fetch_one(&mut *tx).await?;
        if demo {
            return Err(ValidationError("Demo and live interactions cannot be combined").into());
        }
        sqlx::query("UPDATE leads SET inquiry_id=$3 WHERE workspace_id=$1 AND inquiry_id=$2")
            .bind(self.workspace)
            .bind(source)
            .bind(input.target_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE inquiries SET archived_at=now(),version=version+1 WHERE workspace_id=$1 AND id=$2").bind(self.workspace).bind(source).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE inquiries SET version=GREATEST(version,$3)+1 WHERE workspace_id=$1 AND id=$2",
        )
        .bind(self.workspace)
        .bind(input.target_id)
        .bind(from.get::<i32, _>("version"))
        .execute(&mut *tx)
        .await?;
        let payload = json!({"reason":input.reason.trim(),"source_inquiry_id":source,"target_inquiry_id":input.target_id,"interaction_ids":members,"source_snapshot":snapshot(&from),"target_snapshot":snapshot(&to),"attribution":"destination inquiry source retained"});
        event(
            &mut tx,
            self.workspace,
            source,
            "combined_into",
            actor,
            payload.clone(),
        )
        .await?;
        event(
            &mut tx,
            self.workspace,
            input.target_id,
            "combined_from",
            actor,
            payload,
        )
        .await?;
        tx.commit().await?;
        Ok(input.target_id)
    }
    pub async fn split_inquiry(&self, id: Uuid, input: &SplitInput, actor: &str) -> Result<Uuid> {
        reason(&input.reason)?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let old = active(&mut tx, self.workspace, id, input.version).await?;
        let members: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM leads WHERE workspace_id=$1 AND inquiry_id=$2 ORDER BY created_at,id",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        if !members.contains(&input.interaction_id) {
            return Err(Error::NotFound);
        }
        if members.len() < 2 {
            return Err(ValidationError("This interaction already has its own inquiry").into());
        }
        let new_id = Uuid::new_v4();
        sqlx::query("INSERT INTO inquiries(id,workspace_id,primary_lead_id,qualification,version) VALUES($1,$2,$3,'unreviewed',$4)").bind(new_id).bind(self.workspace).bind(input.interaction_id).bind(old.get::<i32,_>("version")+1).execute(&mut *tx).await?;
        sqlx::query("UPDATE leads SET inquiry_id=$3 WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace)
            .bind(input.interaction_id)
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        let primary: Uuid = old.get("primary_lead_id");
        let remaining_primary = if primary == input.interaction_id {
            *members.iter().find(|&&x| x != primary).unwrap()
        } else {
            primary
        };
        sqlx::query("UPDATE inquiries SET primary_lead_id=$3,version=version+1 WHERE workspace_id=$1 AND id=$2").bind(self.workspace).bind(id).bind(remaining_primary).execute(&mut *tx).await?;
        let payload = json!({"reason":input.reason.trim(),"interaction_id":input.interaction_id,"source_inquiry_id":id,"new_inquiry_id":new_id,"source_snapshot":snapshot(&old),"remaining_primary_lead_id":remaining_primary,"new_qualification":"unreviewed","bookings":"remain attached to their original interaction"});
        event(
            &mut tx,
            self.workspace,
            id,
            "interaction_split",
            actor,
            payload.clone(),
        )
        .await?;
        event(
            &mut tx,
            self.workspace,
            new_id,
            "split_from",
            actor,
            payload,
        )
        .await?;
        tx.commit().await?;
        Ok(new_id)
    }
}
fn reason(value: &str) -> Result<()> {
    bounded(value, 500)?;
    Ok(())
}
fn snapshot(row: &sqlx::postgres::PgRow) -> Value {
    json!({"primary_lead_id":row.get::<Uuid,_>("primary_lead_id"),"qualification":row.get::<String,_>("qualification"),"notes":row.get::<String,_>("notes"),"version":row.get::<i32,_>("version")})
}
async fn active(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: Uuid,
    id: Uuid,
    version: i32,
) -> Result<sqlx::postgres::PgRow> {
    let row = sqlx::query(
        "SELECT * FROM inquiries WHERE workspace_id=$1 AND id=$2 AND archived_at IS NULL",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)?;
    if row.get::<i32, _>("version") != version {
        return Err(Error::Conflict);
    }
    Ok(row)
}
pub(crate) async fn event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: Uuid,
    id: Uuid,
    kind: &str,
    actor: &str,
    payload: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO inquiry_events(workspace_id,inquiry_id,kind,actor,payload) VALUES($1,$2,$3,$4,$5)").bind(workspace).bind(id).bind(kind).bind(actor).bind(payload).execute(&mut **tx).await?;
    Ok(())
}
