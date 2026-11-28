use std::future::Future;
use std::pin::Pin;

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::value::RawValue;

use super::AppState;

pub(crate) struct SliceContext {
    pub(crate) state: AppState,
    pub(crate) headers: HeaderMap,
}

impl SliceContext {
    pub(crate) fn device_id(&self) -> Option<&str> {
        super::actor::authenticated_device_id(None, &self.headers)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SliceResult {
    Data(Box<RawValue>),
    Error(SliceError),
}

#[derive(Debug, Serialize)]
pub(crate) struct SliceError {
    message: String,
    http_code: u16,
}

impl SliceResult {
    pub(crate) async fn of<Body: Serialize, Refusal: IntoResponse>(
        read: Result<Body, Refusal>,
    ) -> Self {
        match read {
            Ok(body) => Self::data(&body),
            Err(refusal) => Self::error(refusal.into_response()).await,
        }
    }

    pub(crate) fn data<Body: Serialize>(body: &Body) -> Self {
        match serde_json::value::to_raw_value(body) {
            Ok(raw) => Self::Data(raw),
            Err(error) => Self::Error(SliceError {
                message: error.to_string(),
                http_code: axum::http::StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            }),
        }
    }

    pub(crate) fn reader_panicked() -> Self {
        let status = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        Self::Error(SliceError {
            message: status.canonical_reason().unwrap_or("Error").to_owned(),
            http_code: status.as_u16(),
        })
    }

    async fn error(response: Response) -> Self {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        let message = match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(json) => json
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            Err(_) => Some(String::from_utf8_lossy(&body).trim().to_owned()),
        }
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("Error").to_owned());
        Self::Error(SliceError {
            message,
            http_code: status.as_u16(),
        })
    }
}

pub(crate) type SliceRead<'a> = Pin<Box<dyn Future<Output = SliceResult> + Send + 'a>>;

pub(crate) struct SliceReader {
    pub(crate) key: &'static str,
    pub(crate) read: for<'a> fn(&'a SliceContext) -> SliceRead<'a>,
}
