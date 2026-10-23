use crate::{Result, Store};
use chrono::{DateTime, Utc};
use convert_domain::ValidationError;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub qualification: Option<String>,
}
impl Page {
    fn limit(&self) -> Result<i64> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=100).contains(&limit)
            || self.source.as_ref().is_some_and(|s| s.len() > 400)
            || self.qualification.as_ref().is_some_and(|s| {
                !["unreviewed", "qualified", "unqualified", "spam"].contains(&s.as_str())
            })
        {
            return Err(ValidationError(
                "Use limit 1–100, a bounded source and a supported qualification",
            )
            .into());
        }
        Ok(limit)
    }
}
impl Store {
    pub async fn public_leads(
        &self,
        page: &Page,
        grouped: bool,
        id: Option<Uuid>,
    ) -> Result<Value> {
        let limit = page.limit()?;
        let (identity, date, version, notes, qual) = if grouped {
            (
                "i.id",
                "i.created_at",
                "i.version",
                "i.notes",
                "i.qualification",
            )
        } else {
            (
                "l.id",
                "l.created_at",
                "i.version",
                "i.notes",
                "i.qualification",
            )
        };
        let mut q = QueryBuilder::<Postgres>::new(format!(
            "SELECT {identity} AS id,{date} AS created_at,jsonb_build_object('id',{identity},'inquiry_id',i.id,'primary_lead_id',i.primary_lead_id,'name',l.name,'contact',l.contact,'channel',l.channel,'source',l.source,'campaign',l.campaign,'qualification',{qual},'notes',{notes},'version',{version},'created_at',{date},'is_demo',l.is_demo,'tracking_number',l.tracking_number,'destination',l.destination,'call_status',l.call_status,'duration_seconds',l.duration_seconds,'booked_minor',(SELECT COALESCE(sum(o.amount_minor),0)::bigint FROM outcomes o JOIN leads m ON m.id=o.lead_id AND m.workspace_id=o.workspace_id WHERE o.workspace_id=l.workspace_id AND o.status='booked' AND {})) AS data FROM {} WHERE l.workspace_id=",
            if grouped {
                "m.inquiry_id=i.id"
            } else {
                "o.lead_id=l.id"
            },
            if grouped {
                "inquiries i JOIN leads l ON l.id=i.primary_lead_id AND l.workspace_id=i.workspace_id"
            } else {
                "leads l JOIN inquiries i ON i.id=l.inquiry_id AND i.workspace_id=l.workspace_id"
            }
        ));
        q.push_bind(self.workspace);
        if grouped {
            q.push(" AND i.archived_at IS NULL");
        }
        if let Some(id) = id {
            q.push(format!(" AND {identity}=")).push_bind(id);
        }
        if let Some(since) = page.since {
            q.push(format!(" AND {date}>=")).push_bind(since);
        }
        if let Some(source) = &page.source {
            q.push(" AND lower(l.source)=lower(")
                .push_bind(source)
                .push(")");
        }
        if let Some(qual) = &page.qualification {
            q.push(" AND i.qualification=").push_bind(qual);
        }
        if let Some(cursor) = &page.cursor {
            let (time, id) = cursor
                .split_once('|')
                .ok_or(ValidationError("Invalid page cursor"))?;
            let time = time
                .parse::<DateTime<Utc>>()
                .map_err(|_| ValidationError("Invalid page cursor"))?;
            let id = id
                .parse::<Uuid>()
                .map_err(|_| ValidationError("Invalid page cursor"))?;
            q.push(format!(" AND ({date},{identity})<("))
                .push_bind(time)
                .push(",")
                .push_bind(id)
                .push(")");
        }
        q.push(format!(" ORDER BY {date} DESC,{identity} DESC LIMIT "))
            .push_bind(limit + 1);
        let rows = q.build().fetch_all(&self.pool).await?;
        let next = if rows.len() > limit as usize {
            let r = &rows[limit as usize - 1];
            Some(format!(
                "{}|{}",
                r.get::<DateTime<Utc>, _>("created_at").to_rfc3339(),
                r.get::<Uuid, _>("id")
            ))
        } else {
            None
        };
        let data: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|r| r.get("data"))
            .collect();
        Ok(json!({"data":data,"next_cursor":next}))
    }
    pub async fn public_bookings(&self, page: &Page) -> Result<Value> {
        let limit = page.limit()?;
        if page.since.is_some()
            || page.source.is_some()
            || page.qualification.is_some()
            || page
                .cursor
                .as_ref()
                .is_some_and(|s| s.chars().count() > 150)
        {
            return Err(ValidationError("Bookings accept limit and cursor only").into());
        }
        let rows=sqlx::query("SELECT o.external_id,jsonb_build_object('external_id',o.external_id,'lead_id',o.lead_id,'inquiry_id',l.inquiry_id,'amount_minor',o.amount_minor,'currency',o.currency,'status',o.status,'revision',o.revision) AS data FROM outcomes o JOIN leads l ON l.id=o.lead_id AND l.workspace_id=o.workspace_id WHERE o.workspace_id=$1 AND ($2::text IS NULL OR o.external_id>$2) ORDER BY o.external_id LIMIT $3")
            .bind(self.workspace).bind(&page.cursor).bind(limit+1).fetch_all(&self.pool).await?;
        let next = if rows.len() > limit as usize {
            Some(rows[limit as usize - 1].get::<String, _>("external_id"))
        } else {
            None
        };
        let data: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|r| r.get("data"))
            .collect();
        Ok(json!({"data":data,"next_cursor":next}))
    }
}
