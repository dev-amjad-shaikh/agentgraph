//! HTTP error type: a status code plus a JSON `{error, message}` body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// An API error rendered as `{ "error": <kind>, "message": <detail> }` with
/// the matching HTTP status.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    body: Value,
}

impl ApiError {
    pub fn new(status: StatusCode, kind: &str, message: String) -> Self {
        Self {
            status,
            body: json!({ "error": kind, "message": message }),
        }
    }

    /// 404 — unknown thread, run, or other resource.
    pub fn not_found(message: String) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    /// 409 — multitask `reject` hit an active run, queue full, duplicate id.
    pub fn conflict(message: String) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    /// 400 — malformed payload, unknown graph, bad strategy, non-object input.
    pub fn bad_request(message: String) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    /// 422 — the request is well-formed but cannot be processed: replaying a
    /// run whose graph is not registered in this process, or whose journal
    /// carries evidence server-side replay cannot re-drive.
    pub fn unprocessable(message: String) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", message)
    }

    /// 500 — checkpointer IO failures and other internal errors.
    pub fn internal(message: String) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.body)
    }
}

impl std::error::Error for ApiError {}
