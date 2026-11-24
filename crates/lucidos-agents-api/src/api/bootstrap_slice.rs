//! The slice types of the *bootstrap snapshot*: what a slice reader is given,
//! what it answers, and the reader itself (`docs/glossary.md`).

use std::future::Future;
use std::pin::Pin;

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::value::RawValue;

use super::AppState;

/// What every slice reader is given: the engine state and the request headers.
pub(crate) struct SliceContext {
    pub(crate) state: AppState,
    pub(crate) headers: HeaderMap,
}

impl SliceContext {
    /// The calling device, from `x-lucidos-device-id`. A per-device slice
    /// reads for it, where its own route takes a `device_id` query.
    pub(crate) fn device_id(&self) -> Option<&str> {
        super::actor::authenticated_device_id(None, &self.headers)
    }
}

/// One slice: the body its own route answers, or the error that route sends.
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
    /// The slice a route's read function answers. A refusal is turned into
    /// the response the route sends, so its code and message come from there.
    pub(crate) async fn of<Body: Serialize, Refusal: IntoResponse>(
        read: Result<Body, Refusal>,
    ) -> Self {
        match read {
            Ok(body) => Self::data(&body),
            Err(refusal) => Self::error(refusal.into_response()).await,
        }
    }

    /// The slice of a read that cannot fail. `Json` serializes a route's body
    /// the same way, so the bytes match. It also answers 500 with the error's
    /// text when a body does not serialize.
    pub(crate) fn data<Body: Serialize>(body: &Body) -> Self {
        match serde_json::value::to_raw_value(body) {
            Ok(raw) => Self::Data(raw),
            Err(error) => Self::Error(SliceError {
                message: error.to_string(),
                http_code: axum::http::StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            }),
        }
    }

    /// The slice of a reader that panicked: a 500. Its narrow route would
    /// drop the connection instead.
    pub(crate) fn reader_panicked() -> Self {
        let status = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        Self::Error(SliceError {
            message: status.canonical_reason().unwrap_or("Error").to_owned(),
            http_code: status.as_u16(),
        })
    }

    /// The message is the body's `error` string, else its whole trimmed
    /// text, else the status phrase.
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

/// One row of the slice table: a slice key and the read that answers it. The
/// route knows nothing else about a slice.
pub(crate) struct SliceReader {
    pub(crate) key: &'static str,
    pub(crate) read: for<'a> fn(&'a SliceContext) -> SliceRead<'a>,
}
