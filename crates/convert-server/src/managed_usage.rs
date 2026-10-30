//! Read-only metering evidence. Customer quantities are rated by Rails, never
//! derived by dividing carrier cost summaries by an assumed wholesale rate.
use super::*;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    project_id: Uuid,
    period_start: i64,
    period_end: i64,
}
fn invalid() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Managed usage requires reconciliation".into(),
    )
}
fn timestamp(value: &Value) -> Result<i64, ApiError> {
    let s = value.as_str().ok_or_else(invalid)?;
    DateTime::parse_from_rfc2822(s)
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .map(|v| v.timestamp())
        .map_err(|_| invalid())
}
fn duration(value: &Value) -> Result<i64, ApiError> {
    let v = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(invalid)?;
    if !(0..=31 * 86400).contains(&v) {
        return Err(invalid());
    }
    Ok(v)
}
pub async fn report(
    State(a): State<Shared>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<Query>,
) -> Api<Value> {
    let observed = Utc::now().timestamp();
    if input
        .period_end
        .checked_sub(input.period_start)
        .is_none_or(|v| !(1..=62 * 86400).contains(&v))
        || input.period_start > observed
        || input.period_start < 0
    {
        return Err(invalid());
    }
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    let (connection, token) = signalwire::credentials(&a, &store).await?;
    if !connection.managed || connection.project_id != input.project_id {
        return Err(invalid());
    }
    let project = connection.project_id.to_string();
    let rows = store.metered_numbers(input.project_id).await?;
    let mut numbers = Vec::new();
    let mut phones: HashMap<String, Vec<(i64, Option<i64>)>> = HashMap::new();
    let mut active = HashMap::new();
    for row in rows {
        let sid = row.sid;
        let phone = row.phone;
        let start = row.started_at;
        let stop = row.ended_at;
        let released = row.released_at;
        // We absorb carrier costs after the irreversible release starts, including
        // uncertain deletion retries. A customer never pays for our failed cleanup.
        if released.is_none() {
            active.insert(sid.clone(), phone.clone());
        }
        phones
            .entry(phone)
            .or_default()
            .push((start.timestamp(), stop.map(|t| t.timestamp())));
        numbers.push(json!({"sid":sid,"request_id":row.request,"started_at":start.timestamp(),"ended_at":stop.map(|t|t.timestamp())}));
    }
    let inventory = a
        .signalwire_client
        .usage_records(
            &connection.space_host,
            &project,
            &token,
            "/IncomingPhoneNumbers",
            &[],
        )
        .await?;
    if inventory.len() != active.len()
        || inventory.iter().any(|n| {
            active.get(n["sid"].as_str().unwrap_or(""))
                != n["phone_number"].as_str().map(|s| s.to_string()).as_ref()
        })
    {
        return Err(invalid());
    }
    let from = DateTime::from_timestamp(input.period_start - 1, 0)
        .ok_or_else(invalid)?
        .to_rfc2822();
    let until = DateTime::from_timestamp(input.period_end.min(observed), 0)
        .ok_or_else(invalid)?
        .to_rfc2822();
    let records = a
        .signalwire_client
        .usage_records(
            &connection.space_host,
            &project,
            &token,
            "/Calls",
            &[("EndTime>", &from), ("EndTime<", &until)],
        )
        .await?;
    let mut calls = Vec::new();
    // Reuse parent records from the complete listing; only boundary-spanning
    // parents need an extra retrieve request (avoid one API call per lead).
    let mut parents: HashMap<String, Value> = records
        .iter()
        .filter(|call| call["direction"] == "inbound")
        .filter_map(|call| {
            call["sid"]
                .as_str()
                .map(|sid| (sid.to_string(), call.clone()))
        })
        .collect();
    for call in records {
        if call["status"] != "completed" {
            continue;
        }
        let end = timestamp(&call["end_time"])?;
        if end < input.period_start || end >= input.period_end || end > observed {
            continue;
        }
        if call["direction"] == "inbound" {
            continue;
        }
        if call["direction"] != "outbound-dial" {
            return Err(invalid());
        }
        let parent = call["parent_call_sid"].as_str().ok_or_else(invalid)?;
        if !parents.contains_key(parent) {
            let root = a
                .signalwire_client
                .usage_call(&connection.space_host, &project, &token, parent)
                .await?;
            parents.insert(parent.to_string(), root);
        }
        let root = &parents[parent];
        let start = timestamp(&root["start_time"])?;
        let phone = root["to"].as_str().ok_or_else(invalid)?;
        if root["direction"] != "inbound"
            || !phones.get(phone).is_some_and(|periods| {
                periods
                    .iter()
                    .any(|(a, b)| *a <= start && b.is_none_or(|b| start < b))
            })
        {
            return Err(invalid());
        }
        let seconds = duration(&call["duration"])?;
        if seconds > end - start || seconds < 0 {
            return Err(invalid());
        }
        calls.push(json!({"sid":call["sid"],"parent_sid":parent,"ended_at":end,"connected_seconds":seconds}));
    }
    // Reject concurrent purchase/release changes rather than seal a torn snapshot.
    let changed = store.metering_changed_since(observed).await?;
    let current = store.signalwire_connection().await?.ok_or_else(invalid)?;
    if changed
        || !current.managed
        || current.project_id != connection.project_id
        || current.space_host != connection.space_host
        || current.revision != connection.revision
    {
        return Err(invalid());
    }
    Ok(Json(
        json!({"version":1,"workspace_id":workspace,"project_id":project,"period_start":input.period_start,"period_end":input.period_end,
        "observed_at":observed,"complete":true,"numbers":numbers,"calls":calls}),
    ))
}
