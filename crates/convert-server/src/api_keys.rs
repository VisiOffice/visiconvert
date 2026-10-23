use super::*;
use convert_store::api_keys::KeyInput;
use serde::Deserialize;
pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api-keys", get(list).post(create))
        .route("/api-keys/{id}/rotate", post(rotate))
        .route("/api-keys/{id}/revoke", post(revoke))
}
async fn owner(access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash
        && !access.store.manages_connection(hash).await?
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the workspace owner can manage API keys".into(),
        ));
    }
    Ok(())
}
async fn entitlement(app: &App, access: &hosted::Access) -> Result<(), ApiError> {
    if let Some(hash) = &access.session_hash {
        let (user, version) = access.store.api_identity(hash).await?;
        super::public_api::hosted_access(app, access.store.workspace, user, version).await?;
    }
    Ok(())
}
async fn list(Extension(access): Extension<hosted::Access>) -> Api<Value> {
    owner(&access).await?;
    Ok(Json(access.store.api_keys().await?))
}
async fn create(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Json(input): Json<KeyInput>,
) -> Api<Value> {
    owner(&access).await?;
    entitlement(&app, &access).await?;
    Ok(Json(
        access
            .store
            .issue_api_key(&input, access.session_hash.as_deref(), None)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotation {
    version: i32,
    key: KeyInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    version: i32,
}
async fn rotate(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
    Json(input): Json<Rotation>,
) -> Api<Value> {
    owner(&access).await?;
    entitlement(&app, &access).await?;
    Ok(Json(
        access
            .store
            .issue_api_key(
                &input.key,
                access.session_hash.as_deref(),
                Some((id, input.version)),
            )
            .await?,
    ))
}
async fn revoke(
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
        .revoke_api_key(id, input.version, &actor)
        .await?;
    Ok(Json(json!({"ok":true})))
}
