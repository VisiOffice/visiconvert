//! Read-only activity log shared by the workspace UI (`/api/activity`) and the public API
//! (`/api/v1/activity`, `read` scope like the other lists).
use super::*;
use axum::extract::Query;
use convert_store::activity::ActivityQuery;

pub fn routes() -> Router<Shared> {
    Router::new().route("/activity", get(list))
}
async fn list(
    Extension(access): Extension<hosted::Access>,
    Query(query): Query<ActivityQuery>,
) -> Api<Value> {
    Ok(Json(access.store.activity(&query).await?))
}
