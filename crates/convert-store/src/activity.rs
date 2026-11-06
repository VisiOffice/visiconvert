//! One read-only, newest-first timeline over records that already exist: calls and forms,
//! pipeline events and their Google and Microsoft deliveries, webhook deliveries, and the
//! configuration audit trails. Every row is summarized from columns other endpoints already
//! expose; provider requests, frozen snapshots, click IDs, form payloads, webhook payloads,
//! secrets and endpoint URLs beyond their host are never read into the response.
use crate::{Result, Store};
use chrono::{DateTime, SecondsFormat, Utc};
use convert_domain::ValidationError;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::Row;

pub const KINDS: [&str; 5] = ["call", "form", "conversion", "webhook", "api"];

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityQuery {
    /// One of KINDS; all kinds when absent.
    pub kind: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// Each source is one bounded, index-ordered branch. `at` and `key` order the timeline;
/// `key` is `source:id` (bigint ids zero-padded) so ties across sources page stably.
struct Source {
    kinds: &'static [&'static str],
    at: &'static str,
    key: &'static str,
    fields: &'static str,
    from: &'static str,
}

const HOST: &str = r"substring({url} from '^[A-Za-z][A-Za-z0-9+.-]*://(?:[^@/?#]*@)?([^/:?#]+)')";

const SOURCES: &[Source] = &[
    Source {
        kinds: &["call", "form"],
        at: "l.created_at",
        key: "'interaction:'||l.id::text",
        fields: "'source','interaction','id',l.id,'channel',l.channel,'inquiry_id',l.inquiry_id,'is_demo',l.is_demo,'name',l.name,'contact',l.contact,'marketing_source',l.source,'campaign',l.campaign,'tracking_number',l.tracking_number,'destination',l.destination,'call_status',l.call_status,'duration_seconds',l.duration_seconds,'provider',l.provider,'site',(SELECT s.name FROM website_submissions w JOIN sites s ON s.workspace_id=w.workspace_id AND s.id=w.site_id WHERE w.workspace_id=l.workspace_id AND w.lead_id=l.id LIMIT 1)",
        from: "leads l WHERE l.workspace_id=$1",
    },
    Source {
        kinds: &["conversion"],
        at: "e.created_at",
        key: "'pipeline_event:'||e.id::text",
        fields: "'source','pipeline_event','id',e.id,'action',e.action_key,'action_name',a.name,'external_id',e.external_id,'revision',e.revision,'status',e.status,'value_minor',e.value_minor,'matched_by',e.matched_by,'lead_id',e.lead_id,'inquiry_id',l.inquiry_id,'is_demo',COALESCE(l.is_demo,false),'plan_state',e.plan_state,'blocked_reason',e.blocked_reason,'sendable',e.sendable",
        from: "pipeline_events e LEFT JOIN conversion_actions a ON a.workspace_id=e.workspace_id AND a.key=e.action_key LEFT JOIN leads l ON l.workspace_id=e.workspace_id AND l.id=e.lead_id WHERE e.workspace_id=$1",
    },
    Source {
        kinds: &["conversion"],
        at: "d.created_at",
        key: "'google_delivery:'||d.id::text",
        fields: "'source','google_delivery','id',d.id,'state',d.state,'attempts',d.attempts,'diagnostic',d.diagnostic,'prepared_kind',q.kind,'action',CASE WHEN q.kind='pipeline' THEN d.snapshot->>'action_key' END,'action_name',CASE WHEN q.kind='pipeline' THEN a.name END,'external_id',CASE WHEN q.kind='pipeline' THEN d.snapshot->>'external_id' END,'revision',CASE WHEN q.kind='pipeline' THEN d.snapshot->'revision' END,'value_minor',CASE WHEN q.kind='pipeline' THEN d.snapshot->'value_minor' END",
        from: "conversion_deliveries d JOIN conversion_outbox q ON q.workspace_id=d.workspace_id AND q.id=d.outbox_id LEFT JOIN conversion_actions a ON a.workspace_id=d.workspace_id AND a.key=d.snapshot->>'action_key' WHERE d.workspace_id=$1",
    },
    Source {
        kinds: &["conversion"],
        at: "d.created_at",
        key: "'microsoft_delivery:'||d.id::text",
        fields: "'source','microsoft_delivery','id',d.id,'delivery_kind',d.kind,'state',d.state,'attempts',d.attempts,'diagnostic',d.diagnostic,'action',d.snapshot->>'action_key','action_name',a.name,'external_id',d.snapshot->>'external_id','revision',d.event_revision,'goal_name',d.request->>'goal_name'",
        from: "microsoft_deliveries d LEFT JOIN conversion_actions a ON a.workspace_id=d.workspace_id AND a.key=d.snapshot->>'action_key' WHERE d.workspace_id=$1",
    },
    Source {
        kinds: &["webhook"],
        at: "d.created_at",
        key: "'webhook_delivery:'||d.id::text",
        fields: "'source','webhook_delivery','id',d.id,'event_type',v.type,'state',d.state,'attempts',d.attempts,'http_status',d.last_status,'error',d.last_error,'last_attempt_at',d.last_attempt_at,'endpoint_id',d.endpoint_id,'endpoint_host',{host:e.url},'endpoint_description',e.description",
        from: "webhook_deliveries d JOIN webhook_events v ON v.workspace_id=d.workspace_id AND v.id=d.event_id JOIN webhook_endpoints e ON e.workspace_id=d.workspace_id AND e.id=d.endpoint_id WHERE d.workspace_id=$1",
    },
    // Worker and trigger rows restate the delivery and pipeline rows above; they are left out
    // here, and migration 0028 indexes exactly the rows that remain.
    Source {
        kinds: &["api"],
        at: "e.created_at",
        key: "'conversion_audit:'||lpad(e.id::text,20,'0')",
        fields: "'source','conversion_audit','id',e.id,'event',e.kind,'actor',e.actor,'payload',e.payload",
        from: "conversion_events e WHERE e.workspace_id=$1 AND e.actor NOT IN ('worker','system') AND e.kind NOT IN ('delivery_status','invalidated','pipeline_event_recorded','pipeline_event_canceled')",
    },
    Source {
        kinds: &["api"],
        at: "e.created_at",
        key: "'api_key_audit:'||lpad(e.id::text,20,'0')",
        fields: "'source','api_key_audit','id',e.id,'event',e.kind,'actor',e.actor,'key_name',k.name",
        from: "api_key_events e JOIN workspace_api_keys k ON k.workspace_id=e.workspace_id AND k.id=e.key_id WHERE e.workspace_id=$1",
    },
    Source {
        kinds: &["api"],
        at: "e.created_at",
        key: "'webhook_endpoint_audit:'||lpad(e.id::text,20,'0')",
        fields: "'source','webhook_endpoint_audit','id',e.id,'event',e.kind,'actor',e.actor,'endpoint_id',e.endpoint_id,'endpoint_host',{host:w.url},'endpoint_description',w.description",
        from: "webhook_endpoint_events e LEFT JOIN webhook_endpoints w ON w.workspace_id=e.workspace_id AND w.id=e.endpoint_id WHERE e.workspace_id=$1",
    },
];

/// Audit payload keys that are plain settings or identifiers. Anything else is dropped.
const AUDIT_KEYS: [&str; 14] = [
    "action",
    "key",
    "name",
    "external_id",
    "revision",
    "enabled",
    "auto_send",
    "consent_policy",
    "valid",
    "settings_version",
    "goal_name",
    "state",
    "allow_inferred_calls",
    "include_since",
];

fn branch(source: &Source, channel: Option<&str>) -> String {
    let fields = source
        .fields
        .replace("{host:e.url}", &HOST.replace("{url}", "e.url"))
        .replace("{host:w.url}", &HOST.replace("{url}", "w.url"));
    let channel = channel.map_or(String::new(), |c| format!(" AND l.channel='{c}'"));
    let (at, key) = (source.at, source.key);
    // `at<=$3` bounds the index range; ties at the cursor time compare the stable key.
    format!(
        "(SELECT {at} AS at,{key} AS key,jsonb_build_object({fields}) AS f FROM {from}{channel} \
         AND ($2::timestamptz IS NULL OR {at}>=$2) \
         AND ($3::timestamptz IS NULL OR ({at}<=$3 AND ({at}<$3 OR ({key}) COLLATE \"C\"<$4))) \
         ORDER BY {at} DESC,({key}) COLLATE \"C\" DESC LIMIT $5)",
        from = source.from
    )
}

fn text(f: &Value, key: &str) -> Option<String> {
    f[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn sentence(word: &str) -> String {
    let words = word.replace(['_', '.'], " ");
    let mut chars = words.chars();
    chars.next().map_or(String::new(), |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}
fn delivery_outcome(state: &str) -> &'static str {
    match state {
        "processed" | "succeeded" => "success",
        "failed" => "failure",
        "uncertain" | "needs_review" | "retry" => "warning",
        "canceled" => "neutral",
        _ => "pending",
    }
}
fn event_label(f: &Value) -> String {
    let action = text(f, "action_name").or_else(|| text(f, "action"));
    match (action, text(f, "external_id")) {
        (Some(a), Some(id)) => format!("{a} · {id}"),
        (Some(a), None) => a,
        (None, Some(id)) => id,
        (None, None) => text(f, "prepared_kind").map_or("conversion".into(), |k| sentence(&k)),
    }
}
fn compact(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).collect())
        }
        other => other,
    }
}

/// Builds the public item for one row. Only fields named here leave the store.
fn item(at: DateTime<Utc>, f: &Value) -> Value {
    let source = f["source"].as_str().unwrap_or_default();
    let mut lead = (Value::Null, Value::Null, false);
    let mut actor = Value::Null;
    let (kind, summary, status, outcome, detail) = match source {
        "interaction" => {
            lead = (
                f["id"].clone(),
                f["inquiry_id"].clone(),
                f["is_demo"] == true,
            );
            let who = text(f, "name")
                .or_else(|| text(f, "contact"))
                .unwrap_or("Unknown".into());
            let from = text(f, "marketing_source").unwrap_or("Unknown source".into());
            if f["channel"] == "call" {
                let status = text(f, "call_status").unwrap_or("received".into());
                let outcome = match status.as_str() {
                    "completed" => "success",
                    "failed" => "failure",
                    "busy" | "no-answer" | "canceled" => "warning",
                    "received" => "neutral",
                    _ => "pending",
                };
                (
                    "call",
                    format!("Call from {who} · {from}"),
                    status,
                    outcome,
                    json!({"contact":f["contact"],"campaign":f["campaign"],"tracking_number":f["tracking_number"],"forwarded_to":f["destination"],"duration_seconds":f["duration_seconds"],"provider":f["provider"]}),
                )
            } else {
                let site = text(f, "site").map_or(String::new(), |s| format!(" on {s}"));
                (
                    "form",
                    format!("Form from {who} · {from}{site}"),
                    "received".into(),
                    "success",
                    json!({"contact":f["contact"],"campaign":f["campaign"],"site":f["site"]}),
                )
            }
        }
        "pipeline_event" => {
            lead = (
                f["lead_id"].clone(),
                f["inquiry_id"].clone(),
                f["is_demo"] == true,
            );
            let revision = f["revision"].as_i64().unwrap_or(1);
            let verb = if revision > 1 { "revised" } else { "recorded" };
            let (status, outcome) = if f["status"] == "canceled" {
                ("canceled", "neutral")
            } else if f["plan_state"] == "blocked" {
                ("blocked", "warning")
            } else {
                (verb, "success")
            };
            (
                "conversion",
                format!("Pipeline event {verb} · {}", event_label(f)),
                status.into(),
                outcome,
                json!({"action":f["action"],"external_id":f["external_id"],"revision":f["revision"],"value_minor":f["value_minor"],"matched_by":f["matched_by"],"sendable":f["sendable"],"reason":f["blocked_reason"]}),
            )
        }
        "google_delivery" | "microsoft_delivery" => {
            let state = text(f, "state").unwrap_or_default();
            let what = match (source, f["delivery_kind"].as_str()) {
                ("google_delivery", _) => "Google Ads upload",
                (_, Some("restate")) => "Microsoft Ads restatement",
                (_, Some("retract")) => "Microsoft Ads retraction",
                _ => "Microsoft Ads upload",
            };
            (
                "conversion",
                format!("{what} · {}", event_label(f)),
                state.clone(),
                delivery_outcome(&state),
                json!({"destination":if source == "google_delivery" {"google"} else {"microsoft"},"action":f["action"],"external_id":f["external_id"],"revision":f["revision"],"value_minor":f["value_minor"],"goal_name":f["goal_name"],"attempts":f["attempts"],"diagnostic":f["diagnostic"]}),
            )
        }
        "webhook_delivery" => {
            let state = text(f, "state").unwrap_or_default();
            let host = text(f, "endpoint_host").unwrap_or("endpoint".into());
            let status = f["http_status"]
                .as_i64()
                .map_or(state.clone(), |s| s.to_string());
            (
                "webhook",
                format!(
                    "{} → {host}",
                    text(f, "event_type").unwrap_or("event".into())
                ),
                status,
                delivery_outcome(&state),
                json!({"event_type":f["event_type"],"endpoint":host,"endpoint_id":f["endpoint_id"],"endpoint_description":f["endpoint_description"],"state":state,"attempts":f["attempts"],"http_status":f["http_status"],"error":f["error"],"last_attempt_at":f["last_attempt_at"]}),
            )
        }
        _ => {
            actor = f["actor"].clone();
            let event = text(f, "event").unwrap_or_default();
            let summary = match source {
                "api_key_audit" => format!(
                    "API key “{}” {}",
                    text(f, "key_name").unwrap_or("key".into()),
                    event.replace('_', " ")
                ),
                "webhook_endpoint_audit" => format!(
                    "Webhook endpoint {} {}",
                    text(f, "endpoint_host").unwrap_or("(deleted)".into()),
                    event.replace('_', " ")
                ),
                _ => sentence(&event),
            };
            let mut detail = Map::new();
            detail.insert("event".into(), json!(event));
            if let Some(payload) = f["payload"].as_object() {
                for key in AUDIT_KEYS {
                    match payload.get(key) {
                        Some(Value::String(s)) => {
                            detail
                                .insert(key.into(), json!(s.chars().take(200).collect::<String>()));
                        }
                        Some(v @ (Value::Number(_) | Value::Bool(_))) => {
                            detail.insert(key.into(), v.clone());
                        }
                        _ => {}
                    }
                }
            }
            for key in ["key_name", "endpoint_host", "endpoint_description"] {
                if let Some(v) = text(f, key) {
                    detail.insert(key.into(), json!(v));
                }
            }
            (
                "api",
                summary,
                "recorded".into(),
                "neutral",
                Value::Object(detail),
            )
        }
    };
    json!({
        "id": f["id"], "source": source, "kind": kind,
        "occurred_at": at.to_rfc3339_opts(SecondsFormat::Micros, true),
        "summary": summary, "status": status, "outcome": outcome,
        "actor": actor, "lead_id": lead.0, "inquiry_id": lead.1, "is_demo": lead.2,
        "detail": compact(detail),
    })
}

fn parse_cursor(cursor: &str) -> Result<(DateTime<Utc>, String)> {
    let invalid = || ValidationError("Invalid page cursor");
    if cursor.len() > 200 {
        return Err(invalid().into());
    }
    let (time, key) = cursor.split_once('|').ok_or_else(invalid)?;
    let time = DateTime::parse_from_rfc3339(time)
        .map_err(|_| invalid())?
        .with_timezone(&Utc);
    if key.is_empty() || !key.contains(':') {
        return Err(invalid().into());
    }
    Ok((time, key.to_owned()))
}

impl Store {
    /// Newest first. `next_cursor` continues the same filters; it is opaque to callers.
    pub async fn activity(&self, query: &ActivityQuery) -> Result<Value> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(ValidationError("Use limit 1–100").into());
        }
        let kind = query
            .kind
            .as_deref()
            .filter(|k| !k.is_empty() && *k != "all");
        if kind.is_some_and(|k| !KINDS.contains(&k)) {
            return Err(ValidationError("Use kind call, form, conversion, webhook or api").into());
        }
        let cursor = query.cursor.as_deref().map(parse_cursor).transpose()?;
        let branches: Vec<String> = SOURCES
            .iter()
            .filter(|s| kind.is_none_or(|k| s.kinds.contains(&k)))
            .map(|s| branch(s, kind.filter(|_| s.kinds.len() > 1)))
            .collect();
        let sql = format!(
            "SELECT at,key,f FROM ({}) a ORDER BY at DESC,key COLLATE \"C\" DESC LIMIT $5",
            branches.join(" UNION ALL ")
        );
        // Built only from the constant branches above; every value is a bind parameter.
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(self.workspace)
            .bind(query.since)
            .bind(cursor.as_ref().map(|c| c.0))
            .bind(cursor.as_ref().map(|c| c.1.clone()))
            .bind(limit + 1)
            .fetch_all(&self.pool)
            .await?;
        let next = (rows.len() > limit as usize).then(|| {
            let last = &rows[limit as usize - 1];
            format!(
                "{}|{}",
                last.get::<DateTime<Utc>, _>("at")
                    .to_rfc3339_opts(SecondsFormat::Micros, true),
                last.get::<String, _>("key")
            )
        });
        let data: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|r| item(r.get("at"), &r.get::<Value, _>("f")))
            .collect();
        Ok(json!({"data":data,"next_cursor":next}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursors_and_kinds_are_validated() {
        assert!(parse_cursor("2026-11-01T10:00:00.000001Z|interaction:x").is_ok());
        assert!(parse_cursor("nope").is_err());
        assert!(parse_cursor("2026-11-01T10:00:00Z|").is_err());
        assert!(parse_cursor(&format!("2026-11-01T10:00:00Z|a:{}", "x".repeat(300))).is_err());
        assert_eq!(
            sentence("google_destination_checked"),
            "Google destination checked"
        );
    }
    #[test]
    fn audit_detail_keeps_only_listed_settings() {
        let f = json!({"source":"conversion_audit","id":7,"event":"settings_saved","actor":"owner",
            "payload":{"enabled":true,"refresh_token":"secret-token","gclid":"Cj0-click","action":"booked"}});
        let value = item(Utc::now(), &f);
        let text = value.to_string();
        assert!(!text.contains("secret-token") && !text.contains("Cj0-click"));
        assert_eq!(value["detail"]["enabled"], true);
        assert_eq!(value["detail"]["action"], "booked");
        assert_eq!(value["kind"], "api");
        assert_eq!(value["actor"], "owner");
    }
}
