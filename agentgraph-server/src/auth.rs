//! API-key authentication middleware (`X-Api-Key` header).

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::routes::AppState;

/// When `ServerConfig.api_key` is set, every request must carry a matching
/// `X-Api-Key` header; otherwise the request passes through (dev mode).
pub(crate) async fn require_api_key(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.config.api_key {
        let provided = request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        if provided != Some(expected.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "unauthorized",
                    "message": "a valid `X-Api-Key` header is required",
                })),
            )
                .into_response();
        }
    }
    next.run(request).await
}
