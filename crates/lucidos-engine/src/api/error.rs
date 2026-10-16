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

#[derive(Debug)]
pub(crate) struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// A machine-readable slug beside the message, for a refusal the client
    /// must tell apart from its siblings. The body carries it only when set,
    /// so every other error keeps its exact bytes.
    pub reason: Option<&'static str>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            reason: None,
        }
    }

    /// Attach the machine-readable `reason` slug.
    pub(crate) fn with_reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }

    /// A status given as a bare number, the way an engine error type declares
    /// it beside its taxonomy. A number that is no HTTP status becomes a 500.
    pub(crate) fn with_code(code: u16, message: impl Into<String>) -> Self {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::new(status, message)
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

/// A bare status with no message of its own, carrying the status' canonical
/// reason phrase as the body text.
///
/// This exists so a handler that already returns `StatusCode` errors can adopt
/// `ApiError` for the ONE branch that has something to say, without rewriting
/// the other dozen. It is not a licence to keep returning bare statuses: the
/// canonical phrase ("Conflict", "Bad Request") names the class of failure, not
/// the failure, so any branch a user can reach owes a real message.
///
/// The phrase matters even so. A bare `StatusCode` sends an EMPTY body, and the
/// frontend's `throwIfNotOk` then falls back to `res.statusText`, which is
/// always `""` over HTTP/2, so the toast reads `Failed to send message: 409` and
/// tells the user nothing at all. `Conflict` is at least a word.
impl From<StatusCode> for ApiError {
    fn from(status: StatusCode) -> Self {
        Self::new(status, status.canonical_reason().unwrap_or("Error"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = match self.reason {
            Some(reason) => serde_json::json!({ "error": self.message, "reason": reason }),
            None => serde_json::json!({ "error": self.message }),
        };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_of(e: ApiError) -> serde_json::Value {
        let bytes = axum::body::to_bytes(e.into_response().into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// A reason slug rides beside the message only when a handler sets one.
    /// Every other error body keeps its exact bytes.
    #[tokio::test]
    async fn a_reason_is_added_only_when_set() {
        assert_eq!(
            body_of(ApiError::new(StatusCode::CONFLICT, "busy")).await,
            serde_json::json!({ "error": "busy" })
        );
        assert_eq!(
            body_of(ApiError::new(StatusCode::CONFLICT, "busy").with_reason("apply_in_progress"))
                .await,
            serde_json::json!({ "error": "busy", "reason": "apply_in_progress" })
        );
    }

    #[test]
    fn bare_status_carries_its_canonical_reason() {
        let err: ApiError = StatusCode::CONFLICT.into();
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.message, "Conflict");
    }

    #[test]
    fn an_explicit_message_is_never_replaced_by_the_canonical_one() {
        let err = ApiError::new(
            StatusCode::CONFLICT,
            "Thread is locked to coding-agent mode",
        );
        assert_eq!(err.message, "Thread is locked to coding-agent mode");
    }
}
