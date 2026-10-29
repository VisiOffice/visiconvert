pub mod api_keys;
pub mod connections;
pub mod conversions;
pub mod dni;
pub mod google;
pub mod hosted;
pub mod inquiries;
pub mod managed;
pub mod public_api;
pub mod setup;
pub mod signalwire;
pub mod sites;
use convert_domain::*;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("Collection limit reached; retry in a minute")]
    RateLimited,
    #[error("Not found")]
    NotFound,
    #[error("This record changed, or its identifier is already in use. Refresh and retry.")]
    Conflict,
    #[error("This workspace has reached its tracking number limit")]
    Limit,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub workspace: Uuid,
}

impl Store {
    pub async fn connect(url: &str, workspace: Uuid) -> Result<Self> {
        Ok(Self {
            pool: PgPoolOptions::new().max_connections(8).connect(url).await?,
            workspace,
        })
    }
    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::migrate!().run(&self.pool).await?;
        sqlx::query(
            "INSERT INTO workspaces(id,name) VALUES($1,'My workspace') ON CONFLICT DO NOTHING",
        )
        .bind(self.workspace)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn numbers(&self) -> Result<Vec<TrackingNumber>> {
        let rows =
            sqlx::query("SELECT * FROM tracking_numbers WHERE workspace_id=$1 AND released_at IS NULL ORDER BY label")
                .bind(self.workspace)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|r| TrackingNumber {
                provider: if r.get::<String, _>("provider") == "signalwire" {
                    PhoneProvider::Signalwire
                } else {
                    PhoneProvider::Twilio
                },
                id: r.get("id"),
                label: r.get("label"),
                phone_number: r.get("phone_number"),
                destination: r.get("destination"),
                source: r.get("source"),
                campaign: r.get("campaign"),
                is_demo: r.get("is_demo"),
                purchased: r.get::<Option<String>, _>("carrier_sid").is_some(),
                release_requested_at: r.get("release_requested_at"),
            })
            .collect())
    }
    pub async fn add_number(&self, n: &NewNumber) -> Result<()> {
        self.add_number_with_connection(n, None).await
    }
    pub async fn add_number_with_connection(
        &self,
        n: &NewNumber,
        revision: Option<i64>,
    ) -> Result<()> {
        n.validate()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(self.workspace)
            .fetch_one(&mut *tx)
            .await?;
        self.insert_number(&mut tx, n, revision, None).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Registers a number inside a transaction that already holds the workspace lock.
    /// `carrier_sid` marks a number VisiConvert bought and may later release.
    pub(crate) async fn insert_number(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        n: &NewNumber,
        revision: Option<i64>,
        carrier_sid: Option<&str>,
    ) -> Result<Uuid> {
        n.validate()?;
        let current: Option<i64> = sqlx::query_scalar(match n.provider {
            PhoneProvider::Twilio => {
                "SELECT revision FROM twilio_connections WHERE workspace_id=$1"
            }
            PhoneProvider::Signalwire => {
                "SELECT revision FROM signalwire_connections WHERE workspace_id=$1"
            }
        })
        .bind(self.workspace)
        .fetch_optional(&mut **tx)
        .await?;
        if current != revision || (n.provider == PhoneProvider::Signalwire && revision.is_none()) {
            return Err(Error::Conflict);
        }
        // Serialize registrations across workspaces: a destination must never be a tracked number.
        sqlx::query("SELECT pg_advisory_xact_lock(73581001)")
            .execute(&mut **tx)
            .await?;
        if carrier_sid.is_none() {
            let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM number_purchases WHERE workspace_id=$1 AND phone_number=$2 AND status IN ('pending','cleanup','review'))")
                .bind(self.workspace).bind(&n.phone_number).fetch_one(&mut **tx).await?;
            if pending {
                return Err(Error::Conflict);
            }
        }
        let conflict: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tracking_numbers WHERE released_at IS NULL AND (phone_number=$1 OR phone_number=$2 OR destination=$1)) OR EXISTS(SELECT 1 FROM dni_pools WHERE destination_phone=$1 OR (fallback_phone=$1 AND (workspace_id<>$3 OR destination_phone<>$2)))").bind(&n.phone_number).bind(&n.destination).bind(self.workspace).fetch_one(&mut **tx).await?;
        if conflict {
            return Err(Error::Conflict);
        }
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO tracking_numbers(id,workspace_id,label,phone_number,destination,source,campaign,provider,carrier_sid,purchased_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,CASE WHEN $9::text IS NULL THEN NULL ELSE now() END)")
            .bind(id).bind(self.workspace).bind(n.label.trim()).bind(&n.phone_number).bind(&n.destination).bind(n.source.trim()).bind(n.campaign.trim()).bind(n.provider.as_str()).bind(carrier_sid).execute(&mut **tx).await?;
        Ok(id)
    }
    pub async fn leads(&self) -> Result<Vec<Lead>> {
        let rows = sqlx::query("SELECT l.*, i.qualification AS inquiry_qualification,i.notes AS inquiry_notes,i.version AS inquiry_version, COALESCE((SELECT SUM(amount_minor)::bigint FROM outcomes o WHERE o.workspace_id=l.workspace_id AND o.lead_id=l.id AND o.status='booked'),0)::bigint AS booked_minor FROM leads l JOIN inquiries i ON i.workspace_id=l.workspace_id AND i.id=l.inquiry_id WHERE l.workspace_id=$1 ORDER BY l.created_at DESC LIMIT 500")
            .bind(self.workspace).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| Lead {
                id: r.get("id"),
                inquiry_id: Some(r.get("inquiry_id")),
                name: r.get("name"),
                contact: r.get("contact"),
                channel: r.get("channel"),
                source: r.get("source"),
                campaign: r.get("campaign"),
                qualification: r.get("inquiry_qualification"),
                notes: r.get("inquiry_notes"),
                version: r.get("inquiry_version"),
                created_at: r.get("created_at"),
                is_demo: r.get("is_demo"),
                call_status: r.get("call_status"),
                duration_seconds: r.get("duration_seconds"),
                tracking_number: r.get("tracking_number"),
                destination: r.get("destination"),
                booked_minor: r.get("booked_minor"),
            })
            .collect())
    }
    pub async fn add_lead(&self, n: &NewLead) -> Result<Uuid> {
        n.validate()?;
        let id = Uuid::new_v4();
        let row = sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,notes,submission_payload) VALUES($1,$2,$3,$4,$5,'form',$6,$7,$8,$9) ON CONFLICT(workspace_id,channel,external_id) DO NOTHING RETURNING id")
            .bind(id).bind(self.workspace).bind(&n.external_id).bind(&n.name).bind(&n.contact).bind(&n.source).bind(&n.campaign).bind(&n.notes).bind(serde_json::json!(n)).fetch_optional(&self.pool).await?;
        if row.is_some() {
            return Ok(id);
        }
        // Repeated identifiers are accepted only for an identical submitted payload.
        let row = sqlx::query("SELECT id FROM leads WHERE workspace_id=$1 AND channel='form' AND external_id=$2 AND submission_payload=$3")
            .bind(self.workspace).bind(&n.external_id).bind(serde_json::json!(n)).fetch_optional(&self.pool).await?;
        row.map(|r| r.get("id")).ok_or(Error::Conflict)
    }
    pub async fn update_lead(&self, id: Uuid, n: &LeadUpdate) -> Result<()> {
        self.update_public_lead(id, n, "authenticated legacy API")
            .await
    }
    pub async fn update_public_lead(&self, id: Uuid, n: &LeadUpdate, actor: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let group = self.group_for_lead(&mut tx, id).await?;
        self.update_inquiry_tx(&mut tx, group, n, actor).await?;
        sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) VALUES($1,$2,'lead_updated',$3)").bind(self.workspace).bind(id).bind(serde_json::json!(n)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn outcomes(&self, id: Uuid) -> Result<Vec<Outcome>> {
        let mut tx = self.pool.begin().await?;
        self.group_for_lead(&mut tx, id).await?;
        let rows = sqlx::query(
            "SELECT * FROM outcomes WHERE workspace_id=$1 AND lead_id=$2 ORDER BY external_id",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .iter()
            .map(|r| Outcome {
                external_id: r.get("external_id"),
                amount_minor: r.get("amount_minor"),
                currency: r.get("currency"),
                status: r.get("status"),
                revision: r.get("revision"),
            })
            .collect())
    }
    pub async fn inquiry_outcomes(&self, id: Uuid) -> Result<serde_json::Value> {
        sqlx::query_scalar("SELECT COALESCE((SELECT jsonb_agg(to_jsonb(o) - 'workspace_id' ORDER BY o.external_id) FROM outcomes o JOIN leads l ON l.workspace_id=o.workspace_id AND l.id=o.lead_id WHERE l.workspace_id=i.workspace_id AND l.inquiry_id=i.id),'[]'::jsonb) FROM inquiries i WHERE i.workspace_id=$1 AND i.id=$2 AND i.archived_at IS NULL")
            .bind(self.workspace).bind(id).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn put_public_outcome(&self, id: Uuid, n: &OutcomeInput, actor: &str) -> Result<()> {
        self.put_group_outcome(id, n, false, actor).await
    }
    pub async fn put_outcome(&self, id: Uuid, n: &OutcomeInput) -> Result<()> {
        self.put_group_outcome(id, n, false, "authenticated legacy API")
            .await
    }
    pub async fn put_inquiry_outcome(&self, id: Uuid, n: &OutcomeInput, actor: &str) -> Result<()> {
        self.put_group_outcome(id, n, true, actor).await
    }
    async fn put_group_outcome(
        &self,
        requested: Uuid,
        n: &OutcomeInput,
        is_inquiry: bool,
        actor: &str,
    ) -> Result<()> {
        n.validate()?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let group = if is_inquiry {
            requested
        } else {
            self.group_for_lead(&mut tx, requested).await?
        };
        let primary: Uuid = sqlx::query_scalar("SELECT primary_lead_id FROM inquiries WHERE workspace_id=$1 AND id=$2 AND archived_at IS NULL").bind(self.workspace).bind(group).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        let mut id = if is_inquiry { primary } else { requested };
        lock(
            &mut tx,
            &format!("{}:outcome:{}", self.workspace, n.external_id),
        )
        .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM leads WHERE workspace_id=$1 AND id=$2)",
        )
        .bind(self.workspace)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(Error::NotFound);
        }
        if let Some(row) =
            sqlx::query("SELECT * FROM outcomes WHERE workspace_id=$1 AND external_id=$2")
                .bind(self.workspace)
                .bind(&n.external_id)
                .fetch_optional(&mut *tx)
                .await?
        {
            let original: Uuid = row.get("lead_id");
            if self.group_for_lead(&mut tx, original).await? != group {
                return Err(Error::Conflict);
            }
            id = original;
            let same = row.get::<i64, _>("amount_minor") == n.amount_minor
                && row.get::<String, _>("status") == n.status
                && row.get::<i32, _>("revision") == n.revision;
            if same {
                return Ok(());
            }
            if n.revision != row.get::<i32, _>("revision") + 1 {
                return Err(Error::Conflict);
            }
        } else if n.revision != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO outcomes(workspace_id,lead_id,external_id,amount_minor,currency,status,revision) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(workspace_id,external_id) DO UPDATE SET amount_minor=EXCLUDED.amount_minor,status=EXCLUDED.status,revision=EXCLUDED.revision")
            .bind(self.workspace).bind(id).bind(&n.external_id).bind(n.amount_minor).bind(&n.currency).bind(&n.status).bind(n.revision).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO audit_events(workspace_id,lead_id,kind,payload) VALUES($1,$2,'outcome_updated',$3)").bind(self.workspace).bind(id).bind(serde_json::json!(n)).execute(&mut *tx).await?;
        sqlx::query("UPDATE inquiries SET version=version+1 WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace)
            .bind(group)
            .execute(&mut *tx)
            .await?;
        inquiries::event(
            &mut tx,
            self.workspace,
            group,
            "booking_updated",
            actor,
            serde_json::json!({"interaction_id":id,"booking":n}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn summary(&self) -> Result<Vec<SourceSummary>> {
        let rows=sqlx::query("SELECT p.source,COUNT(*) AS leads,COUNT(*) FILTER(WHERE i.qualification='qualified') AS qualified,COALESCE(SUM(o.bookings),0)::bigint AS bookings,COALESCE(SUM(o.amount),0)::bigint AS booked_minor FROM inquiries i JOIN leads p ON p.workspace_id=i.workspace_id AND p.id=i.primary_lead_id LEFT JOIN LATERAL (SELECT COUNT(*) AS bookings,SUM(o.amount_minor) AS amount FROM outcomes o JOIN leads m ON m.workspace_id=o.workspace_id AND m.id=o.lead_id WHERE m.workspace_id=i.workspace_id AND m.inquiry_id=i.id AND o.status='booked') o ON true WHERE i.workspace_id=$1 AND i.archived_at IS NULL GROUP BY p.source ORDER BY leads DESC,p.source").bind(self.workspace).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| SourceSummary {
                source: r.get("source"),
                leads: r.get("leads"),
                qualified: r.get("qualified"),
                bookings: r.get("bookings"),
                booked_minor: r.get("booked_minor"),
            })
            .collect())
    }

    pub async fn call_event(&self, event: &CallEvent) -> Result<String> {
        self.call_event_for(event, PhoneProvider::Twilio).await
    }
    pub async fn call_event_for(
        &self,
        event: &CallEvent,
        provider: PhoneProvider,
    ) -> Result<String> {
        let sid = match provider {
            PhoneProvider::Twilio => event.sid.clone(),
            PhoneProvider::Signalwire => format!("signalwire:{}", event.sid),
        };
        let mut tx = self.pool.begin().await?;
        lock(&mut tx, &format!("{}:call:{}", self.workspace, sid)).await?;
        let existing=sqlx::query("SELECT id,destination,tracking_number,contact,provider FROM leads WHERE workspace_id=$1 AND channel='call' AND external_id=$2").bind(self.workspace).bind(&sid).fetch_optional(&mut *tx).await?;
        let (id, destination) = if let Some(r) = existing {
            if r.get::<String, _>("tracking_number") != event.to
                || r.get::<String, _>("contact") != event.from
                || r.get::<Option<String>, _>("provider").as_deref() != Some(provider.as_str())
            {
                return Err(Error::Conflict);
            }
            (r.get::<Uuid, _>("id"), r.get::<String, _>("destination"))
        } else {
            let number=sqlx::query("SELECT * FROM tracking_numbers WHERE workspace_id=$1 AND phone_number=$2 AND provider=$3 AND NOT is_demo AND released_at IS NULL").bind(self.workspace).bind(&event.to).bind(provider.as_str()).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
            let id = Uuid::new_v4();
            let destination: String = number.get("destination");
            sqlx::query("INSERT INTO leads(id,workspace_id,external_id,name,contact,channel,source,campaign,tracking_number,destination,call_status,provider) VALUES($1,$2,$3,'Phone inquiry',$4,'call',$5,$6,$7,$8,'ringing',$9)")
                .bind(id).bind(self.workspace).bind(&sid).bind(&event.from).bind(number.get::<String,_>("source")).bind(number.get::<String,_>("campaign")).bind(&event.to).bind(&destination).bind(provider.as_str()).execute(&mut *tx).await?;
            self.attribute_dni_call(
                &mut tx,
                id,
                number.get("id"),
                matches!(event.kind, CallEventKind::Voice),
            )
            .await?;
            (id, destination)
        };
        let receipt=sqlx::query("INSERT INTO webhook_receipts(workspace_id,fingerprint) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(self.workspace).bind(&event.fingerprint).execute(&mut *tx).await?;
        if receipt.rows_affected() > 0 {
            match &event.kind {
                CallEventKind::Voice => {}
                CallEventKind::Dial { status, duration } => {
                    // A destination answer can be an IVR or voicemail, never proof of a human answer.
                    let result = if status == "completed" {
                        "destination_answered"
                    } else {
                        status.as_str()
                    };
                    sqlx::query("UPDATE leads SET call_status=$3,duration_seconds=$4,dial_terminal=true WHERE workspace_id=$1 AND id=$2 AND NOT dial_terminal").bind(self.workspace).bind(id).bind(result).bind(duration).execute(&mut *tx).await?;
                }
                CallEventKind::Status { status, sequence } => {
                    let mapped = match status.as_str() {
                        "completed" => "ended_unknown",
                        "in-progress" => "in_progress",
                        other => other,
                    };
                    sqlx::query("UPDATE leads SET parent_status=$3,parent_sequence=$4,call_status=CASE WHEN dial_terminal THEN call_status ELSE $5 END WHERE workspace_id=$1 AND id=$2 AND parent_sequence<$4 AND COALESCE(parent_status,'') NOT IN ('completed','busy','failed','no-answer','canceled')")
                        .bind(self.workspace).bind(id).bind(status).bind(sequence).bind(mapped).execute(&mut *tx).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(destination)
    }
    pub async fn seed_demo(&self) -> Result<()> {
        for (index, (name, source, campaign)) in [
            ("Avery Morgan", "Google Ads", "Emergency plumbing"),
            ("Jordan Lee", "Organic search", "Website inquiry"),
            ("Casey Rivera", "Google Ads", "Drain cleaning"),
            ("Sam Taylor", "Direct / supplied", "Contact form"),
        ]
        .iter()
        .enumerate()
        {
            let id = self
                .add_lead(&NewLead {
                    external_id: format!("demo-{index}"),
                    name: name.to_string(),
                    contact: format!("demo{}@example.com", index + 1),
                    source: source.to_string(),
                    campaign: campaign.to_string(),
                    notes: "Sample inquiry for exploring the pilot.".into(),
                })
                .await?;
            sqlx::query("UPDATE leads SET is_demo=true WHERE workspace_id=$1 AND id=$2")
                .bind(self.workspace)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}

async fn lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    key: &str,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub struct CallEvent {
    pub sid: String,
    pub to: String,
    pub from: String,
    pub fingerprint: String,
    pub kind: CallEventKind,
}
pub enum CallEventKind {
    Voice,
    Dial {
        status: String,
        duration: Option<i32>,
    },
    Status {
        status: String,
        sequence: i32,
    },
}
