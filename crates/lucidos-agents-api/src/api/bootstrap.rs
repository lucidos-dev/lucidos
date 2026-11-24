//! `GET /api/v1/bootstrap`: the *bootstrap snapshot*, the workspace-wide state
//! a shell draws outside any one thread, in one read (`docs/glossary.md`).
//!
//! Each slice is the body of one narrow route, read through that route's own
//! read function. The route below calls no handler module: only the table
//! rows do.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::panic::AssertUnwindSafe;

use futures::{FutureExt, StreamExt};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};

use super::bootstrap_slice::{SliceContext, SliceReader, SliceResult};
use super::{
    apps, backup, changes, history, intelligence_ladder, memory, notifications, plugins,
    release_notices, repositories, settings, spend_cap_holds, thread_queue, threads,
    trigger_groups, triggers, users, webhooks,
};
use super::{AppState, ChangesListQuery, NotificationsListQuery};
use crate::core::subprocess_origin::HEADER_DEVICE_ID;
use crate::runtime::intelligence_ladder::LadderSurface;

/// How many slice reads one snapshot runs at once. It bounds slices, not
/// connections: one read can hold several, as `changes::read_changes` does.
pub(crate) const BOOTSTRAP_READ_CONCURRENCY: usize = 8;

const CACHE_CONTROL: &str = "private, no-cache";

macro_rules! slice {
    ($key:literal, |$context:ident| $read:expr) => {
        SliceReader {
            key: $key,
            read: |$context| Box::pin(async move { $read }),
        }
    };
}

/// The slices whose handlers stay in this crate when the threads API leaves
/// it. Each row names the fixed request its slice stands for.
const API_SLICES: &[SliceReader] = &[
    slice!("preferences", |context| SliceResult::of(
        settings::read_preferences(&context.state, context.device_id()).await
    )
    .await),
    slice!("unread_notifications", |context| {
        let unread_page = NotificationsListQuery {
            limit: notifications::NOTIFICATIONS_PAGE_MAX,
            before: None,
            filter: Some("unread".to_owned()),
        };
        SliceResult::of(
            notifications::read_notifications(&context.state, &context.headers, &unread_page).await,
        )
        .await
    }),
    slice!("reader_devices", |context| SliceResult::data(
        &users::reader_devices(&context.headers)
    )),
    slice!("users", |context| SliceResult::data(&users::user_list(
        &context.headers
    ))),
    slice!("pinned_apps", |context| {
        match settings::pinned_apps_query(context.device_id()) {
            Ok(query) => {
                SliceResult::of(settings::read_pinned_apps(&context.state, &query).await).await
            }
            Err(rejection) => SliceResult::of(Err::<(), _>(rejection)).await,
        }
    }),
    slice!("apps", |context| SliceResult::of(apps::read_apps(
        &context.state
    ))
    .await),
    slice!("chat_models", |context| SliceResult::of(
        settings::read_models(&context.state).await
    )
    .await),
    slice!("intelligence_ladder", |context| SliceResult::data(
        &intelligence_ladder::read_ladder(&context.state, LadderSurface::Chat, None).await
    )),
    slice!("spend_cap_holds", |context| SliceResult::data(
        &spend_cap_holds::read_holds(&context.state).await
    )),
    slice!("last_successful_backup", |context| SliceResult::of(
        backup::read_last_successful_backup(&context.state).await
    )
    .await),
    slice!("release_notices", |context| SliceResult::of(
        release_notices::read_notices(&context.state).await
    )
    .await),
    slice!("embedding_model_status", |context| SliceResult::data(
        &memory::read_embedding_model_status(&context.state)
    )),
    slice!("webhook_ingress", |context| SliceResult::of(
        webhooks::read_ingress_status(&context.state).await
    )
    .await),
    slice!("webhook_refusals", |context| SliceResult::of(
        webhooks::read_refusal_status(&context.state).await
    )
    .await),
    slice!("installed_plugins", |context| SliceResult::of(
        plugins::read_installed(&context.state).await
    )
    .await),
    slice!("plugin_catalog", |context| SliceResult::of(
        plugins::read_catalog(&context.state).await
    )
    .await),
    slice!("triggers", |context| SliceResult::data(
        &triggers::read_triggers(&context.state).await
    )),
    slice!("historical_triggers", |context| SliceResult::of(
        triggers::read_historical_triggers(&context.state).await
    )
    .await),
    slice!("trigger_groups", |context| SliceResult::data(
        &trigger_groups::read_trigger_groups(&context.state)
    )),
    slice!("thread_queue", |context| SliceResult::of(
        thread_queue::read_thread_queue(&context.state).await
    )
    .await),
    slice!("repositories", |context| SliceResult::of(
        repositories::read_repositories(&context.state).await
    )
    .await),
    slice!("devices", |context| SliceResult::of(
        settings::read_devices(&context.state).await
    )
    .await),
];

/// The slices whose handlers move to the threads API crate. Kept apart so the
/// split lifts these rows out without touching the route.
const THREADS_API_SLICES: &[SliceReader] = &[
    slice!("filter_facets", |context| SliceResult::of(
        threads::read_filter_facets(&context.state).await
    )
    .await),
    slice!("changes_state", |context| {
        let first_page = ChangesListQuery {
            limit: Some(changes::CHANGES_DEFAULT_LIMIT),
            before: None,
            sub_threads_of: None,
        };
        SliceResult::of(changes::read_changes(&context.state, &first_page).await).await
    }),
    slice!("engine_version_status", |context| SliceResult::data(
        &history::read_engine_version_status(&context.state).await
    )),
];

fn slice_readers() -> impl Iterator<Item = &'static SliceReader> {
    API_SLICES.iter().chain(THREADS_API_SLICES)
}

/// Every slice of one read, in table order.
struct Snapshot(Vec<(&'static str, SliceResult)>);

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, slice) in &self.0 {
            map.serialize_entry(key, slice)?;
        }
        map.end()
    }
}

/// Read every slice `readers` names. A reader that fails or panics answers
/// its own error slice, so every key is present whatever the readers do.
async fn read_snapshot(
    context: &SliceContext,
    readers: impl Iterator<Item = &'static SliceReader>,
) -> Snapshot {
    // Queued, not started: `buffered` polls at most the bound at a time.
    let mut reads = Vec::new();
    for reader in readers {
        reads.push(read_slice(context, reader).boxed());
    }
    let slices = futures::stream::iter(reads)
        .buffered(BOOTSTRAP_READ_CONCURRENCY)
        .collect()
        .await;
    Snapshot(slices)
}

async fn read_slice(
    context: &SliceContext,
    reader: &'static SliceReader,
) -> (&'static str, SliceResult) {
    let slice = AssertUnwindSafe(async { (reader.read)(context).await })
        .catch_unwind()
        .await
        // The panic hook has already logged the panic and where it rose.
        .unwrap_or_else(|_| SliceResult::reader_panicked());
    (reader.key, slice)
}

async fn bootstrap(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let context = SliceContext { state, headers };
    let snapshot = read_snapshot(&context, slice_readers()).await;
    match serde_json::to_vec(&snapshot) {
        Ok(body) => snapshot_response(body, &context.headers),
        Err(error) => super::error::ApiError::internal(error.to_string()).into_response(),
    }
}

fn snapshot_response(body: Vec<u8>, request_headers: &HeaderMap) -> Response {
    let opaque_tag = format!("\"{:x}\"", Sha256::digest(&body));
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    headers.insert(header::VARY, HeaderValue::from_static(HEADER_DEVICE_ID));
    headers.append(header::VARY, header::ACCEPT_ENCODING.into());
    if let Ok(etag) = HeaderValue::from_str(&format!("W/{opaque_tag}")) {
        headers.insert(header::ETAG, etag);
    }
    // The field may arrive as several lines, which form one list.
    let held = request_headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| super::file_response::if_none_match_hits(value, &opaque_tag));
    if held {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (headers, body).into_response()
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/bootstrap", get(bootstrap))
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
