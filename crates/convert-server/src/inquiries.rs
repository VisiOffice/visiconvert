use super::*;
use convert_store::inquiries::{CombineInput, Inquiry, SplitInput};
pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/inquiries", get(list))
        .route("/inquiries/{id}", get(show).patch(update))
        .route("/inquiries/{id}/combine", post(combine))
        .route("/inquiries/{id}/history", get(history))
        .route("/inquiries/{id}/split", post(split))
        .route("/inquiries/{id}/outcomes", get(outcomes).put(booking))
}
async fn list(Extension(access): Extension<hosted::Access>) -> Api<Vec<Inquiry>> {
    Ok(Json(access.store.inquiries().await?))
}
async fn show(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    Ok(Json(access.store.inquiry_detail(id).await?))
}
async fn update(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<LeadUpdate>,
) -> Api<Value> {
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access.store.update_inquiry(id, &input, &actor).await?;
    Ok(Json(json!({"ok":true})))
}
async fn combine(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<CombineInput>,
) -> Api<Value> {
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    Ok(Json(
        json!({"id":access.store.combine_inquiries(id,&input,&actor).await?}),
    ))
}
async fn split(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<SplitInput>,
) -> Api<Value> {
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    Ok(Json(
        json!({"id":access.store.split_inquiry(id,&input,&actor).await?}),
    ))
}
async fn outcomes(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    Ok(Json(access.store.inquiry_outcomes(id).await?))
}
async fn booking(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<OutcomeInput>,
) -> Api<Value> {
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access.store.put_inquiry_outcome(id, &input, &actor).await?;
    Ok(Json(json!({"ok":true})))
}

async fn history(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Vec<Value>> {
    Ok(Json(access.store.inquiry_history(id).await?))
}
