use super::*;
use convert_store::service_policy::ServicePolicy;

pub async fn apply(
    State(app): State<Shared>,
    Path(workspace): Path<Uuid>,
    Json(policy): Json<ServicePolicy>,
) -> Api<Value> {
    let store = Store {
        pool: app.store.pool.clone(),
        workspace,
    };
    store.apply_service_policy(&policy).await?;
    Ok(Json(
        json!({"workspace_id":workspace,"project_id":policy.project_id,"revision":policy.revision}),
    ))
}
