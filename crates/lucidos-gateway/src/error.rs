//! Minimal HTTP error response shape for the gateway's control API.
//!
//! The engine has a richer `api::error::ApiError`; the gateway duplicates a tiny
//! version (ADR 0014 §1 — no engine dependency). It is the response shape
//! (`status` + `{"error": msg}` body) consumed structurally by its
//! `IntoResponse` impl, not a domain error type.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    /// 409 — used by the restore flow when the derived/requested workspace name
    /// collides with an existing one (the picker then asks for a different name)
    /// or when a restore is already in progress.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
