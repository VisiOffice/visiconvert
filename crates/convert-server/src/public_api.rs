use super::*;
use axum::extract::{MatchedPath, Query};
use convert_store::{api_keys::KeyAccess, public_api::Page};
use std::time::Duration;

pub fn routes(app: Shared) -> Router<Shared> {
    Router::new()
        .route("/workspace", get(workspace))
        .route("/setup", get(sites::setup))
        .route("/sites", get(sites::list).post(sites::save_new))
        .route("/sites/{id}", axum::routing::patch(sites::save_existing))
        .route(
            "/sites/{id}/number-pool",
            get(sites::pool).put(sites::save_pool),
        )
        .route("/leads", get(interactions).post(add_lead))
        .route("/leads/{id}", get(interaction).patch(review_interaction))
        .route("/leads/{id}/outcomes", get(outcomes).put(booking))
        .route("/interactions", get(interactions).post(add_lead))
        .route(
            "/interactions/{id}",
            get(interaction).patch(review_interaction),
        )
        .route("/interactions/{id}/outcomes", get(outcomes).put(booking))
        .route("/inquiries", get(inquiries))
        .route("/inquiries/{id}", get(inquiry).patch(review_inquiry))
        .route(
            "/inquiries/{id}/outcomes",
            get(inquiry_outcomes).put(inquiry_booking),
        )
        .route("/bookings", get(bookings))
        .route("/reports/sources", get(summary))
        .route("/numbers", get(numbers).post(register_number))
        .merge(managed::routes())
        .route("/connections/signalwire", get(signalwire::show))
        .route(
            "/connections/signalwire/numbers",
            get(signalwire::inventory),
        )
        .route("/connections/twilio", get(connections::show))
        .route("/connections/twilio/numbers", get(connections::inventory))
        .route_layer(middleware::from_fn_with_state(app, auth))
        .route(
            "/openapi.json",
            get(|| async {
                Json(
                    serde_json::from_str::<Value>(include_str!("../assets/openapi.json"))
                        .expect("validated OpenAPI"),
                )
            }),
        )
        .fallback(|| async {
            ApiError(StatusCode::NOT_FOUND, "Unknown public API endpoint".into())
        })
        .layer(middleware::from_fn(errors))
}
async fn errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status().is_client_error()
        && !response
            .headers()
            .get("content-type")
            .is_some_and(|v| v.to_str().unwrap_or("").starts_with("application/json"))
    {
        return ApiError(
            response.status(),
            "Invalid request: check method, JSON fields, UUIDs and query parameters".into(),
        )
        .into_response();
    }
    response
}
/// Authoritative billing/account check on every hosted key request; no cached
/// grace period, no fallback to allowing access if Rails is unavailable.
pub async fn hosted_access(
    app: &App,
    workspace: Uuid,
    user: Uuid,
    version: i64,
) -> Result<(), ApiError> {
    hosted_access_with_mcp(app, workspace, user, version, None).await
}
pub async fn hosted_access_with_mcp(
    app: &App,
    workspace: Uuid,
    user: Uuid,
    version: i64,
    mcp: Option<(Uuid, &[String])>,
) -> Result<(), ApiError> {
    let mut body = json!({"workspace_id":workspace,"user_id":user,"credential_version":version});
    if let Some((grant, scopes)) = mcp {
        body["mcp_grant_id"] = json!(grant);
        body["mcp_scopes"] = json!(scopes);
    }
    let config = app.cloud.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Hosted access checks unavailable".into(),
    ))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Hosted access checks unavailable".into(),
            )
        })?;
    let response = client
        .post(format!("{}/internal/api-access", config.cloud_origin))
        .bearer_auth(&config.service_key)
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Hosted access could not be verified; retry later".into(),
            )
        })?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Hosted workspace access is disabled or requires active billing".into(),
        ));
    }
    if !response.status().is_success() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Hosted access could not be verified; retry later".into(),
        ));
    }
    let value: Value = response.json().await.map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Invalid hosted access response".into(),
        )
    })?;
    if value["allowed"] != true
        || value["workspace_id"] != workspace.to_string()
        || value["user_id"] != user.to_string()
        || value["credential_version"] != version
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Hosted workspace access unavailable".into(),
        ));
    }
    Ok(())
}
async fn auth(State(app): State<Shared>, mut request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    if token.starts_with("vc_session_") {
        return ApiError(
            StatusCode::UNAUTHORIZED,
            "Create a workspace API key for the public API".into(),
        )
        .into_response();
    }
    if !token.starts_with("vc_key_") && !token.starts_with("vc_mcp_") {
        return authenticate(State(app), request, next).await;
    }
    let key_result = if token.starts_with("vc_mcp_") {
        match super::remote_mcp::authenticate(&app, &token).await {
            Ok(key) => Ok(Some(key)),
            Err(e) => return e.into_response(),
        }
    } else {
        app.store.authenticate_api_key(&token).await
    };
    let key = match key_result {
        Ok(Some(key)) => key,
        Ok(None) => {
            return ApiError(
                StatusCode::UNAUTHORIZED,
                "API key is invalid, expired or revoked".into(),
            )
            .into_response();
        }
        Err(convert_store::Error::RateLimited) => {
            let mut r = ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "API key limit exceeded (120 requests per minute)".into(),
            )
            .into_response();
            r.headers_mut().insert("retry-after", "60".parse().unwrap());
            return r;
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str())
        .unwrap_or("");
    let scope = if request.method() == axum::http::Method::GET
        || request.method() == axum::http::Method::HEAD
    {
        "read"
    } else if path == "/api/v1/sites" || path.starts_with("/api/v1/sites/") {
        "setup:write"
    } else if path.ends_with("/outcomes") {
        "bookings:write"
    } else if path.ends_with("/numbers") {
        "numbers:write"
    } else if path.starts_with("/api/v1/numbers/") {
        "numbers:purchase"
    } else {
        "leads:write"
    };
    if !key.scopes.iter().any(|s| s == scope) {
        return ApiError(
            StatusCode::FORBIDDEN,
            format!("This API key requires {scope} permission"),
        )
        .into_response();
    }
    if let Some(user) = key.owner
        && let Err(e) = hosted_access(&app, key.store.workspace, user, key.credential_version).await
    {
        return e.into_response();
    }
    request.extensions_mut().insert(hosted::Access {
        store: key.store.clone(),
        name: key.name.clone(),
        session_hash: None,
    });
    request.extensions_mut().insert(key);
    next.run(request).await
}
async fn workspace(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
) -> Api<Value> {
    Ok(Json(
        json!({"id":access.store.workspace,"name":access.name,"currency":"USD","hosted":key.as_ref().is_some_and(|k|k.owner.is_some()) || access.session_hash.is_some(),"scopes":key.map(|k|k.scopes.clone())}),
    ))
}
async fn interactions(
    Extension(access): Extension<hosted::Access>,
    Query(page): Query<Page>,
) -> Api<Value> {
    Ok(Json(access.store.public_leads(&page, false, None).await?))
}
async fn inquiries(
    Extension(access): Extension<hosted::Access>,
    Query(page): Query<Page>,
) -> Api<Value> {
    Ok(Json(access.store.public_leads(&page, true, None).await?))
}
async fn interaction(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    let value = access
        .store
        .public_leads(&Page::default(), false, Some(id))
        .await?;
    let lead = value["data"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "Interaction not found".into(),
        ))?;
    Ok(Json(lead))
}
async fn inquiry(Extension(access): Extension<hosted::Access>, Path(id): Path<Uuid>) -> Api<Value> {
    Ok(Json(access.store.inquiry_detail(id).await?))
}
async fn bookings(
    Extension(access): Extension<hosted::Access>,
    Query(page): Query<Page>,
) -> Api<Value> {
    Ok(Json(access.store.public_bookings(&page).await?))
}
fn actor(key: Option<Extension<KeyAccess>>) -> String {
    key.map_or("authenticated public API".into(), |k| k.actor.clone())
}
async fn review_interaction(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(input): Json<LeadUpdate>,
) -> Api<Value> {
    access
        .store
        .update_public_lead(id, &input, &actor(key))
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn review_inquiry(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(input): Json<LeadUpdate>,
) -> Api<Value> {
    access.store.update_inquiry(id, &input, &actor(key)).await?;
    Ok(Json(json!({"ok":true})))
}
async fn booking(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(input): Json<OutcomeInput>,
) -> Api<Value> {
    access
        .store
        .put_public_outcome(id, &input, &actor(key))
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn inquiry_booking(
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<Uuid>,
    Json(input): Json<OutcomeInput>,
) -> Api<Value> {
    access
        .store
        .put_inquiry_outcome(id, &input, &actor(key))
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn inquiry_outcomes(
    Extension(access): Extension<hosted::Access>,
    Path(id): Path<Uuid>,
) -> Api<Value> {
    Ok(Json(access.store.inquiry_outcomes(id).await?))
}
async fn register_number(
    State(app): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<NewNumber>,
) -> Api<Value> {
    if key.is_some() && access.store.twilio_connection().await?.is_none() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Connect Twilio in the workspace UI before registering numbers with an API key".into(),
        ));
    }
    connections::register(&app, &access, &input).await?;
    Ok(Json(json!({"ok":true})))
}
