//! Single error-response shape for `/api/v1` handlers that return JSON
//! errors: `{ "error": "<message>" }` with an appropriate HTTP status.
//!
//! Coverage is the converted modules, not (yet) the whole API surface: some
//! handlers still hand-roll the same JSON tuple inline, and others
//! deliberately return plain-text `(StatusCode, String)` errors — converting
//! those would change response bytes, so they migrate opportunistically when
//! their handlers are next touched.
//!
//! Before this type existed, several modules each rebuilt the same
//! `(StatusCode, Json(json!({ "error": … })))` tuple under a private helper
//! name (`bad_request` in notifications, `json_error` in backup,
//! `internal_err` in changes / blobs / threads_compose). `ApiError` replaces
//! those per-file helpers: handlers return `Result<_, ApiError>` and use `?`,
//! and the `IntoResponse` impl produces the exact same wire bytes the helpers
//! did.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub(crate) struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// 400 Bad Request.
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// 500 Internal Server Error.
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// 404 Not Found.
    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// 500 with the `DB error: <e>` message shape used by DB-backed handlers.
    pub(crate) fn db(e: sqlx::Error) -> Self {
        Self::internal(format!("DB error: {e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
