use super::*;
use convert_store::service_policy::ServicePolicy;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adoption {
    adoption_id: Uuid,
    plan: Value,
    policy: ServicePolicy,
}

async fn verify(a: &Shared, store: &Store, plan: &Value) -> Result<(), ApiError> {
    let (connection, token) = signalwire::credentials(a, store).await?;
    let fail = || {
        ApiError(
            StatusCode::CONFLICT,
            "Carrier adoption requires reconciliation".into(),
        )
    };
    if connection.managed
        || plan["project_id"] != connection.project_id.to_string()
        || plan["connection_revision"] != connection.revision
    {
        return Err(fail());
    }
    let inventory = a
        .signalwire_client
        .usage_records(
            &connection.space_host,
            &connection.project_id.to_string(),
            &token,
            "/IncomingPhoneNumbers",
            &[],
        )
        .await?;
    let numbers = plan["numbers"].as_array().ok_or_else(fail)?;
    let base = format!(
        "{}/webhooks/signalwire/workspaces/{}",
        a.public_url.trim_end_matches('/'),
        store.workspace
    );
    if inventory.len() != numbers.len()
        || !numbers.iter().all(|n| {
            inventory.iter().any(|v| {
                v["sid"] == n["sid"]
                    && v["phone_number"] == n["phone"]
                    && v["account_sid"] == plan["project_id"]
                    && v["voice_url"] == format!("{base}/voice")
                    && v["voice_method"] == "POST"
                    && v["status_callback"] == format!("{base}/status")
                    && v["status_callback_method"] == "POST"
                    && v["voice_application_sid"].as_str().unwrap_or("").is_empty()
            })
        })
    {
        return Err(fail());
    }
    Ok(())
}
pub async fn preview(State(a): State<Shared>, Path(workspace): Path<Uuid>) -> Api<Value> {
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    let plan = store.adoption_plan().await?;
    verify(&a, &store, &plan).await?;
    Ok(Json(plan))
}
pub async fn apply(
    State(a): State<Shared>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<Adoption>,
) -> Api<Value> {
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    // Lost-response recovery does not rotate credentials or reset activation.
    if let Some(receipt) = store
        .adoption_receipt(input.adoption_id, &input.plan)
        .await?
    {
        return Ok(Json(receipt));
    }
    verify(&a, &store, &input.plan).await?;
    Ok(Json(
        store
            .adopt_carrier(input.adoption_id, &input.plan, &input.policy)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQuery {
    adoption_id: Uuid,
    plan: Value,
}
pub async fn status(
    State(a): State<Shared>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<ReceiptQuery>,
) -> Api<Value> {
    let store = Store {
        pool: a.store.pool.clone(),
        workspace,
    };
    Ok(Json(
        json!({"receipt":store.adoption_receipt(input.adoption_id,&input.plan).await?}),
    ))
}
