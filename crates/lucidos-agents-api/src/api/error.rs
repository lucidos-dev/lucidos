use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug)]
pub(crate) struct ApiError {
    pub status: StatusCode,
    pub message: String,
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

    pub(crate) fn with_reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }

    pub(crate) fn with_code(code: u16, message: impl Into<String>) -> Self {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::new(status, message)
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub(crate) fn db(e: sqlx::Error) -> Self {
        Self::internal(format!("DB error: {e}"))
    }
}

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

    #[tokio::test]
    async fn a_bare_status_carries_its_canonical_phrase_as_the_message() {
        assert_eq!(
            body_of(ApiError::from(StatusCode::CONFLICT)).await,
            serde_json::json!({ "error": "Conflict" })
        );
    }

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
