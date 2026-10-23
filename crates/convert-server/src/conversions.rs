use super::*;
use convert_store::conversions::{Consent, Revision, Settings, StageInput};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/conversions", get(preview))
        .route("/conversions/settings", axum::routing::put(settings))
        .route(
            "/leads/{id}/conversion-consent",
            axum::routing::put(consent),
        )
        .route("/conversions/stage", post(stage))
        .route("/conversions/{id}/cancel", post(cancel))
}
async fn owner(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash
        && !access.store.manages_connection(hash).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can prepare advertising conversions".into(),
        ));
    }
    Ok(())
}
async fn preview(Extension(access): Extension<hosted::Access>) -> Api<Value> {
    owner(&access).await?;
    Ok(Json(access.store.conversion_preview().await?))
}
async fn settings(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<Settings>,
) -> Api<Value> {
    owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access
        .store
        .save_conversion_settings(&input, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn consent(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<Consent>,
) -> Api<Value> {
    owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access
        .store
        .save_conversion_consent(id, &input, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn stage(
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<StageInput>,
) -> Api<Value> {
    owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    Ok(Json(access.store.stage_conversion(&input, &actor).await?))
}
async fn cancel(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<Revision>,
) -> Api<Value> {
    owner(&access).await?;
    let actor = access
        .store
        .inquiry_actor(access.session_hash.as_deref())
        .await?;
    access
        .store
        .cancel_conversion(id, input.version, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
