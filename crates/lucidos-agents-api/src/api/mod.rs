pub mod actor;
pub(crate) mod app_reach;
mod app_ui;
mod apps;
mod artifacts;
mod background_models;
pub(crate) mod backup;
mod blobs;
mod bootstrap;
mod bootstrap_slice;
pub(crate) mod browser_origin;
pub(crate) mod changes;
pub(crate) mod chat;
mod chatgpt_plan;
mod claude_code;
mod claude_gateway;
mod command_checkpoint;
mod command_permission;
pub(crate) mod credential_reveal;
mod data_api;
mod diagnostic_bundles;
mod diagnostic_snapshot;
pub(crate) mod diff;
mod disk_usage;
pub mod engine_socket;
pub use gateway_signals::{announce_renamed_devices, announce_users_if_changed};
pub(crate) mod error;
mod file_response;
mod fonts;
mod form_requests;
mod frame_capability;
mod frame_file_cors;
mod frontend_preview;
use crate::engine::frontend_snapshot;
mod gateway_signals;
pub mod gateway_stamp;
pub(crate) mod handshake_scripts;
mod history;
pub(crate) mod hooks;
mod intelligence_ladder;
mod internal_faults;
mod snapshots;
#[cfg(feature = "test-support")]
pub(crate) use snapshots::assert_restore_slot_busy;
mod config_changes;
mod images;
pub(crate) mod internal;
mod knowhow;
pub(crate) mod local_auth;
mod main_check;
mod mcp;
mod mcp_permission;
mod memory;
mod model_prices;
pub(crate) mod mutating_gate;
mod notifications;
mod onepassword;
mod recall;
mod restore_generation;
#[cfg(test)]
pub(crate) mod route_scan;
mod side_questions;
mod spend_cap_holds;
mod themes;
mod users;

mod gateway_requests;
mod host_log_transfer;
pub(crate) mod owner_door;
pub(crate) mod permission_requests;
mod permissions;
mod plugin_media;
mod plugins;
mod presence;
pub mod presence_pong;
pub use crate::engine::sse_connections;
pub(crate) mod proxy;
pub(crate) mod proxy_builtin;
pub(crate) mod proxy_cost;
pub(crate) mod proxy_hmac_layer;
pub mod proxy_migration;
pub(crate) mod proxy_pipeline;
#[doc(hidden)]
pub mod wasm_test_internals {
    pub use crate::api::proxy_pipeline_builder::{build_pipeline, PipelineBuildContext};
    pub use crate::api::proxy_pipeline_config::{
        CredentialHandleConfig, LayerConfig, PipelineConfig, StaticKind,
    };
}
pub(crate) mod proxy_pipeline_builder;
pub(crate) mod proxy_pipeline_config;
pub(crate) mod proxy_redaction;
pub(crate) mod proxy_script_layer;
pub(crate) mod proxy_script_runner;
pub(crate) mod proxy_speech;
pub(crate) mod proxy_static_layers;
pub(crate) mod proxy_timeout;
mod release_notices;
mod repositories;
mod sdk;
mod sdk_prefs;
mod search;
pub(crate) mod secret_reveal;
mod settings;
pub(crate) mod socket_ticket;
pub(crate) mod target_workspace;
mod terminal;
mod terminal_socket;
mod thread_queue;
pub(crate) mod thread_reach;
mod thread_users;
pub(crate) mod threads;
mod threads_compose;
mod trigger_groups;
mod triggers;
mod user_only_messages;
mod voice;
pub(crate) mod voice_ticket;
mod webhooks;
mod widgets;
pub(crate) mod workspace_broker;
mod workspace_calls;
mod workspace_label;
mod workspace_list;
mod workspace_operations;
mod workspace_prompt_footprint;
mod ws_echo;

pub use proxy::{load_proxy_config, InsecureTransport, ProxyConfigLoad, RejectedProvider};

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{any, delete, get, post, put},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower::ServiceExt;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use crate::core::oauth::OAuthFlowResult;
use crate::core::{base_path, is_path_traversal};
use crate::core::{
    AppManager, ArtifactManager, ConversationSnapshot, CredentialInfo, EventStore,
    OAuthAccountInfo, Route, SessionMessage,
};
use crate::engine::thread_events::resolve_thread_id_arg;
use crate::engine::{AppContext, CaptureResult, ChatImage, SharedEngine, UrlContext};
use crate::memory::{EmbedderSlot, PgVectorIndex};
use crate::net_config::API_V1_PREFIX;
use crate::scheduler::{Notification, PushSubscription, SchedulerManager};

pub(crate) use error::ApiError;

type PendingOAuthFlows =
    Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<OAuthFlowResult>>>>;

pub(super) fn sanitize_leaf_filename(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn parse_optional_uuid(opt: Option<&str>) -> Result<Option<Uuid>, StatusCode> {
    opt.map(Uuid::parse_str)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)
}

pub(crate) fn parse_optional_uuid_trimmed(opt: Option<&str>) -> Result<Option<Uuid>, String> {
    match opt.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|_| s.to_string()),
    }
}

/// INVARIANT: Settle the `id` of a `NavigationRequested` payload that targets a thread.
///
/// Two surfaces emit that event and both come through here: the `navigate_ui`
/// LLM tool, which passes its calling thread, and the SDK's
/// `POST /api/v1/ui/navigate`, which passes `None` because an app iframe has no
/// thread. Sibling of `notifications::resolve_thread_tap_id`, which settles the
/// same id on the tap a notification stores.
///
/// Matched on the VALUE, never `as_str()`. Reading a non-string as absent is
/// what lets an `id` of `["<uuid>"]` ride the event unchecked, and the page
/// dereferences whatever arrives.
///
/// An ABSENT id is left alone, unlike the notification tap's. This event is
/// transient, so the page-side router reporting `Navigation target missing
/// thread id` reaches the same person in the same session.
pub(crate) fn resolve_thread_id_in_nav_payload(
    payload: &mut serde_json::Value,
    caller: Option<Uuid>,
) -> Result<(), String> {
    if payload.get("target").and_then(|v| v.as_str()) != Some("thread") {
        return Ok(());
    }
    let resolved = match payload.get("id") {
        None | Some(serde_json::Value::Null) => return Ok(()),
        Some(serde_json::Value::String(raw)) => resolve_thread_id_arg(raw, caller)?,
        Some(other) => {
            return Err(format!(
                "thread id must be a string, got {other}. Pass the thread's uuid."
            ))
        }
    };
    payload["id"] = serde_json::Value::String(resolved.to_string());
    Ok(())
}

/// INVARIANT: Settle the repository a `NavigationRequested` file locator names.
///
/// A `file_path` of `repo:<repo>:<mode>:<path>` opens a file in a registered
/// repository clone (codec: `store/repoPath.ts`). The page reads `<repo>` as a
/// uuid. Every other repo-scoped tool takes a name or an id, so an agent writes
/// the name. The page then fails on a bare uuid parse error, after the caller
/// was told the navigate went out.
///
/// So the segment resolves here, by id or name, and the payload carries the
/// uuid. A repository nobody registered is refused before anything is
/// emitted. Both emitters come through here, as with
/// [`resolve_thread_id_in_nav_payload`].
///
/// A malformed locator passes untouched. The page reads it as an artifact
/// path, which `system-knowhow/js-sdk.md` promises apps, and shows a load
/// error for it.
pub(crate) async fn resolve_repo_in_nav_payload(
    pool: &PgPool,
    payload: &mut serde_json::Value,
) -> Result<(), String> {
    if payload.get("target").and_then(|v| v.as_str()) != Some("file") {
        return Ok(());
    }
    let Some(file_path) = payload.get("file_path").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    let Some((repo, mode, path)) = split_repo_locator(file_path) else {
        return Ok(());
    };
    let repos = crate::core::repositories::RepositoryStore::list(pool)
        .await
        .map_err(|e| format!("could not list repositories for '{file_path}': {e}"))?;
    let id = match_repository(&repos, repo).map_err(|reason| {
        format!(
            "{reason} (in file_path '{file_path}'). manage_repositories 'list' shows each \
             one's name and id."
        )
    })?;
    payload["file_path"] = serde_json::Value::String(format!("repo:{id}:{mode}:{path}"));
    Ok(())
}

/// INVARIANT: The id of the one repository `reference` names: by id, then by exact name,
/// then by name in any case. The first tier with a hit decides, and two hits in
/// one tier are refused, since names are not unique. The same tiers as
/// `matchRepository` in `store/actions/repoFileLink.ts`, so a link and a
/// navigate land on the same repository or both refuse.
pub(crate) fn match_repository(
    repos: &[crate::core::repositories::Repository],
    reference: &str,
) -> Result<Uuid, String> {
    let id = Uuid::parse_str(reference).ok();
    let tiers: [&dyn Fn(&crate::core::repositories::Repository) -> bool; 3] =
        [&|r| Some(r.id) == id, &|r| r.name == reference, &|r| {
            r.name.to_lowercase() == reference.to_lowercase()
        }];
    for in_tier in tiers {
        let hits: Vec<_> = repos.iter().filter(|r| in_tier(r)).collect();
        match hits.as_slice() {
            [] => continue,
            [one] => return Ok(one.id),
            many => {
                return Err(format!(
                    "{} registered repositories share the name '{reference}', so name it by id",
                    many.len()
                ))
            }
        }
    }
    Err(format!(
        "no registered repository has the name or id '{reference}'"
    ))
}

/// INVARIANT: A well-formed `repo:` locator's repository, mode and path segments, or
/// `None`. The same rules as `parseRepoPath` in `store/repoPath.ts`: every
/// segment non-empty, mode `file` or `diff`, and a `#` qualifier non-empty.
/// Everything after the second colon is the path, since a ref cannot hold a
/// colon and a path can.
fn split_repo_locator(file_path: &str) -> Option<(&str, &str, &str)> {
    let mut parts = file_path.strip_prefix("repo:")?.splitn(3, ':');
    let (repo, mode, path) = (parts.next()?, parts.next()?, parts.next()?);
    let (mode_name, qualifier) = mode
        .split_once('#')
        .map_or((mode, None), |(name, q)| (name, Some(q)));
    let well_formed = !repo.is_empty()
        && !path.is_empty()
        && qualifier != Some("")
        && matches!(mode_name, "file" | "diff");
    well_formed.then_some((repo, mode, path))
}

pub(super) fn is_dangerous_git_ref(s: &str) -> bool {
    s.is_empty() || s.starts_with('-') || s.contains("..") || s.contains(';') || s.contains('|')
}

fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "txt" | "md" | "log" | "csv" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

#[derive(Clone)]
pub struct AppState {
    pub engine: SharedEngine,
    pub pool: PgPool,
    pub event_store: EventStore,
    pub embedder: Arc<EmbedderSlot>,
    pub memory_index: Option<PgVectorIndex>,
    pub workspace_path: PathBuf,
    pub app_manager: Arc<AppManager>,
    pub scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub pending_oauth_flows: PendingOAuthFlows,
    pub reveal_tokens: secret_reveal::RevealTokens,
    pub socket_tickets: socket_ticket::SocketTickets,
    pub event_hooks: Arc<dyn crate::core::event_hooks::EventHooks>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContext {
    pub path: String,
    #[serde(default)]
    pub lines: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoFileContext {
    pub repo_id: String,
    pub path: String,
    #[serde(default)]
    pub lines: Option<(u32, u32)>,
}

#[derive(Debug, Deserialize)]
pub struct AppCaptureRequest {
    pub request_id: String,
    #[serde(default)]
    pub device_id: Option<String>,
    pub screenshot: String,
    pub dom: String,
}

#[derive(Serialize, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "crate::core::event_hooks::HookArgs::is_empty"
    )]
    pub hook_args: crate::core::event_hooks::HookArgs,
    pub mode: crate::engine::thread_events::ActorMode,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub app_context: Option<AppContext>,
    #[serde(default)]
    pub file_context: Option<FileContext>,
    #[serde(default)]
    pub url_context: Option<UrlContext>,
    #[serde(default)]
    pub repo_file_context: Option<RepoFileContext>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// INVARIANT: Backend to serve the model on this turn, when it has more than one
    /// route. Honoured or refused, never substituted: a value naming an
    /// unconfigured backend errors rather than running somewhere else. Absent
    /// lets the model's own preferred provider decide.
    #[serde(default)]
    pub provider: Option<String>,
    /// INVARIANT: Images sent inline, for a caller that cannot upload first. A
    /// cross-workspace spawn is one, since the mutating gate refuses it the
    /// upload route. Each is stored exactly as an upload is, and a refused one
    /// fails the request with the upload's 415. Ignored when `image_hashes` is
    /// set.
    #[serde(default)]
    pub images: Option<Vec<ChatImage>>,
    #[serde(default)]
    pub image_hashes: Option<Vec<String>>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// INVARIANT: True when this request spawns / continues a *coding-agent thread* (any
    /// backend), as opposed to a chat thread answered by the Lucidos Agent.
    /// The `coding_agent` field below picks the backend. Aliased to the legacy
    /// `use_claude_code` key so payloads persisted before the rename (queued
    /// `ThreadQueueRequest` rows, in-flight clients) still deserialize.
    #[serde(default, alias = "use_claude_code")]
    pub use_coding_agent: Option<bool>,
    #[serde(default)]
    pub cc_model: Option<String>,
    #[serde(default)]
    pub coding_agent: Option<crate::runtime::CodingAgent>,
    #[serde(default, alias = "message_id")]
    pub event_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub input_chain: Option<String>,
    #[serde(default)]
    pub input_seq: Option<i64>,
    /// INVARIANT: Compose epoch captured with the device's send intent. An accepted
    /// retry retains this value; it is never refreshed from today's draft.
    #[serde(default)]
    pub compose_epoch: Option<i64>,
    /// INVARIANT: `true` when `thread_id` names a thread that is EXPECTED not to exist yet,
    /// because this request is the one creating it.
    ///
    /// A thread has no creation event: it exists because an event exists on its
    /// aggregate id, and the `MessageReceived` projection is an upsert. So a
    /// `thread_id` naming nothing used to be indistinguishable from a follow-up
    /// on a thread that does exist, and the engine silently manufactured the
    /// thread instead of refusing. A caller that reached the wrong engine got a
    /// thread there, and reading its own message back confirmed the mistake
    /// rather than catching it (2026-08-06).
    ///
    /// So an id-carrying create must now say so. Omitted (or `false`) means "I
    /// am addressing a thread that already exists", and an id that names nothing
    /// is a 404. Carrying `parent_thread_id` or `caller_workspace` is an
    /// equivalent create signal, which is why `lucidos spawn-thread` and the
    /// cross-workspace client need no flag.
    #[serde(default)]
    pub new_thread: Option<bool>,
    #[serde(default)]
    pub parent_thread_id: Option<String>,
    #[serde(default)]
    pub spawning_event_id: Option<String>,
    #[serde(default)]
    pub wake_mode: Option<String>,
    #[serde(default)]
    pub caller_workspace: Option<String>,
    #[serde(default)]
    pub caller_thread_id: Option<String>,
    #[serde(default)]
    pub caller_event_id: Option<String>,
    #[serde(default)]
    pub conflict_change_id: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub integrated: bool,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub agent_mentioned: bool,
}

#[derive(Serialize)]
pub struct CancelResponse {
    pub canceled: bool,
}

/// INVARIANT: Chat Stop persists unread-input take-backs before canceling the live turn.
/// `canceled` describes the live cancellation only; an input taken back with
/// no live turn still needs an immediate client re-sync. Returned content lets
/// another device restore inputs absent from its local timeline.
#[derive(Serialize)]
pub struct ChatCancelResponse {
    pub canceled: bool,
    #[serde(flatten)]
    pub take_back: crate::engine::consumption_claim::TakeBackReport,
}

#[derive(Serialize)]
pub struct NotificationsResponse {
    pub notifications: Vec<Notification>,
    pub unread_count: i64,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct MarkReadResponse {
    pub success: bool,
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub success: bool,
    pub filename: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct CredentialsListResponse {
    pub credentials: Vec<CredentialInfo>,
}

#[derive(Deserialize, Default)]
pub struct RequestedScope {
    #[serde(default)]
    pub base_urls: Option<Vec<String>>,
    #[serde(default)]
    pub base_url: Option<String>,
}

impl RequestedScope {
    /// INVARIANT: Every base URL the body asked for, or `None` when it named NEITHER
    /// field.
    ///
    /// The distinction is load-bearing on the update verb, where absence has to
    /// mean "leave the scope alone". Both fields are optional, so the two
    /// spellings can coexist. An absent scope therefore deserializes as an
    /// empty one, and an empty one CLEARS the credential. A `PUT` that only
    /// meant to change the auth header would silently have emptied the set.
    pub fn declared(&self) -> Option<Vec<String>> {
        if self.base_urls.is_none() && self.base_url.is_none() {
            return None;
        }
        let mut out = self.base_urls.clone().unwrap_or_default();
        out.extend(self.base_url.clone());
        Some(out)
    }
}

#[derive(Deserialize)]
pub struct CreateCredentialRequest {
    pub service_name: String,
    #[serde(flatten)]
    pub scope: RequestedScope,
    pub auth_type: String,
    pub auth_value: String,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub env_var_name: Option<String>,
    #[serde(default)]
    pub form_request_id: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
pub struct SetCredentialBaseUrlsRequest {
    pub base_urls: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateCredentialRequest {
    #[serde(flatten)]
    pub scope: RequestedScope,
    pub auth_type: String,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_value: Option<String>,
    /// INVARIANT: Editable email server settings, present only when editing an
    /// `email_password` credential. The credential's `email_accounts` row is
    /// the source of truth for IMAP/SMTP, so it must be kept in sync here.
    #[serde(default)]
    pub email: Option<EmailAccountSettings>,
    #[serde(default)]
    pub env_var_name: Option<String>,
    #[serde(default)]
    pub form_request_id: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
pub struct EmailAccountSettings {
    pub email_address: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub smtp_host: String,
    pub smtp_port: i32,
    pub username: String,
    pub use_tls: bool,
    pub require_send_confirmation: bool,
}

pub fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
pub struct RouteInfo {
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i32>,
    pub reasoning_efforts: &'static [&'static str],
    pub configured: Option<bool>,
    pub proxy: Option<ModelProxyInfo>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ModelProxyProtocol {
    AnthropicMessages,
    OpenaiResponses,
    OpenaiChatCompletions,
    VertexAnthropic,
    VertexGoogle,
}

#[derive(Serialize, Deserialize)]
pub struct ModelProxyInfo {
    pub name: String,
    pub protocol: ModelProxyProtocol,
    pub request_defaults: std::collections::BTreeMap<String, serde_json::Value>,
}

/// INVARIANT: A registry row as the API serves it: its own fields, with `routes` replaced
/// by the derived [`RouteInfo`] list.
///
/// Spelled out rather than flattened over `Model`, because the served `routes`
/// carry a field the stored ones do not. A flatten would emit the key twice.
#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub routes: Vec<RouteInfo>,
    pub preferred_provider: Option<String>,
    pub vision: bool,
    pub default_effort: Option<String>,
    pub successor: Option<String>,
    pub sort_order: i32,
    pub source: String,
    pub on_chat_ladder: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct ModelsListResponse {
    pub models: Vec<ModelInfo>,
}

/// INVARIANT: The merged *style library*, shipped rows first. Read-only: the library is
/// written through `PUT /api/v1/preferences` on `response_styles`, so that the
/// one write chokepoint bounds-checks every edit.
#[derive(Serialize)]
pub struct ResponseStylesListResponse {
    pub styles: Vec<crate::core::response_style::Style>,
}

#[derive(Deserialize)]
pub struct CreateModelRequest {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default)]
    pub context_window: Option<i32>,
    #[serde(default)]
    pub routes: Option<Vec<Route>>,
    #[serde(default)]
    pub vision: bool,
}

#[derive(Deserialize)]
pub struct UpdateModelRequest {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default)]
    pub on_chat_ladder: Option<bool>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub context_window: Option<Option<i32>>,
    #[serde(default)]
    pub routes: Option<Vec<Route>>,
    /// INVARIANT: The provider to remember for this model. Absent keeps the stored pick;
    /// an explicit `null` clears it back to the first configured route. Must
    /// name one of the row's routes.
    ///
    /// This is the model picker's write path, so it is accepted on a builtin.
    /// Refusing it there would leave every seeded model stuck on one backend.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub preferred_provider: Option<Option<String>>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Serialize)]
pub struct CronPreview {
    pub next_runs: Vec<String>,
    pub warnings: Vec<String>,
}

impl CronPreview {
    pub(crate) fn from_validated(validated: &crate::scheduler::cron_rules::ValidatedCron) -> Self {
        Self {
            next_runs: validated.next_runs_rfc3339(),
            warnings: validated.warnings.clone(),
        }
    }
}

#[derive(Serialize, Default)]
pub struct ApiResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_request: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_preview: Option<CronPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

impl ApiResult {
    fn ok() -> Json<Self> {
        Json(Self {
            success: true,
            ..Default::default()
        })
    }
    fn ok_for_trigger(preview: Option<CronPreview>, warnings: Vec<String>) -> Json<Self> {
        Json(Self {
            success: true,
            cron_preview: preview,
            warnings: (!warnings.is_empty()).then_some(warnings),
            ..Default::default()
        })
    }
    fn err(msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            error: Some(msg.into()),
            ..Default::default()
        })
    }
    fn with_auth_url(url: String) -> Json<Self> {
        Json(Self {
            success: true,
            auth_url: Some(url),
            ..Default::default()
        })
    }
    fn needs_credentials(
        provider: &str,
        overrides: &crate::core::oauth::OAuthClientOverrides,
    ) -> Json<Self> {
        Json(Self {
            error: Some(format!(
                "OAuth client credentials required for {}",
                provider
            )),
            credential_request: Some(crate::core::oauth::oauth_client_request(
                provider, overrides,
            )),
            ..Default::default()
        })
    }

    fn needs_credential_repair(provider: &str, request: serde_json::Value) -> Json<Self> {
        Json(Self {
            error: Some(format!(
                "The OAuth client registration for {} is incomplete",
                provider
            )),
            credential_request: Some(request),
            ..Default::default()
        })
    }
}

#[derive(Serialize)]
pub struct OAuthAccountsListResponse {
    pub accounts: Vec<OAuthAccountInfo>,
}

#[derive(Deserialize)]
pub struct OAuthAccountQuery {
    pub id: String,
}

#[derive(Deserialize)]
pub struct DeviceRegisterRequest {
    pub device_id: String,
    pub user_agent: Option<String>,
}

#[derive(Deserialize)]
pub struct DeviceRenameRequest {
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct DevicePushRequest {
    pub push_enabled: bool,
}

#[derive(Deserialize)]
pub struct DeviceHandOverRequest {
    pub old_device_id: String,
    pub device_id: String,
}

#[derive(Serialize)]
pub struct DevicesListResponse {
    pub devices: Vec<crate::core::devices::DeviceListing>,
}

#[derive(Serialize)]
pub struct PreferencesResponse {
    pub preferences: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
pub struct SetPreferenceRequest {
    pub value: String,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Deserialize)]
struct PreferencesQuery {
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Deserialize)]
pub struct EmitEventRequest {
    pub event_type: String,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub transient: bool,
}

#[derive(Serialize)]
pub struct VapidKeyResponse {
    pub public_key: String,
}

#[derive(Deserialize)]
pub struct PushSubscribeRequest {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_id: Option<String>,
    #[serde(default)]
    pub scope_url: Option<String>,
}

#[derive(Deserialize)]
pub struct PushUnsubscribeRequest {
    pub endpoint: String,
}

#[derive(Deserialize)]
struct CredentialIdQuery {
    id: uuid::Uuid,
}

#[derive(Deserialize)]
struct KeyQuery {
    key: String,
}

#[derive(Deserialize)]
struct NameQuery {
    name: String,
}

#[derive(Deserialize)]
struct ProviderQuery {
    provider: String,
}

#[derive(Deserialize)]
pub(super) struct ModelIdQuery {
    pub id: String,
}

#[derive(Deserialize)]
pub(super) struct DeleteModelQuery {
    pub id: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Deserialize)]
struct NotificationQuery {
    id: Uuid,
}

#[derive(Deserialize)]
struct BeforeTimestampQuery {
    before: i64,
}

fn default_notif_limit() -> i64 {
    50
}

#[derive(Deserialize)]
struct NotificationsListQuery {
    #[serde(default = "default_notif_limit")]
    limit: i64,
    before: Option<f64>,
    #[serde(default)]
    filter: Option<String>,
}

fn parse_unix_ts(ts: f64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    let secs = ts as i64;
    let nanos = ((ts - secs as f64) * 1_000_000_000.0) as u32;
    chrono::Utc
        .timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(chrono::Utc::now)
}

#[derive(Deserialize)]
struct ChangesListQuery {
    limit: Option<i64>,
    before: Option<f64>,
    sub_threads_of: Option<Uuid>,
}

#[derive(Deserialize)]
struct EventQuery {
    event: Uuid,
}

#[derive(Deserialize)]
struct SessionMessagesQuery {
    id: String,
}

#[derive(Serialize)]
struct GitVersion {
    commit: String,
    message: String,
    timestamp: i64,
    author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
}

async fn request_logger(req: axum::extract::Request, next: Next) -> Response {
    let uri_path = req.uri().path();
    let should_log = match uri_path {
        "/api/v1/health" | "/api/v1/events" => false,
        p => p.starts_with("/api/v1/") || p.starts_with("/app/"),
    };
    if !should_log {
        return next.run(req).await;
    }

    let method = req.method().clone();
    let path = uri_path.to_string();
    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16();
    let elapsed = start.elapsed();

    log_legacy!("[API] {} {} → {} ({:.0?})", method, path, status, elapsed);

    response
}

async fn serve_frontend(static_dir: PathBuf, req: axum::extract::Request) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }

    let prefix = base_path::forwarded_prefix(req.headers());
    let path = req.uri().path().to_string();

    if path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if path == "/" || path == "/index.html" {
        return serve_shell(&static_dir, &prefix);
    }

    let service = ServeDir::new(&static_dir);
    match service.oneshot(req).await {
        Ok(resp) if resp.status() != StatusCode::NOT_FOUND => resp.map(axum::body::Body::new),
        _ if path.starts_with("/assets/") => StatusCode::NOT_FOUND.into_response(),
        _ => serve_shell(&static_dir, &prefix),
    }
}

fn serve_shell(static_dir: &std::path::Path, prefix: &str) -> Response {
    let index = static_dir.join("index.html");
    match std::fs::read_to_string(&index) {
        Ok(html) => {
            let stamped = base_path::inject_base_href(&html, prefix);
            let stamped =
                base_path::inject_gateway_port(&stamped, base_path::gateway_port().as_deref());
            let stamped =
                base_path::inject_workspace_id(&stamped, base_path::workspace_id().as_deref());
            (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                stamped,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}

pub(crate) const API_BODY_LIMIT: usize = 100 * 1024 * 1024;

pub(crate) fn api_routes(state: &AppState) -> Router {
    // INVARIANT: ALL routes live under `/api/v1/`: see `.claude/rules/rust.md`
    // § API URL Conventions.
    // Convention: query params for identifiers, path segments only for file paths.
    //
    // Route registration lives with the domain module that owns each URL
    // surface (`<module>::router()`); this merge is the single mount point.
    // Routes whose handler lives in a different module than their path prefix
    // (e.g. `/changes/:id/diff` → `repositories::get_change_diff`) register in
    // the router of the module that owns the path, so a reader looking for a
    // path finds it under the path's domain.
    //
    // The fallback is load-bearing: without it, an unmatched `/api/v1/*`
    // request would fall through the nest to the outer router's static
    // frontend fallback instead of returning 404.
    Router::new()
        .merge(history::router())
        .merge(hooks::router())
        .merge(chat::router())
        .merge(claude_code::router())
        .merge(claude_gateway::router())
        .merge(chatgpt_plan::router())
        .merge(intelligence_ladder::router())
        .merge(model_prices::router())
        .merge(spend_cap_holds::router())
        .merge(side_questions::router())
        .merge(permission_requests::router())
        .merge(permissions::router())
        .merge(threads::router())
        .merge(changes::router())
        .merge(main_check::router())
        .merge(gateway_signals::router())
        .merge(notifications::router())
        .merge(artifacts::router())
        .merge(settings::router())
        .merge(background_models::router())
        .merge(credential_reveal::router())
        .merge(onepassword::router())
        .merge(triggers::router())
        .merge(trigger_groups::router())
        .merge(thread_queue::router())
        .merge(knowhow::router())
        .merge(presence::router())
        .merge(presence_pong::router())
        .merge(memory::router())
        .merge(recall::router())
        .merge(apps::router())
        .merge(widgets::router())
        .merge(command_permission::router())
        .merge(command_checkpoint::router())
        .merge(mcp::router())
        .merge(mcp_permission::router())
        .merge(form_requests::router())
        .merge(internal::router())
        .merge(backup::router())
        .merge(snapshots::router())
        .merge(disk_usage::router())
        .merge(terminal::router())
        .merge(terminal_socket::router())
        .merge(voice::router())
        .merge(voice_ticket::router())
        .merge(ws_echo::router())
        .merge(frontend_preview::router())
        .merge(search::router())
        .merge(release_notices::router())
        .merge(repositories::router())
        .merge(sdk::router())
        .merge(fonts::router())
        .merge(sdk_prefs::router())
        .merge(data_api::router())
        .merge(blobs::router())
        .merge(bootstrap::router())
        .merge(themes::router())
        .merge(plugins::router())
        .merge(plugin_media::router())
        .merge(webhooks::router())
        .merge(thread_users::router())
        .merge(user_only_messages::router())
        .merge(config_changes::router())
        .merge(users::router())
        .merge(workspace_calls::router())
        .merge(workspace_operations::router())
        .merge(diagnostic_bundles::router())
        .merge(workspace_label::router())
        .merge(workspace_list::router())
        .merge(workspace_broker::router())
        .merge(host_log_transfer::router())
        .merge(gateway_requests::router())
        .merge(workspace_prompt_footprint::router())
        .merge(handshake_scripts::router())
        .merge(proxy::router())
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        // INVARIANT: Axum's 2 MiB default rejects mobile screenshots in chat/app-capture
        // bodies — and large `PUT /api/v1/data/*path` binary writes — with
        // "Failed to buffer the request body". `Router::layer` only covers
        // routes registered before the call, so this is applied after every
        // domain merge to guarantee it reaches all routes.
        .layer(DefaultBodyLimit::max(API_BODY_LIMIT))
        .layer(CompressionLayer::new().compress_when(compression_predicate()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            mutating_gate::enforce_caller_identified,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            restore_generation::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            threads::child_reports::note_waiting_child_reports,
        ))
        .layer(axum::middleware::from_fn(app_reach::enforce_app_reach))
        .layer(axum::middleware::from_fn(
            crate::core::oauth_callback::note_browser_context,
        ))
        // INVARIANT: Refuse a request that named a DIFFERENT workspace than this engine
        // serves (409). Layered here rather than per-handler because a
        // mis-aimed write is a hazard on every mutating endpoint, and a
        // per-handler check is one a new endpoint can forget. Applied after
        // every domain merge for the same reason `DefaultBodyLimit` is:
        // `Router::layer` only covers routes registered before the call.
        // See `api::target_workspace`.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            target_workspace::enforce_target_workspace,
        ))
        // INVARIANT: Read a gateway stamp's age before any body arrives
        // (`api::gateway_stamp::note_arrival`).
        .layer(axum::middleware::from_fn(gateway_stamp::note_arrival))
        .layer(axum::middleware::from_fn_with_state(
            lucidos_diagnostic_log::fault::Inbox::new(&state.workspace_path),
            internal_faults::observe,
        ))
        .with_state(state.clone())
}

#[allow(clippy::too_many_arguments)]
pub fn create_router(
    engine: SharedEngine,
    pool: PgPool,
    event_store: EventStore,
    embedder: Arc<EmbedderSlot>,
    memory_index: Option<PgVectorIndex>,
    workspace_path: PathBuf,
    scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
    started_at: chrono::DateTime<chrono::Utc>,
    event_hooks: Arc<dyn crate::core::event_hooks::EventHooks>,
) -> Router {
    let app_manager = engine.shared_app_manager();

    let served_frontend_handle: Option<Arc<std::sync::RwLock<PathBuf>>> =
        std::env::var_os("LUCIDOS_STATIC_DIR").map(|static_dir| {
            // INVARIANT: Dev pins a private snapshot of `dist/`. The running engine then
            // serves only the client it was built against, even on a hard
            // reload, never a newer and possibly incompatible one. Packaged
            // serves the bundled Resources dir unchanged. See `frontend_snapshot`.
            let source = PathBuf::from(&static_dir);
            let served = frontend_snapshot::resolve_served_dir(source.clone(), &workspace_path);
            let handle = Arc::new(std::sync::RwLock::new(served));
            engine.init_served_frontend(handle.clone(), source);
            handle
        });

    let state = AppState {
        engine,
        pool,
        event_store,
        embedder,
        memory_index,
        workspace_path: workspace_path.clone(),
        app_manager,
        scheduler,
        started_at,
        pending_oauth_flows: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        reveal_tokens: secret_reveal::RevealTokens::new(),
        socket_tickets: socket_ticket::SocketTickets::new(),
        event_hooks,
    };

    // INVARIANT: Serve static files from the data/ tree. One mount covers every
    // subdirectory, gated by the same read allowlist `GET /api/v1/data/*path`
    // applies. See `data_api::serve_workspace_data`.
    let serve_data = data_api::static_mount(workspace_path.join(crate::core::DATA_DIR));

    let app_ui_state = state.clone();

    let api_routes = api_routes(&state);
    state.engine.set_api_router(api_routes.clone());

    // INVARIANT: Refuse a browser request that came from another origin (403), in front of
    // everything. An engine binds loopback, so its remaining browser-shaped
    // exposure is a page on some other origin driving this port out of the
    // user's own browser. Layered last, which makes it OUTERMOST, so it runs
    // before routing and before any handler resolves a credential.
    //
    // `LUCIDOS_PERMISSIVE_CORS` skips it: that variable already declares this
    // deployment wants cross-origin browser access, and a gate refusing exactly
    // what the CORS layer below allows would answer to nobody. `/proxy/*` keeps
    // its own copy of the check for that case, since a credentialed route must
    // refuse even then. See `api::browser_origin`.
    let api_routes = if permissive_cors_enabled() {
        crate::log_legacy!("[API] permissive CORS is on, so the same-origin gate is off with it");
        api_routes
    } else {
        api_routes.layer(axum::middleware::from_fn(browser_origin::enforce))
    };

    let frame_files = Router::new()
        .nest("/app", apps::ui_router().with_state(app_ui_state))
        .nest_service("/data", serve_data)
        .layer(axum::middleware::from_fn(
            frame_file_cors::grant_font_and_script_loads,
        ));

    let router = Router::new()
        .nest(API_V1_PREFIX, api_routes)
        .merge(frame_files);

    let router = if let Some(handle) = served_frontend_handle {
        router.fallback(move |req: axum::extract::Request| {
            let static_dir = handle.read().unwrap().clone();
            serve_frontend(static_dir, req)
        })
    } else {
        router
    };

    local_auth::publish_this_engines_local_token();
    let router = router
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        // INVARIANT: Who is calling. Layered out here rather than inside the `/api/v1`
        // nest. `/app`, `/data` and the frontend fallback are siblings of that
        // nest, so a gate inside it would miss all three. `/data` serves the
        // workspace's files straight off disk, which would be the whole leak.
        //
        // The engine binds loopback only, so it admits only a loopback Host,
        // which shuts out a DNS-rebound page.
        //
        // Deliberately NOT skipped for `LUCIDOS_PERMISSIVE_CORS`, unlike the
        // origin gate below. That variable says this deployment wants
        // cross-origin browser access. It does not say it wants no
        // authentication, and reading it that way would make an escape hatch
        // for one concern silently open another.
        .layer(axum::middleware::from_fn(local_auth::enforce))
        // INVARIANT: A request the credentialed proxy forwarded is never the shell, so
        // it is refused before any credential is read. See
        // `proxy::refuse_proxied_request`.
        .layer(axum::middleware::from_fn(proxy::refuse_proxied_request))
        .layer(axum::middleware::from_fn(request_logger));

    if permissive_cors_enabled() {
        router.layer(CorsLayer::permissive())
    } else {
        router
    }
}

/// INVARIANT: Compress text-shaped payloads only. Already-compressed media gains nothing
/// and loses two headers it needs.
///
/// tower-http strips `Accept-Ranges` from every response it compresses, and
/// drops `Content-Length` with it. So compressing a video is on its own enough
/// to stop a `<video>` element seeking, however the handler answers. A `206` is
/// already safe, since tower-http never compresses a response carrying
/// `Content-Range`; the full `200` is the exposure. Curl hides this, because it
/// sends no `accept-encoding` unless asked; every browser does.
///
/// `DefaultPredicate` covers `image/`, gRPC and SSE. Added here: video and audio
/// (both are compressed containers, and both want seeking), woff/woff2 (already
/// deflated), and the archive types.
fn compression_predicate() -> impl Predicate {
    DefaultPredicate::new()
        .and(NotForContentType::const_new("video/"))
        .and(NotForContentType::const_new("audio/"))
        .and(NotForContentType::const_new("font/"))
        .and(NotForContentType::const_new("application/pdf"))
        .and(NotForContentType::const_new("application/zip"))
        .and(NotForContentType::const_new("application/gzip"))
}

fn permissive_cors_enabled() -> bool {
    permissive_cors_enabled_value(std::env::var("LUCIDOS_PERMISSIVE_CORS").ok().as_deref())
}

fn permissive_cors_enabled_value(value: Option<&str>) -> bool {
    value.is_some_and(crate::core::prefs::env_switch_is_on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{MAX_IMAGE_BASE64_BYTES, MAX_IMAGE_DIMENSION};
    use base64::Engine as _;
    use image::{ImageBuffer, Rgba};

    fn make_test_image(width: u32, height: u32) -> ChatImage {
        let img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_fn(width, height, |x, y| {
                Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
            });
        let mut png_buf = std::io::Cursor::new(Vec::new());
        img_buf
            .write_to(&mut png_buf, image::ImageFormat::Png)
            .unwrap();
        let base64_data = base64::engine::general_purpose::STANDARD.encode(png_buf.into_inner());
        ChatImage {
            base64: base64_data,
            mime_type: "image/png".to_string(),
        }
    }

    #[test]
    fn a_body_naming_no_scope_is_told_apart_from_one_naming_an_empty_scope() {
        let parse = |body: &str| {
            serde_json::from_str::<RequestedScope>(body)
                .expect("the body parses")
                .declared()
        };

        assert_eq!(
            parse("{}"),
            None,
            "neither field named: keep what is stored"
        );
        assert_eq!(
            parse(r#"{"base_urls":[]}"#),
            Some(vec![]),
            "an empty list is a real answer and clears the scope"
        );
        assert_eq!(
            parse(r#"{"base_urls":["https://api.binance.com"]}"#),
            Some(vec!["https://api.binance.com".to_string()])
        );
    }

    #[test]
    fn the_old_singular_field_joins_the_set() {
        let both: RequestedScope = serde_json::from_str(
            r#"{"base_urls":["https://api.binance.com"],"base_url":"https://fapi.binance.com"}"#,
        )
        .expect("the body parses");
        assert_eq!(
            both.declared(),
            Some(vec![
                "https://api.binance.com".to_string(),
                "https://fapi.binance.com".to_string(),
            ])
        );

        let singular: RequestedScope =
            serde_json::from_str(r#"{"base_url":"https://api.binance.com"}"#)
                .expect("the body parses");
        assert_eq!(
            singular.declared(),
            Some(vec!["https://api.binance.com".to_string()]),
            "the singular alone still names a scope"
        );
    }

    #[test]
    fn compression_skips_media_and_still_covers_json() {
        let probe = |content_type: &str| {
            let body = axum::body::Body::from(vec![b'x'; 4096]);
            let response = Response::builder()
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, "4096")
                .body(body)
                .unwrap();
            compression_predicate().should_compress(&response)
        };

        for compressible in [
            "application/json",
            "text/plain",
            "text/html",
            "application/javascript",
            "image/svg+xml",
        ] {
            assert!(probe(compressible), "{compressible} should compress");
        }
        for skipped in [
            "video/mp4",
            "video/webm",
            "audio/mpeg",
            "audio/wav",
            "font/woff2",
            "application/pdf",
            "application/zip",
            "application/gzip",
            "image/png",
        ] {
            assert!(!probe(skipped), "{skipped} should not compress");
        }
    }

    #[test]
    fn permissive_cors_is_disabled_unless_explicitly_enabled() {
        for value in [None, Some(""), Some("0"), Some("false"), Some("off")] {
            assert!(!permissive_cors_enabled_value(value), "value: {value:?}");
        }
        for &value in crate::core::prefs::FLAG_ON_VALUES {
            assert!(
                permissive_cors_enabled_value(Some(value)),
                "value: {value:?}"
            );
        }
    }

    #[tokio::test]
    async fn missing_hashed_frontend_asset_is_404_not_html_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!doctype html><html><head></head><body>shell</body></html>",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();

        let request = axum::extract::Request::builder()
            .uri("/assets/index-oldhash.js")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = serve_frontend(dir.path().to_path_buf(), request).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_ne!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8")),
            "a missing module must never receive index.html as JavaScript"
        );
    }

    #[test]
    fn small_image_compressed_to_jpeg() {
        let img = make_test_image(100, 100);
        let result = img.compress();
        assert_eq!(result.mime_type, "image/jpeg");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.base64)
            .unwrap();
        assert!(decoded.starts_with(&[0xFF, 0xD8]), "must be valid JPEG");
    }

    #[test]
    fn large_dimensions_capped() {
        let img = make_test_image(4000, 3000);
        let result = img.compress();
        assert!(
            result.base64.len() <= MAX_IMAGE_BASE64_BYTES,
            "must be under limit: {} bytes",
            result.base64.len()
        );
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.base64)
            .unwrap();
        let loaded = image::load_from_memory(&decoded).unwrap();
        let (w, h) = image::GenericImageView::dimensions(&loaded);
        assert!(
            w <= MAX_IMAGE_DIMENSION && h <= MAX_IMAGE_DIMENSION,
            "dimensions {}x{} must be <= {}",
            w,
            h,
            MAX_IMAGE_DIMENSION
        );
    }

    #[test]
    fn is_path_traversal_pins_security_contract() {
        assert!(!is_path_traversal("scripts/auth/handshake.py"));
        assert!(!is_path_traversal("foo.py"));
        assert!(!is_path_traversal("a/b/c/d.txt"));

        assert!(is_path_traversal("../escape"));
        assert!(is_path_traversal("../../etc/passwd"));

        assert!(is_path_traversal("/etc/passwd"));
        assert!(is_path_traversal("\\windows\\system32"));

        assert!(is_path_traversal("foo/../bar"));

        assert!(is_path_traversal("weird..name.txt"));

        assert!(!is_path_traversal("%2e%2e/escape"));
    }

    #[test]
    fn oversized_image_compressed_under_limit() {
        let img = make_test_image(5000, 4000);
        assert!(
            img.base64.len() > MAX_IMAGE_BASE64_BYTES,
            "Test image must exceed limit: {} bytes",
            img.base64.len()
        );

        let result = img.compress();
        assert!(
            result.base64.len() <= MAX_IMAGE_BASE64_BYTES,
            "Compressed image must be under limit: {} bytes",
            result.base64.len()
        );
        assert_eq!(result.mime_type, "image/jpeg");
    }

    #[test]
    fn invalid_base64_returns_unchanged() {
        let img = ChatImage {
            base64: "not-valid-base64!!!".to_string(),
            mime_type: "image/png".to_string(),
        };
        let result = img.compress();
        assert_eq!(result.base64, "not-valid-base64!!!");
    }

    #[test]
    fn fit_for_llm_passes_small_image_through_untouched() {
        let img = ChatImage {
            base64: "AAAA".to_string(),
            mime_type: "image/png".to_string(),
        };
        let result = img.fit_for_llm();
        assert_eq!(result.base64, "AAAA");
        assert_eq!(result.mime_type, "image/png");
    }

    fn make_flat_png(width: u32, height: u32) -> ChatImage {
        let flat: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(width, height, Rgba([240, 240, 240, 255]));
        let mut png_buf = std::io::Cursor::new(Vec::new());
        flat.write_to(&mut png_buf, image::ImageFormat::Png)
            .unwrap();
        let img = ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(png_buf.into_inner()),
            mime_type: "image/png".to_string(),
        };
        assert!(img.base64.len() <= MAX_IMAGE_BASE64_BYTES);
        img
    }

    #[test]
    fn fit_for_llm_keeps_an_image_the_provider_accepts_byte_identical() {
        let img = make_flat_png(3000, 7900);
        let original = img.base64.clone();
        let result = img.fit_for_llm();
        assert_eq!(result.base64, original);
        assert_eq!(result.mime_type, "image/png");
    }

    #[test]
    fn fit_for_llm_downscales_a_tall_image_that_is_small_in_bytes() {
        let img = make_flat_png(1485, 8420);

        let result = img.fit_for_llm();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.base64)
            .unwrap();
        let loaded = image::load_from_memory(&decoded).unwrap();
        let (w, h) = image::GenericImageView::dimensions(&loaded);
        assert!(
            w <= MAX_IMAGE_DIMENSION && h <= MAX_IMAGE_DIMENSION,
            "dimensions {w}x{h} must be <= {MAX_IMAGE_DIMENSION}"
        );
    }

    #[test]
    fn fit_for_llm_compresses_oversized_image() {
        let img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(1400, 1400, |x, y| {
            let mut h = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            h ^= h >> 15;
            h = h.wrapping_mul(0x85EB_CA6B);
            h ^= h >> 13;
            let b = h.to_le_bytes();
            Rgba([b[0], b[1], b[2], 255])
        });
        let mut png_buf = std::io::Cursor::new(Vec::new());
        img_buf
            .write_to(&mut png_buf, image::ImageFormat::Png)
            .unwrap();
        let img = ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(png_buf.into_inner()),
            mime_type: "image/png".to_string(),
        };
        assert!(
            img.base64.len() > MAX_IMAGE_BASE64_BYTES,
            "fixture must start over the target, got {} bytes",
            img.base64.len()
        );
        let result = img.fit_for_llm();
        assert!(
            result.base64.len() <= MAX_IMAGE_BASE64_BYTES,
            "fitted payload must be within target, got {} bytes",
            result.base64.len()
        );
        assert_eq!(result.mime_type, "image/jpeg");
    }

    fn make_jpeg_with_exif_orientation(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let img_buf: ImageBuffer<image::Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(width, height, |x, y| {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            });
        let mut jpeg_buf = std::io::Cursor::new(Vec::new());
        img_buf
            .write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg_bytes = jpeg_buf.into_inner();

        let orient_lo = (orientation & 0xFF) as u8;
        let orient_hi = ((orientation >> 8) & 0xFF) as u8;
        #[rustfmt::skip]
        let exif_payload: Vec<u8> = vec![
            0x45, 0x78, 0x69, 0x66, 0x00, 0x00,
            0x49, 0x49,
            0x2A, 0x00,
            0x08, 0x00, 0x00, 0x00,
            0x01, 0x00,
            0x12, 0x01,
            0x03, 0x00,
            0x01, 0x00, 0x00, 0x00,
            orient_lo, orient_hi, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];

        let app1_len = (exif_payload.len() + 2) as u16;
        let mut result = Vec::new();
        result.extend_from_slice(&[0xFF, 0xD8]);
        result.push(0xFF);
        result.push(0xE1);
        result.push((app1_len >> 8) as u8);
        result.push((app1_len & 0xFF) as u8);
        result.extend_from_slice(&exif_payload);
        result.extend_from_slice(&jpeg_bytes[2..]);
        result
    }

    fn assert_compressed_dimensions(img: ChatImage, expected_w: u32, expected_h: u32, label: &str) {
        let result = img.compress();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.base64)
            .unwrap();
        let loaded = image::load_from_memory(&decoded).unwrap();
        let (w, h) = image::GenericImageView::dimensions(&loaded);
        assert_eq!(
            (w, h),
            (expected_w, expected_h),
            "{}: expected {}×{}, got {}×{}",
            label,
            expected_w,
            expected_h,
            w,
            h
        );
    }

    fn make_exif_chat_image(width: u32, height: u32, orientation: u16) -> ChatImage {
        let jpeg_bytes = make_jpeg_with_exif_orientation(width, height, orientation);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
        ChatImage {
            base64: b64,
            mime_type: "image/jpeg".to_string(),
        }
    }

    #[test]
    fn compress_applies_exif_orientation() {
        assert_compressed_dimensions(
            make_exif_chat_image(200, 100, 6),
            100,
            200,
            "EXIF orientation 6",
        );
        assert_compressed_dimensions(
            make_exif_chat_image(200, 100, 8),
            100,
            200,
            "EXIF orientation 8",
        );
        assert_compressed_dimensions(
            make_exif_chat_image(200, 100, 3),
            200,
            100,
            "EXIF orientation 3",
        );
    }

    #[test]
    fn compress_no_exif_preserves_dimensions() {
        assert_compressed_dimensions(make_test_image(200, 100), 200, 100, "no EXIF (PNG)");
    }

    #[test]
    fn emit_event_request_parses_transient_true() {
        let body = serde_json::json!({
            "event_type": "SlidePresenterPing",
            "payload": {"timestamp": 123},
            "transient": true,
        });
        let req: EmitEventRequest = serde_json::from_value(body).unwrap();
        assert!(req.transient);
        assert_eq!(req.event_type, "SlidePresenterPing");
    }

    #[test]
    fn emit_event_request_transient_defaults_to_false() {
        let body = serde_json::json!({
            "event_type": "Anything",
            "payload": {},
        });
        let req: EmitEventRequest = serde_json::from_value(body).unwrap();
        assert!(
            !req.transient,
            "omitted transient must default to false so existing callers persist"
        );
    }

    #[test]
    fn sanitize_leaf_filename_rejects_traversal_and_separators() {
        assert!(super::sanitize_leaf_filename("../escape.txt").is_none());
        assert!(super::sanitize_leaf_filename("foo/../bar.txt").is_none());
        assert!(super::sanitize_leaf_filename("..").is_none());
        assert!(super::sanitize_leaf_filename(".").is_none());
        assert!(super::sanitize_leaf_filename("/etc/passwd").is_none());
        assert!(super::sanitize_leaf_filename("\\windows\\system32").is_none());
        assert!(super::sanitize_leaf_filename("foo/bar").is_none());
        assert!(super::sanitize_leaf_filename("foo\\bar").is_none());
        assert!(super::sanitize_leaf_filename("nul\0byte").is_none());
        assert!(super::sanitize_leaf_filename("").is_none());
        assert!(super::sanitize_leaf_filename("   ").is_none());
    }

    #[test]
    fn sanitize_leaf_filename_accepts_normal_names() {
        assert_eq!(
            super::sanitize_leaf_filename("report.pdf"),
            Some("report.pdf".to_string())
        );
        assert_eq!(
            super::sanitize_leaf_filename("My Document v1.docx"),
            Some("My Document v1.docx".to_string())
        );
        assert_eq!(
            super::sanitize_leaf_filename("  padded.txt  "),
            Some("padded.txt".to_string())
        );
        assert_eq!(
            super::sanitize_leaf_filename("plugin-1.0.lucidos-plugin"),
            Some("plugin-1.0.lucidos-plugin".to_string())
        );
    }

    fn nav(target: &str, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "target": target, "id": id })
    }

    #[test]
    fn a_nav_payload_alias_resolves_to_the_calling_thread() {
        let caller = Uuid::new_v4();
        for alias in ["current", "this", " Current "] {
            let mut payload = nav("thread", serde_json::json!(alias));
            super::resolve_thread_id_in_nav_payload(&mut payload, Some(caller))
                .unwrap_or_else(|e| panic!("{alias} must resolve, got: {e}"));
            assert_eq!(payload["id"], serde_json::json!(caller.to_string()));
        }
    }

    #[test]
    fn a_nav_payload_alias_is_refused_without_a_caller() {
        let mut payload = nav("thread", serde_json::json!("current"));
        let err = super::resolve_thread_id_in_nav_payload(&mut payload, None).expect_err("refused");
        assert!(err.contains("surface has none"), "got: {err}");
    }

    #[test]
    fn a_non_string_nav_thread_id_is_refused_rather_than_read_as_absent() {
        for bad in [
            serde_json::json!([Uuid::new_v4().to_string()]),
            serde_json::json!({ "id": "current" }),
            serde_json::json!(7),
        ] {
            let mut payload = nav("thread", bad.clone());
            let err = super::resolve_thread_id_in_nav_payload(&mut payload, Some(Uuid::new_v4()))
                .expect_err("a non-string must be refused");
            assert!(err.contains("must be a string"), "{bad} gave: {err}");
        }
    }

    #[test]
    fn a_repo_locator_splits_at_the_first_two_colons() {
        assert_eq!(
            super::split_repo_locator("repo:example-repo:file#feature/x:docs/a:b.md"),
            Some(("example-repo", "file#feature/x", "docs/a:b.md"))
        );
        assert_eq!(
            super::split_repo_locator("repo:r:diff#change-1:src/main.rs"),
            Some(("r", "diff#change-1", "src/main.rs"))
        );
    }

    fn repo(name: &str) -> crate::core::repositories::Repository {
        crate::core::repositories::Repository {
            id: Uuid::new_v4(),
            name: name.to_string(),
            path: format!("/home/user/src/{name}"),
            description: None,
            root_commit_sha: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_repository_matches_by_id_then_exact_name_then_any_case() {
        let (exact, folded) = (repo("Example-Repo"), repo("example-repo-two"));
        let repos = [exact.clone(), folded.clone()];
        assert_eq!(
            super::match_repository(&repos, &exact.id.to_string()),
            Ok(exact.id)
        );
        assert_eq!(
            super::match_repository(&repos, "Example-Repo"),
            Ok(exact.id)
        );
        assert_eq!(
            super::match_repository(&repos, "EXAMPLE-REPO-TWO"),
            Ok(folded.id)
        );
    }

    #[test]
    fn a_name_two_repositories_share_is_refused_but_an_exact_match_wins() {
        let (upper, lower) = (repo("App"), repo("app"));
        let repos = [upper.clone(), lower.clone()];
        assert_eq!(super::match_repository(&repos, "app"), Ok(lower.id));
        let err = super::match_repository(&repos, "APP").expect_err("ambiguous");
        assert!(err.contains("2 registered repositories"), "{err}");
        let err = super::match_repository(&repos, "unregistered-repo").expect_err("missing");
        assert!(err.contains("no registered repository"), "{err}");
    }

    #[test]
    fn a_malformed_repo_locator_is_not_one() {
        for not_one in [
            "artifacts/notes.md",
            "repo:example-repo",
            "repo::file:x.md",
            "repo:example-repo:file:",
            "repo:example-repo:file#:x.md",
            "repo:example-repo:docs/x.md",
            "repo:example-repo:blob:x.md",
        ] {
            assert_eq!(super::split_repo_locator(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn a_nav_payload_leaves_other_targets_and_an_absent_id_alone() {
        let mut idless = serde_json::json!({ "target": "thread" });
        super::resolve_thread_id_in_nav_payload(&mut idless, Some(Uuid::new_v4())).unwrap();
        assert!(idless.get("id").is_none());

        let mut trigger = nav("trigger", serde_json::json!("not-a-uuid"));
        super::resolve_thread_id_in_nav_payload(&mut trigger, None).unwrap();
        assert_eq!(trigger["id"], serde_json::json!("not-a-uuid"));

        let mut settings = serde_json::json!({ "target": "settings", "settings_view": "models" });
        super::resolve_thread_id_in_nav_payload(&mut settings, None).unwrap();
        assert_eq!(settings["settings_view"], serde_json::json!("models"));
    }

    #[test]
    fn a_malformed_optional_uuid_is_refused_not_read_as_absent() {
        assert_eq!(parse_optional_uuid(None), Ok(None));
        let id = Uuid::new_v4();
        assert_eq!(parse_optional_uuid(Some(&id.to_string())), Ok(Some(id)));
        assert_eq!(
            parse_optional_uuid(Some("cancel-one")),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn a_git_ref_git_reads_as_a_flag_or_a_shell_token_is_refused() {
        for dangerous in [
            "",
            "--upload-pack=evil",
            "-f",
            "main..feature",
            "a;rm",
            "a|b",
        ] {
            assert!(
                is_dangerous_git_ref(dangerous),
                "{dangerous:?} must be refused"
            );
        }
        for safe in ["main", "feature/x", "v1.2.3"] {
            assert!(!is_dangerous_git_ref(safe), "{safe} must pass");
        }
    }
}
