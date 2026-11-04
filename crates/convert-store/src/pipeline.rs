//! Automatic pipeline conversions: workspace actions, business events, and the planner
//! that turns eligible event revisions into queued Google deliveries. No provider I/O;
//! the caller supplies the request builder so this crate stays transport-free.
use crate::{
    Error, Result, Store,
    conversions::{self, Consent, Decision},
};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use convert_domain::ValidationError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

/// Builds the provider request from a frozen snapshot, the login customer and the first
/// request already sent for the same conversion (restatements must keep its identity).
pub type Build<'a> =
    &'a (dyn Fn(&Value, &str, Option<&Value>) -> std::result::Result<Value, &'static str> + Sync);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionInput {
    pub key: String,
    pub name: String,
    pub trigger: String,
    pub default_value_minor: Option<i64>,
    pub counting: String,
    pub google_action_id: Option<String>,
    pub enabled: bool,
    pub version: i32,
}
impl ActionInput {
    fn validate(&self) -> Result<()> {
        let key = self.key.as_bytes();
        if key.is_empty()
            || key.len() > 40
            || !key[0].is_ascii_lowercase()
            || !key
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
            || self.name.trim().is_empty()
            || self.name.chars().count() > 100
            || self.name.chars().any(char::is_control)
            || !["events", "new_lead"].contains(&self.trigger.as_str())
            || !["once_per_click", "every"].contains(&self.counting.as_str())
            || self
                .default_value_minor
                .is_some_and(|v| !(0..=100_000_000_000).contains(&v))
            || self.google_action_id.as_deref().is_some_and(|s| {
                !(1..=20).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_digit())
            })
            || self.version < 0
        {
            return Err(ValidationError("Use a lowercase key (letters, digits, underscores; up to 40), a name, trigger events or new_lead, counting once_per_click or every, a non-negative value in cents and a numeric Google action ID").into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyInput {
    pub auto_send: bool,
    pub consent_policy: String,
    #[serde(default)]
    pub attest: bool,
    pub version: i32,
}
/// Body of `PUT /api/v1/conversion-delivery` (conversions:send).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryInput {
    pub enabled: Option<bool>,
    pub auto_send: Option<bool>,
    pub consent_policy: Option<String>,
    #[serde(default)]
    pub attest_no_eea_advertising: bool,
    pub version: i32,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    pub phone: Option<String>,
    pub email: Option<String>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventInput {
    pub action: String,
    pub external_id: String,
    pub lead_id: Option<Uuid>,
    #[serde(rename = "match")]
    pub matcher: Option<Match>,
    pub attribution: Option<String>,
    pub value_minor: Option<i64>,
    pub occurred_at: Option<DateTime<Utc>>,
    pub status: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub action: Option<String>,
    pub external_id: Option<String>,
}
enum Matcher {
    Lead(Uuid),
    Phone(String),
    Email(String),
}
impl EventInput {
    /// Normalized matcher, attribution and status. Phone numbers compare on their last
    /// 10 digits; email addresses compare trimmed and case-insensitively.
    fn parse(&self) -> Result<(Option<Matcher>, &'static str, &'static str)> {
        let bad = |m| -> Error { ValidationError(m).into() };
        if self.external_id.trim().is_empty()
            || self.external_id.chars().count() > 150
            || self.external_id.chars().any(char::is_control)
            || self.action.len() > 40
        {
            return Err(bad(
                "Use an action key and an external_id of 1–150 characters",
            ));
        }
        if self
            .value_minor
            .is_some_and(|v| !(0..=100_000_000_000).contains(&v))
        {
            return Err(bad(
                "value_minor must be whole cents between 0 and 100000000000",
            ));
        }
        if self
            .occurred_at
            .is_some_and(|t| t > Utc::now() + Duration::minutes(5))
        {
            return Err(bad("occurred_at cannot be in the future"));
        }
        let attribution = match self.attribution.as_deref() {
            None | Some("first_click") => "first_click",
            Some("latest_click") => "latest_click",
            _ => return Err(bad("attribution must be first_click or latest_click")),
        };
        let status = match self.status.as_deref() {
            None | Some("active") => "active",
            Some("canceled") => "canceled",
            _ => return Err(bad("status must be active or canceled")),
        };
        let m = self.matcher.clone().unwrap_or_default();
        let given = usize::from(self.lead_id.is_some())
            + usize::from(m.phone.is_some())
            + usize::from(m.email.is_some());
        if given > 1 {
            return Err(bad("Use one of lead_id, match.phone or match.email"));
        }
        let matcher = if let Some(id) = self.lead_id {
            Some(Matcher::Lead(id))
        } else if let Some(phone) = m.phone {
            let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
            if digits.len() < 10 || phone.chars().count() > 40 {
                return Err(bad("match.phone needs at least 10 digits"));
            }
            Some(Matcher::Phone(digits[digits.len() - 10..].into()))
        } else if let Some(email) = m.email {
            let email = email.trim().to_lowercase();
            if !email.contains('@') || email.chars().count() > 200 {
                return Err(bad("match.email must be an email address"));
            }
            Some(Matcher::Email(email))
        } else {
            None
        };
        if matcher.is_none() && status == "active" {
            return Err(bad("Use lead_id, match.phone or match.email"));
        }
        Ok((matcher, attribution, status))
    }
}
impl Matcher {
    fn digest(&self, attribution: &str) -> String {
        let (kind, value) = match self {
            Self::Lead(id) => ("lead_id", id.to_string()),
            Self::Phone(p) => ("phone", p.clone()),
            Self::Email(e) => ("email", e.clone()),
        };
        conversions::digest(&json!([kind, value, attribution]))
    }
}

/// Lead → captured website visit, using the same joins as manual preparation: the
/// lead's form submission session, or the DNI session behind an inferred_window call.
const CLICK: &str = "LEFT JOIN LATERAL (SELECT s.evidence,s.created_at AS visit_at FROM website_submissions f JOIN website_sessions s ON s.site_id=f.site_id AND s.id=f.session_id WHERE f.workspace_id=l.workspace_id AND f.lead_id=l.id ORDER BY f.created_at LIMIT 1) ws ON true
LEFT JOIN LATERAL (SELECT s.evidence,s.created_at AS visit_at FROM dni_call_evidence dc JOIN dni_assignments da ON da.id=dc.assignment_id AND da.workspace_id=dc.workspace_id JOIN website_sessions s ON s.site_id=da.site_id AND s.id=da.session_id WHERE dc.workspace_id=l.workspace_id AND dc.lead_id=l.id AND dc.status='inferred_window' LIMIT 1) ds ON true";
const HAS_CLICK: &str = "(COALESCE(COALESCE(ws.evidence,ds.evidence)->>'gclid','')<>'' OR COALESCE(COALESCE(ws.evidence,ds.evidence)->>'gbraid','')<>'' OR COALESCE(COALESCE(ws.evidence,ds.evidence)->>'wbraid','')<>'')";
/// Only compile-time SQL fragments are interpolated; every value is bound.
fn sql(s: String) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(s)
}
const EVENT: &str = "SELECT e.*,d.id AS delivery_id,d.state AS delivery_state,d.diagnostic,d.request_id,(d.snapshot->>'revision')::int AS delivery_revision FROM pipeline_events e LEFT JOIN LATERAL (SELECT d.* FROM conversion_deliveries d WHERE d.workspace_id=e.workspace_id AND d.outbox_id=e.outbox_id AND d.snapshot->>'event_id'=e.id::text ORDER BY d.created_at DESC LIMIT 1) d ON true WHERE e.workspace_id=$1";

fn event_json(r: &sqlx::postgres::PgRow) -> Value {
    let status: String = r.get("status");
    let plan: String = r.get("plan_state");
    let revision: i32 = r.get("revision");
    let reason: Option<String> = r.get("blocked_reason");
    let current = r.get::<Option<i32>, _>("delivery_revision") == Some(revision);
    let raw: Option<String> = r.get("delivery_state");
    let (google, reason) = if status == "canceled" {
        ("canceled", reason)
    } else if plan == "pending" {
        ("pending", Some("Waiting for the delivery worker".into()))
    } else if plan != "planned" || !current {
        ("blocked", reason)
    } else {
        let diagnostic: Option<String> = r.get("diagnostic");
        match raw.as_deref() {
            Some("queued" | "retry" | "sending") => ("queued", None),
            Some("submitted" | "polling") => ("sent", None),
            Some("processed") => ("delivered", None),
            Some("uncertain") => ("uncertain", diagnostic),
            Some("canceled") => ("canceled", diagnostic),
            _ => ("failed", diagnostic),
        }
    };
    json!({"id":r.get::<Uuid,_>("id"),"action":r.get::<String,_>("action_key"),"external_id":r.get::<String,_>("external_id"),
        "lead_id":r.get::<Option<Uuid>,_>("lead_id"),"matched_by":r.get::<String,_>("matched_by"),"attribution":r.get::<String,_>("attribution"),
        "value_minor":r.get::<Option<i64>,_>("value_minor"),"currency":r.get::<String,_>("currency"),"occurred_at":r.get::<DateTime<Utc>,_>("occurred_at"),
        "revision":revision,"status":status,"created_at":r.get::<DateTime<Utc>,_>("created_at"),"updated_at":r.get::<DateTime<Utc>,_>("updated_at"),
        "delivery":{"google":google,"reason":reason,"delivery_id":r.get::<Option<Uuid>,_>("delivery_id"),"delivery_state":raw,
            "delivery_revision":r.get::<Option<i32>,_>("delivery_revision"),"request_id":r.get::<Option<String>,_>("request_id")}})
}
async fn record(
    conn: &mut PgConnection,
    workspace: Uuid,
    kind: &str,
    actor: &str,
    payload: Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO conversion_events(workspace_id,kind,actor,payload) VALUES($1,$2,$3,$4)",
    )
    .bind(workspace)
    .bind(kind)
    .bind(actor)
    .bind(payload)
    .execute(conn)
    .await?;
    Ok(())
}
async fn find_lead(
    conn: &mut PgConnection,
    workspace: Uuid,
    matcher: &Matcher,
    attribution: &str,
) -> Result<Option<Uuid>> {
    let (condition, value) = match matcher {
        Matcher::Lead(id) => {
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM leads WHERE workspace_id=$1 AND id=$2)",
            )
            .bind(workspace)
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
            return if found {
                Ok(Some(*id))
            } else {
                Err(Error::NotFound)
            };
        }
        Matcher::Phone(p) => ("right(regexp_replace(l.contact,'[^0-9]','','g'),10)=$2", p),
        Matcher::Email(e) => ("lower(btrim(l.contact))=$2", e),
    };
    let order = if attribution == "latest_click" {
        "DESC"
    } else {
        "ASC"
    };
    Ok(sqlx::query_scalar(sql(format!("SELECT l.id FROM leads l {CLICK} WHERE l.workspace_id=$1 AND NOT l.is_demo AND {HAS_CLICK} AND {condition} ORDER BY COALESCE(ws.visit_at,ds.visit_at) {order},l.created_at {order},l.id LIMIT 1")))
        .bind(workspace).bind(value).fetch_optional(&mut *conn).await?)
}
fn matched_by(m: &Matcher) -> &'static str {
    match m {
        Matcher::Lead(_) => "lead_id",
        Matcher::Phone(_) => "phone",
        Matcher::Email(_) => "email",
    }
}

struct CurrentDelivery {
    version: i32,
    enabled: bool,
    auto_send: bool,
    consent_policy: String,
}
async fn current_delivery(conn: &mut PgConnection, workspace: Uuid) -> Result<CurrentDelivery> {
    let row = sqlx::query("SELECT version,enabled,auto_send,consent_policy FROM conversion_delivery_settings WHERE workspace_id=$1")
        .bind(workspace).fetch_optional(&mut *conn).await?;
    Ok(row.map_or(
        CurrentDelivery {
            version: 0,
            enabled: false,
            auto_send: false,
            consent_policy: "per_lead".into(),
        },
        |r| CurrentDelivery {
            version: r.get("version"),
            enabled: r.get("enabled"),
            auto_send: r.get("auto_send"),
            consent_policy: r.get("consent_policy"),
        },
    ))
}
/// Whether the policy carries an attestation; not_required_outside_eea needs one.
fn attestation(policy: &str, attest: bool) -> Result<bool> {
    match policy {
        "per_lead" => Ok(false),
        "not_required_outside_eea" if attest => Ok(true),
        "not_required_outside_eea" => Err(ValidationError(
            "Confirm that this workspace does not advertise to people in the EEA, UK or Switzerland",
        )
        .into()),
        _ => Err(
            ValidationError("consent_policy must be per_lead or not_required_outside_eea").into(),
        ),
    }
}
/// Automatic sending and consent policy, inside the caller's locked, version-checked
/// transaction. auto_send_at moves only when sending turns on, so events and leads from
/// while it was off are never sent.
async fn apply_pipeline_policy(
    conn: &mut PgConnection,
    workspace: Uuid,
    current: &CurrentDelivery,
    (auto_send, policy, attested): (bool, &str, bool),
    actor: &str,
    attester: &str,
) -> Result<()> {
    // Re-attest only on a change to the attested policy, so the record names who chose it.
    let reattest = attested && current.consent_policy != policy;
    let version: i32 = sqlx::query_scalar("INSERT INTO conversion_delivery_settings AS s(workspace_id,auto_send,auto_send_at,consent_policy,attested_by,attested_at) VALUES($1,$2,CASE WHEN $2 THEN now() END,$3,CASE WHEN $4 THEN $5 END,CASE WHEN $4 THEN now() END)
        ON CONFLICT(workspace_id) DO UPDATE SET auto_send=$2,auto_send_at=CASE WHEN NOT $2 THEN NULL WHEN s.auto_send THEN s.auto_send_at ELSE now() END,consent_policy=$3,
        attested_by=CASE WHEN NOT $4 THEN NULL WHEN $6 THEN $5 ELSE s.attested_by END,attested_at=CASE WHEN NOT $4 THEN NULL WHEN $6 THEN now() ELSE s.attested_at END,version=s.version+1
        RETURNING version")
        .bind(workspace).bind(auto_send).bind(policy).bind(attested).bind(attester).bind(reattest).fetch_one(&mut *conn).await?;
    if auto_send && !current.auto_send {
        sqlx::query("UPDATE pipeline_events SET plan_after=now() WHERE workspace_id=$1 AND plan_state='blocked'")
            .bind(workspace).execute(&mut *conn).await?;
    }
    let mut payload = json!({"auto_send":auto_send,"consent_policy":policy,"attested":attested,"version":version});
    if reattest {
        payload["attested_by"] = json!(attester);
    }
    record(conn, workspace, "pipeline_policy_saved", actor, payload).await
}

impl Store {
    pub async fn conversion_actions(&self) -> Result<Vec<Value>> {
        Ok(sqlx::query_scalar("SELECT to_jsonb(a)-'workspace_id'-'id' FROM conversion_actions a WHERE workspace_id=$1 ORDER BY created_at,key")
            .bind(self.workspace).fetch_all(&self.pool).await?)
    }
    /// Key, trigger and counting are fixed once created: changing them would change
    /// transaction IDs or what counts, and could count a conversion twice.
    pub async fn save_conversion_action(&self, input: &ActionInput, actor: &str) -> Result<Value> {
        let mut input = input.clone();
        input.google_action_id = input.google_action_id.filter(|s| !s.is_empty());
        input.validate()?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let existing = sqlx::query(
            "SELECT trigger,counting,version FROM conversion_actions WHERE workspace_id=$1 AND key=$2",
        )
        .bind(self.workspace)
        .bind(&input.key)
        .fetch_optional(&mut *tx)
        .await?;
        let saved: Value = match existing {
            None => {
                if input.version != 0 {
                    return Err(Error::Conflict);
                }
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM conversion_actions WHERE workspace_id=$1",
                )
                .bind(self.workspace)
                .fetch_one(&mut *tx)
                .await?;
                if count >= 50 {
                    return Err(ValidationError("A workspace can define up to 50 actions").into());
                }
                sqlx::query_scalar("INSERT INTO conversion_actions AS a(id,workspace_id,key,name,trigger,default_value_minor,counting,google_action_id,enabled) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING to_jsonb(a)-'workspace_id'-'id'")
                    .bind(Uuid::new_v4()).bind(self.workspace).bind(&input.key).bind(input.name.trim()).bind(&input.trigger).bind(input.default_value_minor).bind(&input.counting).bind(&input.google_action_id).bind(input.enabled).fetch_one(&mut *tx).await?
            }
            Some(r) => {
                if r.get::<i32, _>("version") != input.version {
                    return Err(Error::Conflict);
                }
                if r.get::<String, _>("trigger") != input.trigger
                    || r.get::<String, _>("counting") != input.counting
                {
                    return Err(ValidationError(
                        "An action's trigger and counting cannot change. Create a new action instead.",
                    )
                    .into());
                }
                sqlx::query_scalar("UPDATE conversion_actions a SET name=$3,default_value_minor=$4,google_action_id=$5,enabled=$6,version=version+1,updated_at=now() WHERE workspace_id=$1 AND key=$2 RETURNING to_jsonb(a)-'workspace_id'-'id'")
                    .bind(self.workspace).bind(&input.key).bind(input.name.trim()).bind(input.default_value_minor).bind(&input.google_action_id).bind(input.enabled).fetch_one(&mut *tx).await?
            }
        };
        // Blocked events are reconsidered promptly with the new mapping.
        sqlx::query("UPDATE pipeline_events SET plan_after=now() WHERE workspace_id=$1 AND action_key=$2 AND plan_state='blocked'")
            .bind(self.workspace).bind(&input.key).execute(&mut *tx).await?;
        record(
            &mut tx,
            self.workspace,
            "pipeline_action_saved",
            actor,
            json!({"key":input.key,"version":saved["version"],"google_action_id":input.google_action_id,"enabled":input.enabled}),
        )
        .await?;
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn pipeline_policy(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('enabled',enabled,'auto_send',auto_send,'auto_send_at',auto_send_at,'consent_policy',consent_policy,'attested_by',attested_by,'attested_at',attested_at,'version',version) FROM conversion_delivery_settings WHERE workspace_id=$1")
            .bind(self.workspace).fetch_optional(&self.pool).await?
            .unwrap_or(json!({"enabled":false,"auto_send":false,"auto_send_at":null,"consent_policy":"per_lead","attested_by":null,"attested_at":null,"version":0})))
    }
    /// Shares the delivery settings version, so a stale pause/enable and a stale policy
    /// change cannot overwrite each other.
    pub async fn set_pipeline_policy(&self, input: &PolicyInput, actor: &str) -> Result<()> {
        let attested = attestation(&input.consent_policy, input.attest)?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let current = current_delivery(&mut tx, self.workspace).await?;
        if current.version != input.version {
            return Err(Error::Conflict);
        }
        apply_pipeline_policy(
            &mut tx,
            self.workspace,
            &current,
            (input.auto_send, &input.consent_policy, attested),
            actor,
            actor,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// The same switches for a workspace key or MCP connection holding conversions:send.
    /// Omitted fields keep their value; nothing is written when nothing changes. The rules
    /// are the owner routes' own helpers. `actor` names the credential in the audit log;
    /// `attester` is the owner the credential acts for, recorded as `attested_by`.
    pub async fn set_conversion_delivery(
        &self,
        input: &DeliveryInput,
        actor: &str,
        attester: &str,
    ) -> Result<()> {
        if input.enabled.is_none() && input.auto_send.is_none() && input.consent_policy.is_none() {
            return Err(ValidationError(
                "Set at least one of enabled, auto_send or consent_policy",
            )
            .into());
        }
        if input.attest_no_eea_advertising
            && input.consent_policy.as_deref() != Some("not_required_outside_eea")
        {
            return Err(ValidationError("attest_no_eea_advertising applies only when setting consent_policy to not_required_outside_eea").into());
        }
        if input.consent_policy.as_deref() == Some("not_required_outside_eea")
            && !input.attest_no_eea_advertising
        {
            return Err(ValidationError("Setting consent_policy to not_required_outside_eea requires attest_no_eea_advertising: true, confirming the owner said this workspace does not advertise to people in the EEA, UK or Switzerland").into());
        }
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let current = current_delivery(&mut tx, self.workspace).await?;
        if current.version != input.version {
            return Err(Error::Conflict);
        }
        let policy = input
            .consent_policy
            .clone()
            .unwrap_or_else(|| current.consent_policy.clone());
        // An unchanged attested policy keeps its original attestation.
        let attested = attestation(
            &policy,
            input.attest_no_eea_advertising || input.consent_policy.is_none(),
        )?;
        let auto_send = input.auto_send.unwrap_or(current.auto_send);
        if let Some(enabled) = input.enabled
            && enabled != current.enabled
        {
            crate::delivery::apply_delivery_enabled(&mut tx, self.workspace, enabled, actor)
                .await?;
        }
        if auto_send != current.auto_send || policy != current.consent_policy {
            apply_pipeline_policy(
                &mut tx,
                self.workspace,
                &current,
                (auto_send, &policy, attested),
                actor,
                attester,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn pipeline_event(&self, id: Uuid) -> Result<Value> {
        let row = sqlx::query(sql(format!("{EVENT} AND e.id=$2")))
            .bind(self.workspace)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(event_json(&row))
    }
    pub async fn pipeline_events(&self, page: &EventPage) -> Result<Value> {
        let limit = page.limit.unwrap_or(50);
        if !(1..=100).contains(&limit)
            || page.action.as_ref().is_some_and(|s| s.len() > 40)
            || page
                .external_id
                .as_ref()
                .is_some_and(|s| s.chars().count() > 150)
        {
            return Err(
                ValidationError("Use limit 1–100, an action key and an external_id").into(),
            );
        }
        let cursor = page
            .cursor
            .as_deref()
            .map(|c| {
                let (t, id) = c.split_once('|')?;
                Some((t.parse::<DateTime<Utc>>().ok()?, id.parse::<Uuid>().ok()?))
            })
            .map(|c| c.ok_or(ValidationError("Invalid page cursor")))
            .transpose()?;
        let rows = sqlx::query(sql(format!("{EVENT} AND ($2::text IS NULL OR e.action_key=$2) AND ($3::text IS NULL OR e.external_id=$3) AND ($4::timestamptz IS NULL OR (e.created_at,e.id)<($4,$5)) ORDER BY e.created_at DESC,e.id DESC LIMIT $6")))
            .bind(self.workspace).bind(&page.action).bind(&page.external_id).bind(cursor.map(|c| c.0)).bind(cursor.map(|c| c.1)).bind(limit + 1).fetch_all(&self.pool).await?;
        let next = (rows.len() > limit as usize).then(|| {
            let r = &rows[limit as usize - 1];
            format!(
                "{}|{}",
                r.get::<DateTime<Utc>, _>("created_at").to_rfc3339(),
                r.get::<Uuid, _>("id")
            )
        });
        let data: Vec<Value> = rows.iter().take(limit as usize).map(event_json).collect();
        Ok(json!({"data":data,"next_cursor":next}))
    }
    /// Record a business event. An identical repost is a no-op (an unmatched one is
    /// matched again); any other change is the next revision. Returns the event ID.
    pub async fn record_pipeline_event(&self, input: &EventInput, actor: &str) -> Result<Uuid> {
        let (matcher, attribution, status) = input.parse()?;
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let action = sqlx::query("SELECT trigger,default_value_minor FROM conversion_actions WHERE workspace_id=$1 AND key=$2")
            .bind(self.workspace).bind(&input.action).fetch_optional(&mut *tx).await?
            .ok_or(ValidationError("Unknown action. Create it in Conversions first."))?;
        if action.get::<String, _>("trigger") != "events" {
            return Err(ValidationError(
                "VisiConvert records this action itself when an attributed lead arrives",
            )
            .into());
        }
        let existing = sqlx::query("SELECT * FROM pipeline_events WHERE workspace_id=$1 AND action_key=$2 AND external_id=$3")
            .bind(self.workspace).bind(&input.action).bind(&input.external_id).fetch_optional(&mut *tx).await?;
        let digest = matcher.as_ref().map(|m| m.digest(attribution));
        let occurred = input.occurred_at.map(|t| t.trunc_subsecs(6));
        let Some(e) = existing else {
            if status == "canceled" {
                return Err(Error::NotFound);
            }
            let m = matcher.as_ref().ok_or(Error::NotFound)?;
            let lead = find_lead(&mut tx, self.workspace, m, attribution).await?;
            let sendable: bool = sqlx::query_scalar("SELECT COALESCE((SELECT enabled AND auto_send FROM conversion_delivery_settings WHERE workspace_id=$1),false)")
                .bind(self.workspace).fetch_one(&mut *tx).await?;
            let id = Uuid::new_v4();
            let value = input.value_minor.or(action.get("default_value_minor"));
            sqlx::query("INSERT INTO pipeline_events(id,workspace_id,action_key,external_id,lead_id,matched_by,attribution,match_digest,value_minor,occurred_at,sendable) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,COALESCE($10,now()),$11)")
                .bind(id).bind(self.workspace).bind(&input.action).bind(&input.external_id).bind(lead).bind(if lead.is_some() {matched_by(m)} else {"none"})
                .bind(attribution).bind(&digest).bind(value).bind(occurred).bind(sendable).execute(&mut *tx).await?;
            record(&mut tx, self.workspace, "pipeline_event_recorded", actor, json!({"id":id,"action":input.action,"external_id":input.external_id,"revision":1,"matched":lead.is_some(),"value_minor":value})).await?;
            tx.commit().await?;
            return Ok(id);
        };
        let id: Uuid = e.get("id");
        let revision: i32 = e.get("revision");
        let digest = digest.unwrap_or_else(|| e.get("match_digest"));
        let value = input.value_minor.or(e.get("value_minor"));
        let occurred = occurred.unwrap_or_else(|| e.get("occurred_at"));
        let rematch = digest != e.get::<String, _>("match_digest")
            || (e.get::<String, _>("matched_by") == "none" && status == "active");
        let (mut lead, mut by): (Option<Uuid>, String) = (e.get("lead_id"), e.get("matched_by"));
        if rematch && let Some(m) = &matcher {
            lead = find_lead(&mut tx, self.workspace, m, attribution).await?;
            by = if lead.is_some() {
                matched_by(m).into()
            } else {
                "none".into()
            };
        }
        let same = digest == e.get::<String, _>("match_digest")
            && value == e.get::<Option<i64>, _>("value_minor")
            && occurred == e.get::<DateTime<Utc>, _>("occurred_at")
            && status == e.get::<String, _>("status")
            && lead == e.get::<Option<Uuid>, _>("lead_id");
        if same {
            return Ok(id);
        }
        let canceled = status == "canceled";
        let mut reason: Option<String> = None;
        if canceled {
            reason = Some(
                self.cancel_pipeline_outbox(&mut tx, id, e.get("outbox_id"))
                    .await?,
            );
        }
        sqlx::query("UPDATE pipeline_events SET lead_id=$3,matched_by=$4,attribution=$5,match_digest=$6,value_minor=$7,occurred_at=$8,status=$9,revision=revision+1,plan_state=CASE WHEN $10 THEN 'final' ELSE 'pending' END,plan_after=now(),blocked_reason=$11,updated_at=now() WHERE workspace_id=$1 AND id=$2")
            .bind(self.workspace).bind(id).bind(lead).bind(&by).bind(attribution).bind(&digest).bind(value).bind(occurred).bind(status).bind(canceled).bind(&reason).execute(&mut *tx).await?;
        record(&mut tx, self.workspace, if canceled {"pipeline_event_canceled"} else {"pipeline_event_revised"}, actor, json!({"id":id,"action":input.action,"external_id":input.external_id,"revision":revision+1,"matched":lead.is_some(),"value_minor":value})).await?;
        tx.commit().await?;
        Ok(id)
    }
    /// Stop unsent work without changing the outbox version, so receipts stay pollable.
    /// Google Data Manager cannot retract, so an already sent conversion is flagged.
    async fn cancel_pipeline_outbox(
        &self,
        tx: &mut PgConnection,
        event: Uuid,
        outbox: Option<Uuid>,
    ) -> Result<String> {
        let Some(outbox) = outbox else {
            return Ok("Canceled before it was sent".into());
        };
        let owned: bool = sqlx::query_scalar("UPDATE conversion_outbox SET state='canceled',updated_at=now() WHERE workspace_id=$1 AND id=$2 AND snapshot->>'event_id'=$3 RETURNING true")
            .bind(self.workspace).bind(outbox).bind(event.to_string()).fetch_optional(&mut *tx).await?.unwrap_or(false);
        if !owned {
            return Ok("Canceled before it was sent".into());
        }
        sqlx::query("UPDATE conversion_deliveries SET state='canceled',due_at=NULL,diagnostic='Event canceled before it was sent.',updated_at=now() WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NULL AND state IN ('queued','retry','validated')")
            .bind(self.workspace).bind(outbox).execute(&mut *tx).await?;
        let sent = sqlx::query("UPDATE conversion_deliveries SET review_required=true,diagnostic='Event canceled after it was sent. Google cannot retract it; adjust or remove it in Google Ads.',updated_at=now() WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NOT NULL")
            .bind(self.workspace).bind(outbox).execute(&mut *tx).await?;
        Ok(if sent.rows_affected() > 0 {
            "Canceled after it was sent. Google cannot retract it; review it in Google Ads.".into()
        } else {
            "Canceled before it was sent".into()
        })
    }
    pub async fn pipeline_overview(&self) -> Result<Value> {
        let events = self
            .pipeline_events(&EventPage {
                limit: Some(100),
                ..Default::default()
            })
            .await?;
        Ok(
            json!({"actions":self.conversion_actions().await?,"policy":self.pipeline_policy().await?,"events":events["data"]}),
        )
    }
    /// Facts for the setup checklist. The server adds whether Google is connected.
    pub async fn pipeline_setup(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object(
            'customer_id',NULLIF(g.configuration->>'customer_id',''),'settings_version',COALESCE(g.version,0),
            'checked_at',c.checked_at,'check_valid',COALESCE((c.validation->>'valid')::boolean,false),
            'check_current',COALESCE(c.validated_settings_version=g.version AND c.checked_at>now()-interval '24 hours',false),
            'check_message',c.validation->>'message','pipeline_actions_valid',COALESCE((c.validation->>'pipeline_actions_valid')::boolean,false),
            'delivery_enabled',COALESCE(s.enabled,false),'auto_send',COALESCE(s.auto_send,false),'consent_policy',COALESCE(s.consent_policy,'per_lead'),
            'actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1),
            'enabled_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled),
            'mapped_actions',(SELECT count(*) FROM conversion_actions WHERE workspace_id=$1 AND enabled AND google_action_id IS NOT NULL))
            FROM (SELECT $1::uuid AS w) x
            LEFT JOIN conversion_settings g ON g.workspace_id=x.w
            LEFT JOIN google_connections c ON c.workspace_id=x.w
            LEFT JOIN conversion_delivery_settings s ON s.workspace_id=x.w")
            .bind(self.workspace).fetch_one(&self.pool).await?)
    }
    /// Google action IDs mapped by enabled pipeline actions, for destination checks.
    pub async fn pipeline_action_ids(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT DISTINCT google_action_id FROM conversion_actions WHERE workspace_id=$1 AND enabled AND google_action_id IS NOT NULL ORDER BY 1 LIMIT 48")
            .bind(self.workspace).fetch_all(&self.pool).await?)
    }
    /// Workspaces with pipeline work due now: events to plan, or new_lead actions.
    pub async fn pipeline_workspaces(&self) -> Result<Vec<Uuid>> {
        Ok(sqlx::query_scalar("SELECT workspace_id FROM pipeline_events WHERE plan_state IN ('pending','blocked') AND plan_after<=now() UNION SELECT workspace_id FROM conversion_actions WHERE enabled AND trigger='new_lead' LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    /// Record a new_lead event for each attributed lead that arrived after its action was
    /// created. Leads from before auto-send was turned on are kept but never sent.
    pub async fn create_lead_events(&self) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let n = sqlx::query(sql(format!("WITH added AS (INSERT INTO pipeline_events(id,workspace_id,action_key,external_id,lead_id,matched_by,attribution,match_digest,value_minor,occurred_at,sendable)
            SELECT gen_random_uuid(),a.workspace_id,a.key,l.id::text,l.id,'new_lead','first_click','new_lead',a.default_value_minor,l.created_at,
             COALESCE(s.enabled AND s.auto_send AND l.created_at>=s.auto_send_at,false)
            FROM conversion_actions a JOIN leads l ON l.workspace_id=a.workspace_id AND l.created_at>=a.created_at AND l.created_at>now()-interval '90 days' AND NOT l.is_demo
            {CLICK}
            LEFT JOIN conversion_delivery_settings s ON s.workspace_id=a.workspace_id
            WHERE a.workspace_id=$1 AND a.enabled AND a.trigger='new_lead' AND {HAS_CLICK}
             AND NOT EXISTS(SELECT 1 FROM pipeline_events e WHERE e.workspace_id=a.workspace_id AND e.action_key=a.key AND e.external_id=l.id::text)
            ORDER BY l.created_at LIMIT 200 ON CONFLICT DO NOTHING RETURNING id,action_key,external_id,value_minor)
            INSERT INTO conversion_events(workspace_id,kind,actor,payload) SELECT $1,'pipeline_event_recorded','system',jsonb_build_object('id',id,'action',action_key,'external_id',external_id,'revision',1,'matched',true,'value_minor',value_minor) FROM added")))
            .bind(self.workspace).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(n)
    }
    /// Login to refresh the destination check with, when due events need a check that is
    /// missing, older than 24 hours, for other settings, or lacks a mapped action.
    pub async fn pipeline_refresh_login(&self) -> Result<Option<String>> {
        let due: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pipeline_events WHERE workspace_id=$1 AND plan_state IN ('pending','blocked') AND plan_after<=now() AND sendable) OR EXISTS(SELECT 1 FROM conversion_deliveries d JOIN conversion_outbox q ON q.workspace_id=d.workspace_id AND q.id=d.outbox_id WHERE d.workspace_id=$1 AND q.kind='pipeline' AND d.state IN ('queued','retry'))")
            .bind(self.workspace).fetch_one(&self.pool).await?;
        if !due {
            return Ok(None);
        }
        let settings = self.google_conversion_settings().await?;
        let c = self.google_connection().await?;
        let Some(report) = c.validation.filter(|_| c.encrypted_token.is_some()) else {
            return Ok(None);
        };
        let login = report["login_customer_id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if login.is_empty() || settings.version == 0 {
            return Ok(None);
        }
        let validated: Option<i32> = sqlx::query_scalar(
            "SELECT validated_settings_version FROM google_connections WHERE workspace_id=$1",
        )
        .bind(self.workspace)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        let age = c.checked_at.map_or(Duration::days(365), |t| Utc::now() - t);
        let ids = self.pipeline_action_ids().await?;
        let missing = report["valid"] != true
            || ids.iter().any(|id| {
                !report["actions"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v["id"] == id.as_str()))
            });
        let stale = age > Duration::hours(24) || validated != Some(settings.version);
        Ok((stale || (missing && age > Duration::minutes(10))).then_some(login))
    }
    /// Turn due event revisions into frozen outbox snapshots and queued deliveries, or
    /// record why they are blocked. Holds the workspace lock used by every edit.
    pub async fn plan_pipeline(&self, only: Option<Uuid>, build: Build<'_>) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        self.lock_inquiry_writes(&mut tx).await?;
        let settings = conversions::settings(&mut tx, self.workspace).await?;
        let policy = sqlx::query("SELECT enabled,auto_send,consent_policy FROM conversion_delivery_settings WHERE workspace_id=$1")
            .bind(self.workspace).fetch_optional(&mut *tx).await?;
        let (enabled, auto_send, consent_policy) =
            policy
                .as_ref()
                .map_or((false, false, "per_lead".to_string()), |r| {
                    (
                        r.get("enabled"),
                        r.get("auto_send"),
                        r.get("consent_policy"),
                    )
                });
        let connection = sqlx::query("SELECT revision,validation,checked_at,validated_settings_version FROM google_connections WHERE workspace_id=$1 AND encrypted_token IS NOT NULL")
            .bind(self.workspace).fetch_optional(&mut *tx).await?;
        let rows = sqlx::query(sql(format!("SELECT e.id,e.action_key,e.external_id,e.lead_id,e.value_minor,e.occurred_at,e.revision,e.status,e.sendable,e.outbox_id,
            a.enabled AS action_enabled,a.google_action_id,a.counting,a.trigger,l.channel,l.is_demo,i.qualification,
            COALESCE(ws.evidence,ds.evidence) AS evidence,COALESCE(ws.visit_at,ds.visit_at) AS visit_at,cc.evidence AS consent,cc.version AS consent_version
            FROM pipeline_events e JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key
            LEFT JOIN leads l ON l.workspace_id=e.workspace_id AND l.id=e.lead_id
            LEFT JOIN inquiries i ON i.workspace_id=l.workspace_id AND i.id=l.inquiry_id
            {CLICK}
            LEFT JOIN conversion_consents cc ON cc.workspace_id=e.workspace_id AND cc.lead_id=e.lead_id
            WHERE e.workspace_id=$1 AND e.plan_state IN ('pending','blocked') AND e.plan_after<=now() AND ($2::uuid IS NULL OR e.id=$2)
            ORDER BY e.plan_after,e.id LIMIT 100 FOR UPDATE OF e")))
            .bind(self.workspace).bind(only).fetch_all(&mut *tx).await?;
        let now = Utc::now();
        let mut planned = 0;
        for r in rows {
            let id: Uuid = r.get("id");
            let outcome = plan_one(
                &mut tx,
                self.workspace,
                &r,
                &settings,
                (enabled, auto_send, &consent_policy),
                connection.as_ref(),
                now,
                build,
            )
            .await?;
            let (state, reason, delay) = match outcome {
                Plan::Queued => {
                    planned += 1;
                    continue;
                }
                Plan::Final(reason) => ("final", reason, 0),
                Plan::Blocked(reason, minutes) => ("blocked", reason, minutes),
            };
            sqlx::query("UPDATE pipeline_events SET plan_state=$3,blocked_reason=$4,plan_after=now()+make_interval(mins=>$5) WHERE workspace_id=$1 AND id=$2")
                .bind(self.workspace).bind(id).bind(state).bind(reason).bind(delay).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(planned)
    }
}
/// How long an automatic `new_lead` conversion waits before it is sent.
const NEW_LEAD_HOLD: Duration = Duration::hours(6);
enum Plan {
    Queued,
    Final(&'static str),
    Blocked(&'static str, i32),
}
#[allow(clippy::too_many_arguments)]
async fn plan_one(
    tx: &mut PgConnection,
    workspace: Uuid,
    r: &sqlx::postgres::PgRow,
    settings: &conversions::Settings,
    (enabled, auto_send, consent_policy): (bool, bool, &str),
    connection: Option<&sqlx::postgres::PgRow>,
    now: DateTime<Utc>,
    build: Build<'_>,
) -> Result<Plan> {
    use Plan::*;
    let id: Uuid = r.get("id");
    let event_at: DateTime<Utc> = r.get("occurred_at");
    let visit_at: Option<DateTime<Utc>> = r.get("visit_at");
    let evidence: Value = r.get::<Option<Value>, _>("evidence").unwrap_or_default();
    if r.get::<String, _>("status") != "active" {
        return Ok(Final("Canceled"));
    }
    if !r.get::<bool, _>("sendable") {
        return Ok(Final(
            "Recorded while automatic sending was off; kept for reporting and never sent",
        ));
    }
    if !r.get::<bool, _>("action_enabled") {
        return Ok(Blocked("Action is disabled", 15));
    }
    let Some(action_id) = r.get::<Option<String>, _>("google_action_id") else {
        return Ok(Blocked(
            "No Google conversion action is mapped to this action",
            15,
        ));
    };
    if r.get::<Option<Uuid>, _>("lead_id").is_none() {
        return Ok(Final("No attributed lead matched this event"));
    }
    if r.get::<Option<bool>, _>("is_demo") == Some(true) {
        return Ok(Final("Demo data is never sent"));
    }
    if r.get::<Option<String>, _>("qualification").as_deref() == Some("spam") {
        return Ok(Blocked("The inquiry is marked spam", 15));
    }
    if r.get::<String, _>("trigger") == "new_lead" {
        // Automatic lead conversions cannot be retracted from Google, so give a person
        // time to mark test calls and wrong numbers before sending.
        if r.get::<Option<String>, _>("qualification").as_deref() == Some("unqualified") {
            return Ok(Blocked("The inquiry is marked unqualified", 15));
        }
        let wait = event_at + NEW_LEAD_HOLD - now;
        if wait > Duration::zero() {
            let minutes = i32::try_from(wait.num_minutes() + 1).unwrap_or(i32::MAX);
            return Ok(Blocked(
                "Held for 6 hours so the lead can be reviewed before sending",
                minutes,
            ));
        }
    }
    let mut identifiers = serde_json::Map::new();
    for key in ["gclid", "gbraid", "wbraid"] {
        if let Some(v) = evidence
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            identifiers.insert(key.into(), json!(v));
        }
    }
    let Some(visit) = visit_at.filter(|_| !identifiers.is_empty()) else {
        return Ok(if now - event_at > Duration::days(90) {
            Final("No Google click")
        } else {
            Blocked("No Google click", 15)
        });
    };
    let channel: String = r.get("channel");
    if channel == "call" && !settings.allow_inferred_calls {
        return Ok(Blocked(
            "Inferred call attribution requires explicit opt-in in the Google destination settings",
            15,
        ));
    }
    if event_at < visit {
        return Ok(Final("Event predates the ad visit"));
    }
    if event_at - visit > Duration::days(90) {
        return Ok(Final("Outside the click window"));
    }
    let mut consent: Consent = r
        .get::<Option<Value>, _>("consent")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ValidationError("Invalid saved consent evidence"))?
        .unwrap_or_default();
    consent.version = r.get::<Option<i32>, _>("consent_version").unwrap_or(0);
    if consent_policy == "per_lead"
        && (consent.ad_user_data != Decision::Granted
            || consent.ad_personalization == Decision::Unknown
            || consent.effective_at.is_none_or(|c| c > event_at))
    {
        return Ok(Blocked("No advertising consent recorded for this lead", 15));
    }
    if settings.version == 0 || settings.customer_id.is_empty() {
        return Ok(Blocked("Save a Google Ads destination first", 15));
    }
    if !enabled || !auto_send {
        return Ok(Blocked("Automatic sending is off", 60));
    }
    let Some(g) = connection else {
        return Ok(Blocked("Connect Google to send", 15));
    };
    let report: Value = g.get::<Option<Value>, _>("validation").unwrap_or_default();
    let checked: Option<DateTime<Utc>> = g.get("checked_at");
    if g.get::<Option<i32>, _>("validated_settings_version") != Some(settings.version)
        || !checked.is_some_and(|t| t > now - Duration::hours(24))
    {
        return Ok(Blocked("Waiting for the Google destination check", 2));
    }
    if report["account"]["status"] != "ENABLED" {
        return Ok(Blocked(
            "The Google destination check failed; see Conversions",
            10,
        ));
    }
    let Some(window) = report["actions"]
        .as_array()
        .and_then(|xs| xs.iter().find(|v| v["id"] == action_id.as_str()))
        .map(|v| (v["eligible"] == true).then(|| v["window_days"].as_i64().unwrap_or(0)))
    else {
        return Ok(Blocked("Waiting for the Google destination check", 2));
    };
    let Some(window) = window.filter(|w| *w > 0) else {
        return Ok(Blocked(
            "The mapped Google action is not an enabled import action owned by this account",
            15,
        ));
    };
    if event_at - visit > Duration::days(window.min(90)) {
        return Ok(Final("Outside the click window"));
    }
    let login = report["login_customer_id"].as_str().unwrap_or("");
    let action_key: String = r.get("action_key");
    let external_id: String = r.get("external_id");
    let counting: String = r.get("counting");
    let click = ["gclid", "gbraid", "wbraid"]
        .iter()
        .find_map(|k| identifiers.get(*k).and_then(Value::as_str))
        .unwrap_or_default();
    let kind = format!("pipeline:{action_key}");
    let (entity_key, transaction_id) = if counting == "once_per_click" {
        (
            format!(
                "{action_key}:click:{}",
                &conversions::digest(&json!(click))[..32]
            ),
            conversions::transaction_id(workspace, &kind, &format!("click:{click}")),
        )
    } else {
        (
            format!("{action_key}:event:{external_id}"),
            conversions::transaction_id(workspace, &kind, &format!("event:{external_id}")),
        )
    };
    let outbox = sqlx::query("SELECT id,state,snapshot->>'event_id' AS event_id FROM conversion_outbox WHERE workspace_id=$1 AND kind='pipeline' AND entity_key=$2")
        .bind(workspace).bind(&entity_key).fetch_optional(&mut *tx).await?;
    let previous: Option<Uuid> = r.get("outbox_id");
    let outbox_id: Option<Uuid> = outbox.as_ref().map(|o| o.get("id"));
    if let Some(prev) = previous.filter(|p| Some(*p) != outbox_id) {
        let sent: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NOT NULL)")
            .bind(workspace).bind(prev).fetch_one(&mut *tx).await?;
        if sent {
            return Ok(Final(
                "Already sent for a different click; its lead cannot change",
            ));
        }
    }
    let mut prior = None;
    if let Some(o) = &outbox {
        if o.get::<Option<String>, _>("event_id") != Some(id.to_string()) {
            return Ok(Final(
                "Already counted for this ad click (counted once per click)",
            ));
        }
        if o.get::<String, _>("state") == "canceled" {
            return Ok(Final(
                "This event was canceled; post a new external_id to count it again",
            ));
        }
        // A revision waits while an earlier one is in flight, failed or uncertain.
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND (state IN ('queued','retry','sending','submitted','polling','uncertain') OR (attempted_at IS NOT NULL AND state IN ('failed','needs_review'))))")
            .bind(workspace).bind(outbox_id).fetch_one(&mut *tx).await?;
        if waiting {
            return Ok(Blocked(
                "Waiting for the previous delivery of this event to finish; failed or uncertain sends need review in Conversions",
                2,
            ));
        }
        prior = sqlx::query_scalar("SELECT request FROM conversion_deliveries WHERE workspace_id=$1 AND outbox_id=$2 AND attempted_at IS NOT NULL ORDER BY created_at LIMIT 1")
            .bind(workspace).bind(outbox_id).fetch_optional(&mut *tx).await?;
    }
    let revision: i32 = r.get("revision");
    let snapshot = json!({"kind":"pipeline","event_id":id,"action_key":action_key,"external_id":external_id,"revision":revision,
        "lead_id":r.get::<Uuid,_>("lead_id"),"channel":channel,"event_at":event_at,"visit_at":visit,"value_minor":r.get::<Option<i64>,_>("value_minor"),
        "currency":"USD","click_identifiers":identifiers,"consent_policy":consent_policy,"consent":consent,"counting":counting,
        "customer_id":settings.customer_id,"google_action_id":action_id,"transaction_id":transaction_id,
        "confidence":if channel == "call" {"inferred"} else {"browser-reported"}});
    let request = match build(&snapshot, login, prior.as_ref()) {
        Ok(v) => v,
        Err(reason) => return Ok(Final(reason)),
    };
    let fingerprint = conversions::digest(&snapshot);
    let (outbox_id, version): (Uuid, i32) = match outbox_id {
        Some(o) => {
            let v = sqlx::query_scalar("UPDATE conversion_outbox SET snapshot=$3,fingerprint=$4,state='awaiting_connection',version=version+1,updated_at=now() WHERE workspace_id=$1 AND id=$2 RETURNING version")
                .bind(workspace).bind(o).bind(&snapshot).bind(&fingerprint).fetch_one(&mut *tx).await?;
            (o, v)
        }
        None => {
            let o = Uuid::new_v4();
            sqlx::query("INSERT INTO conversion_outbox(id,workspace_id,kind,entity_key,transaction_id,fingerprint,snapshot,state) VALUES($1,$2,'pipeline',$3,$4,$5,$6,'awaiting_connection')")
                .bind(o).bind(workspace).bind(&entity_key).bind(&transaction_id).bind(&fingerprint).bind(&snapshot).execute(&mut *tx).await?;
            (o, 1)
        }
    };
    let delivery = Uuid::new_v4();
    sqlx::query("INSERT INTO conversion_deliveries(id,workspace_id,outbox_id,outbox_version,connection_revision,snapshot,request,state,due_at) VALUES($1,$2,$3,$4,$5,$6,$7,'queued',now())")
        .bind(delivery).bind(workspace).bind(outbox_id).bind(version).bind(g.get::<i64,_>("revision")).bind(&snapshot).bind(&request).execute(&mut *tx).await?;
    sqlx::query("UPDATE pipeline_events SET plan_state='planned',blocked_reason=NULL,outbox_id=$3 WHERE workspace_id=$1 AND id=$2")
        .bind(workspace).bind(id).bind(outbox_id).execute(&mut *tx).await?;
    record(
        tx,
        workspace,
        "delivery_queued",
        "planner",
        json!({"delivery_id":delivery,"event_id":id,"revision":revision}),
    )
    .await?;
    Ok(Queued)
}
