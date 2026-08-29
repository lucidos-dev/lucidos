pub mod actor;
mod app_ui;
mod apps;
mod artifacts;
pub(crate) mod backup;
pub(crate) mod base_path;
mod blobs;
pub(crate) mod browser_origin;
mod changes;
pub(crate) mod chat;
mod claude_code;
mod command_checkpoint;
mod command_permission;
pub(crate) mod credential_reveal;
mod data_api;
pub(crate) mod diff;
mod disk_usage;
pub(crate) mod error;
mod file_response;
mod frontend_preview;
pub(crate) mod frontend_snapshot;
pub(crate) mod handshake_scripts;
pub(crate) mod hex;
mod history;
mod images;
pub(crate) mod internal;
mod knowhow;
pub(crate) mod local_auth;
mod mcp;
mod mcp_permission;
mod memory;
mod notifications;
mod plugins;
mod presence;
pub mod presence_pong;
pub(crate) mod proxy;
pub(crate) mod proxy_auth_layer;
pub(crate) mod proxy_builtin;
pub(crate) mod proxy_hmac_layer;
pub mod proxy_migration;
pub(crate) mod proxy_pipeline;
pub(crate) mod proxy_pipeline_builder;
pub(crate) mod proxy_pipeline_config;
pub(crate) mod proxy_script_layer;
pub(crate) mod proxy_script_runner;
pub(crate) mod proxy_static_layers;
pub(crate) mod proxy_token_cache;
pub(crate) mod proxy_wasm_host;
pub(crate) mod proxy_wasm_signer;
mod release_notices;
mod repositories;
mod sdk;
mod sdk_fonts;
mod sdk_prefs;
mod search;
pub(crate) mod secret_reveal;
mod settings;
pub mod sse_connections;
pub(crate) mod target_workspace;
mod thread_queue;
pub(crate) mod thread_reach;
mod threads;
mod threads_compose;
mod trigger_groups;
mod triggers;
mod voice;
mod webhooks;
mod workspace_label;
mod ws_echo;

/// The `apis.json` read, re-exported because the engine binary reads it at
/// startup and `SystemEvent::ProxyConfigRejected` carries the refusals. The
/// rest of `proxy` stays crate-internal.
pub use proxy::{load_proxy_config, InsecureTransport, ProxyConfigLoad, RejectedProvider};
pub use proxy_builtin::local_upstream_base_url;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
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
use crate::core::{
    AppManager, ArtifactManager, ConversationSnapshot, CredentialInfo, EventStore, Model,
    OAuthAccountInfo, SessionMessage,
};
use crate::engine::{CaptureResult, LucidosEngine};
use crate::memory::{EmbedderSlot, PgVectorIndex};
use crate::scheduler::{Notification, PushSubscription, SchedulerManager};

pub(crate) use error::ApiError;

pub type SharedEngine = Arc<LucidosEngine>;

/// Pending OAuth flows keyed by provider name, each awaiting token exchange completion.
type PendingOAuthFlows =
    Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<OAuthFlowResult>>>>;

/// Canonical traversal guard, re-exported from `core` so every existing
/// `crate::api::is_path_traversal` call site keeps working while the engine has
/// a single implementation. See `crate::core::is_path_traversal`.
pub(crate) use crate::core::is_path_traversal;

/// Reject upload filenames that would escape their destination directory.
/// Multipart `Content-Disposition` filenames are attacker-controlled — without
/// this guard, a name like `../../etc/passwd` slipped into `dir.join(name)` or
/// `format!("imported/{name}")` would write outside the intended subtree.
/// Trims surrounding whitespace; the leaf invariant is what matters.
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

/// Parse an optional UUID query/body string, mapping malformed input to `BAD_REQUEST`.
///
/// Without this, handlers fall back to silently treating malformed ids as `None`
/// (e.g. "cancel one" becomes "cancel all") — see CLAUDE.md "no silent defaults".
pub(super) fn parse_optional_uuid(opt: Option<&str>) -> Result<Option<Uuid>, StatusCode> {
    opt.map(Uuid::parse_str)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)
}

/// Trim-then-parse for an optional UUID coming from a JSON body or LLM tool
/// argument. Treats `None` / empty / whitespace-only as `Ok(None)`; otherwise
/// returns the parsed UUID or `Err(raw)` carrying the offending input so the
/// caller can build a context-appropriate error message (the HTTP handler
/// returns 400 with the value; the LLM tool returns it in a tool-error).
pub(crate) fn parse_optional_uuid_trimmed(opt: Option<&str>) -> Result<Option<Uuid>, String> {
    match opt.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|_| s.to_string()),
    }
}

/// Reject refs/commits that git would parse as a flag, traverse with `..`, or contain
/// shell metacharacters. The git invocations themselves use argv (no shell), so these
/// checks defend against ref-as-flag injection and against passing the value through
/// any future shell-quoting layer.
pub(super) fn is_dangerous_git_ref(s: &str) -> bool {
    s.is_empty() || s.starts_with('-') || s.contains("..") || s.contains(';') || s.contains('|')
}

/// Map a file extension to its MIME content type.
fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
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
        _ => "application/octet-stream",
    }
}

/// Application state containing engine and shared read-only resources.
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
    /// Pending OAuth flows keyed by provider — the receiver resolves when the
    /// background listener receives the callback and completes token exchange.
    pub pending_oauth_flows: PendingOAuthFlows,
    /// Live one-shot secret-reveal tokens, shared by the credential reveal and
    /// the backup key. Ephemeral by design; see `api::secret_reveal`.
    pub reveal_tokens: secret_reveal::RevealTokens,
}

/// App context sent when an app UI is open — tells the LLM which app is active.
#[derive(Debug, Clone, Deserialize)]
pub struct AppContext {
    pub app_id: String,
}

/// File context sent when a file preview window is focused
#[derive(Debug, Clone, Deserialize)]
pub struct FileContext {
    pub path: String,
    /// Inclusive 1-based line range the user has selected in the preview, when
    /// they picked one. Same shape and meaning as `RepoFileContext::lines`.
    #[serde(default)]
    pub lines: Option<(u32, u32)>,
}

/// Repo file context sent when the user is viewing a file from a registered repo
#[derive(Debug, Clone, Deserialize)]
pub struct RepoFileContext {
    pub repo_id: String,
    pub path: String,
    #[serde(default)]
    pub lines: Option<(u32, u32)>,
}

/// URL context sent when the user has a webpage open in the Tauri panel webview
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlContext {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    pub content: String,
}

/// An image pasted by the user, sent inline as base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImage {
    pub base64: String,
    pub mime_type: String,
}

/// Maximum base64-encoded image size accepted by LLM APIs (Claude: 5 MB).
/// We target 4.5 MB to leave margin for encoding overhead.
pub(crate) const MAX_IMAGE_BASE64_BYTES: usize = 4_500_000;

/// Maximum dimension (width or height) for images sent to LLMs.
/// Larger images waste tokens without improving understanding.
const MAX_IMAGE_DIMENSION: u32 = 2048;

impl ChatImage {
    /// Always compress images for LLM consumption: re-encode as JPEG (quality 85)
    /// and cap dimensions at `MAX_IMAGE_DIMENSION`. This reduces token usage and
    /// ensures images stay under the API's 5 MB limit.
    pub fn compress(self) -> Self {
        use base64::Engine as _;
        use image::GenericImageView;

        let raw = match base64::engine::general_purpose::STANDARD.decode(&self.base64) {
            Ok(bytes) => bytes,
            Err(e) => {
                crate::log!("[Image] base64 decode failed, skipping compression: {}", e);
                return self;
            }
        };

        let img = match image::load_from_memory(&raw) {
            Ok(img) => img,
            Err(e) => {
                crate::log!("[Image] image decode failed, skipping compression: {}", e);
                return self;
            }
        };

        let img = crate::core::blobs::apply_exif_orientation(&raw, img);

        let (orig_w, orig_h) = img.dimensions();
        let orig_base64_len = self.base64.len();

        // Scale down if either dimension exceeds the cap
        let img = if orig_w > MAX_IMAGE_DIMENSION || orig_h > MAX_IMAGE_DIMENSION {
            img.resize(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img
        };

        let (new_w, new_h) = img.dimensions();

        // Encode as JPEG — always, even if already JPEG (re-encoding normalizes quality)
        let mut jpeg_buf = std::io::Cursor::new(Vec::new());
        if let Err(e) = img.write_to(&mut jpeg_buf, image::ImageFormat::Jpeg) {
            crate::log!("[Image] JPEG encoding failed: {}", e);
            return self;
        }

        let mut jpeg_bytes = jpeg_buf.into_inner();
        let mut encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

        // If still over the limit, progressively reduce dimensions
        let mut scale: f64 = 0.8;
        while encoded.len() > MAX_IMAGE_BASE64_BYTES && scale > 0.1 {
            let sw = ((new_w as f64) * scale).max(1.0) as u32;
            let sh = ((new_h as f64) * scale).max(1.0) as u32;
            let resized = img.resize(sw, sh, image::imageops::FilterType::Lanczos3);
            let mut buf = std::io::Cursor::new(Vec::new());
            if resized.write_to(&mut buf, image::ImageFormat::Jpeg).is_ok() {
                jpeg_bytes = buf.into_inner();
                encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
            }
            scale *= 0.7;
        }

        crate::log!(
            "[Image] {}x{} ({} KB) → {}x{} ({} KB)",
            orig_w,
            orig_h,
            orig_base64_len / 1024,
            new_w,
            new_h,
            encoded.len() / 1024
        );

        ChatImage {
            base64: encoded,
            mime_type: "image/jpeg".to_string(),
        }
    }

    /// Ensure this image's base64 payload fits the LLM's per-image size target,
    /// compressing (JPEG re-encode + downscale via [`compress`]) only when it's
    /// over. Images already within budget pass through untouched — no re-encode,
    /// no quality loss, original format preserved.
    ///
    /// This is the single decision point for "is this image small enough to send
    /// to a model". Every LLM-bound image path routes through it — chat message
    /// blocks, the image-description pass, and `read_file` — so the
    /// compress-or-skip rule lives in exactly one place and an oversized photo
    /// can't reach a provider's hard limit (Claude rejects images over 5 MB).
    ///
    /// [`compress`]: ChatImage::compress
    pub(crate) fn fit_for_llm(self) -> Self {
        if self.base64.len() <= MAX_IMAGE_BASE64_BYTES {
            self
        } else {
            self.compress()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AppCaptureRequest {
    pub request_id: String,
    pub screenshot: String, // base64-encoded image (JPEG from html2canvas; format sniffed downstream)
    pub dom: String,
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    /// Mandatory. Semantic mode of the actor that authored this message —
    /// `"human"` (a real person, the default for typical chat), `"agent"`
    /// (LLM-driven, e.g. one thread spawning another via the `run_thread`
    /// tool or a cross-workspace agent call), or `"engine"` (engine-internal,
    /// e.g. recovery / scheduler).
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
    #[serde(default)]
    pub images: Option<Vec<ChatImage>>,
    /// Forward-compat: when set, the handler resolves each hash against the
    /// workspace blob store and uses those bytes for the LLM call. Frontends
    /// that have already uploaded their images via `POST /threads/:id/blobs`
    /// can send hash refs only — keeping this body small even on cellular.
    /// Mutually exclusive with `images`.
    #[serde(default)]
    pub image_hashes: Option<Vec<String>>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// True when this request spawns / continues a *coding-agent thread* (any
    /// backend), as opposed to a chat thread answered by the Lucidos Agent.
    /// The `coding_agent` field below picks the backend. Aliased to the legacy
    /// `use_claude_code` key so payloads persisted before the rename (queued
    /// `ThreadQueueRequest` rows, in-flight clients) still deserialize.
    #[serde(default, alias = "use_claude_code")]
    pub use_coding_agent: Option<bool>,
    #[serde(default)]
    pub cc_model: Option<String>,
    /// Which coding-agent backend a NEW coding-agent thread should run on
    /// (`claude-code` | `codex`). Requires `use_coding_agent: true`; ignored
    /// on follow-ups — the thread's stored backend wins (locked at first
    /// SessionStarted).
    #[serde(default)]
    pub coding_agent: Option<crate::runtime::CodingAgent>,
    #[serde(default, alias = "message_id")]
    pub event_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// `true` when `thread_id` names a thread that is EXPECTED not to exist yet,
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
    /// Required when `mode` is `"agent"` or `"engine"`: the thread that is
    /// spawning this new thread (e.g. the Claude Code session whose `spawn-thread` skill
    /// is making this call). Must be `null` when `mode` is `"human"`. The
    /// `spawn-thread` skill reads `$LUCIDOS_THREAD_ID` and forwards it here.
    #[serde(default)]
    pub parent_thread_id: Option<String>,
    /// The specific event in the parent thread that triggered this spawn —
    /// e.g. the `ToolCalled` event for a `run_thread` tool invocation, or
    /// the `CodingAgentToolCalled` event for a Bash call that ran the
    /// `spawn-thread` skill. Allowed only when `mode` is `"agent"` or
    /// `"engine"`. Lets audits trace exactly which step in the parent thread
    /// caused this spawn.
    #[serde(default)]
    pub spawning_event_id: Option<String>,
    /// Cross-workspace origin: name of the calling workspace (e.g. `"dev"`,
    /// `"myws"`). When set, the receiver constructs
    /// `MessageOrigin::Workspace` from the three `caller_*` fields. Mutually
    /// exclusive with `parent_thread_id` / `spawning_event_id` — those are
    /// for *same-workspace* parent-child spawns (with callback). Cross-workspace
    /// is fire-and-forget; no callback path exists, so "parent" wording would
    /// be misleading.
    #[serde(default)]
    pub caller_workspace: Option<String>,
    /// Cross-workspace origin: UUID of the thread in the calling workspace
    /// that initiated this POST. Allowed only when `caller_workspace` is set.
    #[serde(default)]
    pub caller_thread_id: Option<String>,
    /// Cross-workspace origin: UUID of the event in the calling workspace
    /// that triggered this POST (e.g. the `ToolCalled` event for a
    /// `lucidos spawn-thread` invocation). Allowed only when `caller_workspace`
    /// is set. Often `None` from Claude Code subprocesses, which lack access to their
    /// own tool-call event id.
    #[serde(default)]
    pub caller_event_id: Option<String>,
    #[serde(default)]
    pub conflict_change_id: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
    /// Scope-picker payload: an absolute path, a workspace-relative path
    /// (`data/apps/<id>`), or a registered repo name/UUID. Resolved via the
    /// shared `coding_agent_kind` pipeline to one of `lucidos | app | external`
    /// and translated to (repo_id, app stash) by the chat handler. Mutually
    /// exclusive with `repo_id`; supplying both is a 400. Frontend sends this
    /// going forward; `repo_id` stays accepted for back-compat (CLI, older
    /// frontends). An empty / missing folder falls through to today's default
    /// (Lucidos when `repo_id` is also empty / missing).
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Body of `POST /api/v1/chat/cancel` and the default (Stop) mode of
/// `POST /api/v1/claude-code/stop`. `canceled = true` when the call canceled
/// live work, settled a stuck `running` projection, or cancel-stamped a pending
/// question — i.e. a status-changing event is on its way over SSE. `false` when
/// the server had nothing to cancel: the client's optimistic "canceling" state
/// is stale and it must re-sync the thread (the uncancelable-thread wedge fix).
/// HTTP status stays 200 either way — the bool is additive.
#[derive(Serialize)]
pub struct CancelResponse {
    pub canceled: bool,
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

// Credentials types
#[derive(Serialize)]
pub struct CredentialsListResponse {
    pub credentials: Vec<CredentialInfo>,
}

/// The *credential scope* a create or update body declares.
///
/// `base_urls` is the field. `base_url` is permanent back-compat for a caller
/// written before the scope became a set, and [`RequestedScope::declared`]
/// folds it into the list. Both present is not an error: the singular joins the
/// set, which is what adding a host to an old body means.
#[derive(Deserialize, Default)]
pub struct RequestedScope {
    #[serde(default)]
    pub base_urls: Option<Vec<String>>,
    #[serde(default)]
    pub base_url: Option<String>,
}

impl RequestedScope {
    /// Every base URL the body asked for, or `None` when it named NEITHER
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
    /// Optional custom env var name for the injected secret (e.g. `GITHUB_TOKEN`
    /// instead of `CRED_<NAME>`). Validated like a user env var name.
    #[serde(default)]
    pub env_var_name: Option<String>,
}

/// Body of `PUT /api/v1/credential-base-urls?id=<uuid>`, which replaces one
/// credential's scope and nothing else.
///
/// Narrow on purpose. The whole-row edit needs the auth type, and it defaults
/// the auth header. So a script widening a scope through it can clobber fields
/// it never meant to name.
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
    /// New secret. `None` / empty string keeps the currently-stored secret —
    /// lets the user edit non-secret fields without re-entering it.
    #[serde(default)]
    pub auth_value: Option<String>,
    /// Editable email server settings, present only when editing an
    /// `email_password` credential. The credential's `email_accounts` row is
    /// the source of truth for IMAP/SMTP, so it must be kept in sync here.
    #[serde(default)]
    pub email: Option<EmailAccountSettings>,
    /// Optional custom env var name for the injected secret. `None`/empty clears
    /// it back to the default `CRED_<NAME>` form. Validated like a user env var.
    #[serde(default)]
    pub env_var_name: Option<String>,
}

/// Email server settings carried by an `UpdateCredentialRequest` for
/// `email_password` credentials. Mirrors the columns `configure_email` writes.
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

/// Deserialize an `Option<T>` field into `Option<Option<T>>` so a handler can
/// tell "key absent" (`None`) from "key present and null" (`Some(None)`).
///
/// `#[serde(default)]` alone collapses both to `None`. Pair this with
/// `#[serde(default)]` on a nullable PATCH field whose `null` must mean *clear
/// it* rather than *leave it alone*.
pub fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

// Model registry types

/// A registry row plus what the engine derives about it. Flattened, so the wire
/// shape is the `Model` row's own fields with the derived ones alongside.
///
/// `reasoning_efforts` exists so the Lucidos Agent picker filters against the
/// tiers the engine will actually send rather than deriving its own answer from
/// the model id. Both used to derive it separately and disagreed, which is how
/// a local model was offered a tier its server rejected. See
/// `llm::reasoning::supported_efforts`, the single source of truth.
#[derive(Serialize)]
pub struct ModelInfo {
    #[serde(flatten)]
    pub model: Model,
    pub reasoning_efforts: &'static [&'static str],
}

#[derive(Serialize)]
pub struct ModelsListResponse {
    pub models: Vec<ModelInfo>,
}

#[derive(Deserialize)]
pub struct CreateModelRequest {
    pub id: String,
    /// Display name. Optional — an absent/empty label defaults to the id, matching
    /// the `manage_models` LLM handler and the `lucidos models add` CLI (whose
    /// `--label` is optional). The Settings UI always sends one.
    #[serde(default)]
    pub label: String,
    /// Backend that serves the model: "vertex" | "anthropic" | "openai" |
    /// "openrouter" | "xai" | "opencode-free" | "local". Validated by
    /// `settings::valid_provider`.
    pub provider: String,
    /// Display order; omitted user models sort after the builtins.
    #[serde(default)]
    pub sort_order: Option<i32>,
    /// Context window in tokens. Omit to let the engine infer it from the model
    /// id — only worth setting for ids the id-shape fallback gets wrong (every
    /// OpenRouter / xAI / Gemini / local model, which otherwise takes 200k).
    #[serde(default)]
    pub context_window: Option<i32>,
}

/// PUT body for a model. For builtin rows only `enabled` is applied (disable-only);
/// for user rows any provided field is updated, omitted fields keep their value.
#[derive(Deserialize)]
pub struct UpdateModelRequest {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Context window in tokens. Absent keeps the stored value; an explicit
    /// `null` CLEARS it, handing the model back to the id-shape fallback. The
    /// double `Option` is what distinguishes those two cases — serde maps a
    /// missing key to `None` and a literal `null` to `Some(None)`.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub context_window: Option<Option<i32>>,
}

/// The engine's read-back on a trigger's cron after a create or update: the fire
/// times it computed, plus any non-fatal advice. Set only by the trigger write
/// endpoints. A schedule that can never fire is a hard error instead, so a
/// preview with an empty `next_runs` means the trigger has no cron at all.
#[derive(Serialize)]
pub struct CronPreview {
    /// The next few upcoming fire times (RFC3339, in the trigger's timezone),
    /// merged across the whole expression array under OR semantics.
    pub next_runs: Vec<String>,
    /// Non-fatal warnings, e.g. the day-of-month/day-of-week AND footgun.
    pub warnings: Vec<String>,
}

impl CronPreview {
    /// Project the engine's validation result onto the wire shape. Lives here
    /// rather than on `ValidatedCron` so the scheduler stays unaware of the HTTP
    /// layer.
    pub(crate) fn from_validated(
        validated: &crate::engine::tools::scheduler::ValidatedCron,
    ) -> Self {
        Self {
            next_runs: validated.next_runs_rfc3339(),
            warnings: validated.warnings.clone(),
        }
    }
}

/// Generic success/error response used by credential, preference, and trigger endpoints.
///
/// `Default` is what the constructors below build on, so a new optional field
/// costs one line here instead of an edit to every one of them.
#[derive(Serialize, Default)]
pub struct ApiResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When set, the frontend should open the credential request form with this data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_request: Option<serde_json::Value>,
    /// When set, the frontend should open this URL for OAuth authorization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// When set, the next fire times and cron warnings for the trigger this call
    /// just wrote. Trigger create / update only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_preview: Option<CronPreview>,
    /// Non-fatal notes about the write that just succeeded. Today: an event
    /// type in a trigger's `on` list that this workspace has never emitted.
    ///
    /// Separate from `cron_preview.warnings`, which is about the schedule. A
    /// trigger with no cron has no preview to hang these on.
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
    /// Success for a trigger write, carrying both read-backs the caller should
    /// surface: the cron preview, and any event-type warnings.
    ///
    /// One constructor rather than two, so an update that only rewrote the
    /// `on:` list cannot drop its warnings for want of a cron preview.
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
    /// No OAuth client registered yet: hand the modal a request, prefilled from
    /// the *OAuth provider registry* when the provider is a known one.
    ///
    /// `overrides` used to be `OAuthClientOverrides::default()` unconditionally,
    /// which is what made the Settings Connect button strictly worse than asking
    /// the *Lucidos Agent*: the agent looked the endpoints up in knowhow and
    /// passed them, while this path emitted no `defaults` block at all and left
    /// the user to type a provider's authorization and token URLs by hand.
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

    /// An OAuth client exists but cannot drive a flow. Same modal, targeted at
    /// the existing row so the save repairs it instead of creating a duplicate.
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

// OAuth account types
#[derive(Serialize)]
pub struct OAuthAccountsListResponse {
    pub accounts: Vec<OAuthAccountInfo>,
}

#[derive(Deserialize)]
pub struct OAuthAccountQuery {
    pub id: String,
}

// Device types
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

/// Claim that this caller used to be `old_device_id`.
///
/// `device_id` is where it is now. The client sends both because it is the only
/// party that can see both at once, for the one page load that spans the
/// change. See `core::devices::DeviceStore::hand_over`.
#[derive(Deserialize)]
pub struct DeviceHandOverRequest {
    pub old_device_id: String,
    pub device_id: String,
}

#[derive(Serialize)]
pub struct DevicesListResponse {
    pub devices: Vec<crate::core::devices::Device>,
}

// Preferences types
#[derive(Serialize)]
pub struct PreferencesResponse {
    pub preferences: std::collections::HashMap<String, String>,
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
    /// When true, broadcast on SSE but skip the events table write.
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

// Query param structs for routes migrated from path params

/// Addresses one credential row by its primary key.
///
/// The credential verbs that act on an EXISTING row (copy-value, update,
/// delete) take this rather than a service name, because a name stopped being a
/// unique handle when `auth_type` became the discriminator
/// (`20260805134838_drop_credential_name_prefixes_use_auth_type.sql`): an
/// `oauth_client` registration may share a name with an API key. Update needs
/// the id for a sharper reason still, since the edit form can change
/// `auth_type`, so even `(name, type)` cannot name the row being edited. Create
/// stays name-keyed: it is an upsert, and the row has no id yet.
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

/// `?id=<model-id>` for the model-registry PUT/DELETE routes. The id is the
/// model string (e.g. `claude-fable-5`), not a UUID — distinct from
/// `NotificationQuery`.
#[derive(Deserialize)]
pub(super) struct ModelIdQuery {
    pub id: String,
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
    /// Unix-timestamp cursor for pagination (created_at < before)
    before: Option<f64>,
    /// "all" (default) or "unread"
    #[serde(default)]
    filter: Option<String>,
}

/// Parse a Unix timestamp (with sub-second precision) into a `DateTime<Utc>`.
/// Used by paginated query endpoints that accept a `before` cursor.
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
    /// Unix-timestamp cursor for pagination (resolved_at < before)
    before: Option<f64>,
}

#[derive(Deserialize)]
struct EventQuery {
    event: Uuid,
}

#[derive(Deserialize)]
struct SessionMessagesQuery {
    id: String,
}

/// Git version information for an app commit
#[derive(Serialize)]
struct GitVersion {
    commit: String,
    message: String,
    timestamp: i64,
    author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
}

/// Log every API request with method, path, status, and duration.
/// Skips noisy endpoints (health checks, the SSE stream) to keep logs readable.
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

    log!("[API] {} {} → {} ({:.0?})", method, path, status, elapsed);

    response
}

/// Serve the built frontend (`dist/`) for an unmatched request.
///
/// Real bundled files (`/assets/*`, `/sw.js`, `/manifest.json`, `/favicon*`, …)
/// stream straight from disk via `ServeDir`. The SPA shell — `/`, `/index.html`,
/// or any navigation that doesn't resolve to a file — is served as `index.html`
/// with `<base href>` stamped from `X-Forwarded-Prefix` (ADR 0014 §4), so the
/// bundle's relative refs resolve under the workspace prefix behind the gateway.
async fn serve_frontend(static_dir: PathBuf, req: axum::extract::Request) -> Response {
    // The SPA shell and static assets are only ever served for read requests.
    // A non-GET/HEAD request that reaches this fallback hit no API/app/data route,
    // so the path simply doesn't exist — return 404. Without this gate the request
    // falls through to `ServeDir`, which answers any non-GET with 405 Method Not
    // Allowed, leaking a misleading "method not allowed" for an unknown path (e.g.
    // a stray `POST /api/internal/...` that dropped the `/v1/`).
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }

    let prefix = base_path::forwarded_prefix(req.headers());
    // Own the path because `ServeDir::oneshot` consumes the request before the
    // missing-asset branch decides whether SPA fallback is legal.
    let path = req.uri().path().to_string();

    if path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    // The shell: root or an explicit index request → stamped index.html.
    if path == "/" || path == "/index.html" {
        return serve_shell(&static_dir, &prefix);
    }

    // Otherwise try the path as a real asset; a 404 means it's a client-side
    // route (hash routing keeps these rare) → fall back to the stamped shell.
    let service = ServeDir::new(&static_dir);
    match service.oneshot(req).await {
        Ok(resp) if resp.status() != StatusCode::NOT_FOUND => resp.map(axum::body::Body::new),
        _ if path.starts_with("/assets/") => StatusCode::NOT_FOUND.into_response(),
        _ => serve_shell(&static_dir, &prefix),
    }
}

/// Read `index.html` from `static_dir`, stamp `<base href>` for `prefix` and the
/// gateway port (when behind a gateway), return it as `text/html`. The gateway
/// port lets a page served on the engine's own port (direct access) build an
/// absolute URL to the gateway picker — its origin differs from the engine's, so
/// the relative `/~/` route can't reach it (ADR 0014).
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

// Top-level wiring helper that takes every engine subsystem the router needs;
// reducing it would force the call site to thread the same set into a holder.
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
    // What this engine is about to bind. It decides whether callers must
    // present a credential, so it is resolved before the router is built rather
    // than at listen time. See `api::local_auth`.
    bind_choice: &crate::net_config::BindChoice,
) -> Router {
    // Initialize AppManager
    let app_manager =
        Arc::new(AppManager::new(&workspace_path).expect("Failed to initialize AppManager"));

    // Resolve + pin the served frontend (dev) BEFORE `engine` is moved into
    // `AppState`. The served dir lives behind a swappable handle so a
    // frontend-only Apply can re-snapshot `dist/` and advance what we serve
    // in-process (no respawn) — see `frontend_snapshot` + `engine::frontend_refresh`.
    // `None` when there's no `LUCIDOS_STATIC_DIR` (headless API-only).
    let served_frontend_handle: Option<Arc<std::sync::RwLock<PathBuf>>> =
        std::env::var_os("LUCIDOS_STATIC_DIR").map(|static_dir| {
            // Dev pins a private snapshot of `dist/` so the running engine only
            // ever serves the client it was built against — never a newer,
            // possibly incompatible client (incl. on a hard reload). Packaged
            // serves the bundled Resources dir unchanged. See `frontend_snapshot`.
            let source = PathBuf::from(&static_dir);
            let served = frontend_snapshot::resolve_served_dir(source.clone(), &workspace_path);
            let handle = Arc::new(std::sync::RwLock::new(served));
            // Register the handle + source on the engine so a frontend-only Apply
            // can swap it. No-op registration in packaged (the engine's refresh
            // path gates on `!is_packaged()` and never swaps).
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
    };

    // Serve static files from the data/ tree. One mount covers every
    // subdirectory, gated by the same read allowlist `GET /api/v1/data/*path`
    // applies. See `data_api::serve_workspace_data`.
    let serve_data = data_api::static_mount(workspace_path.join(crate::core::DATA_DIR));

    // Clone for the top-level app-UI router; the /api/v1 router below
    // consumes `state` directly.
    let app_ui_state = state.clone();

    // ALL routes live under `/api/v1/`: see `.claude/rules/rust.md`
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
    let api_routes = Router::new()
        .merge(history::router())
        .merge(chat::router())
        .merge(claude_code::router())
        .merge(threads::router())
        .merge(changes::router())
        .merge(notifications::router())
        .merge(artifacts::router())
        .merge(settings::router())
        .merge(credential_reveal::router())
        .merge(triggers::router())
        .merge(trigger_groups::router())
        .merge(thread_queue::router())
        .merge(knowhow::router())
        .merge(presence::router())
        .merge(presence_pong::router())
        .merge(memory::router())
        .merge(apps::router())
        .merge(command_permission::router())
        .merge(command_checkpoint::router())
        .merge(mcp::router())
        .merge(mcp_permission::router())
        .merge(internal::router())
        .merge(backup::router())
        .merge(disk_usage::router())
        .merge(voice::router())
        .merge(ws_echo::router())
        .merge(frontend_preview::router())
        .merge(search::router())
        .merge(release_notices::router())
        .merge(repositories::router())
        .merge(sdk::router())
        .merge(sdk_fonts::router())
        .merge(sdk_prefs::router())
        .merge(data_api::router())
        .merge(blobs::router())
        .merge(plugins::router())
        .merge(webhooks::router())
        .merge(workspace_label::router())
        .merge(handshake_scripts::router())
        .merge(proxy::router())
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        // Axum's 2 MiB default rejects mobile screenshots in chat/app-capture
        // bodies — and large `PUT /api/v1/data/*path` binary writes — with
        // "Failed to buffer the request body". `Router::layer` only covers
        // routes registered before the call, so this is applied after every
        // domain merge to guarantee it reaches all routes.
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        // Compress JSON responses (br/gzip per Accept-Encoding). The thread
        // events snapshot (`GET /threads/:id/events`) ships the full event
        // history uncompressed today — a heavy coding-agent thread is multiple
        // MB on every open, dominating load latency over Tailscale / on mobile.
        // The default predicate leaves the SSE stream untouched: it excludes
        // `text/event-stream` (the plain SSE path) and skips responses that
        // already carry `content-encoding` (the hand-rolled gzipped SSE in
        // `history.rs`), so the existing streaming transport is unchanged.
        // `compression_predicate` extends it to leave media alone, which is what
        // keeps `Accept-Ranges` on a data-file response.
        .layer(CompressionLayer::new().compress_when(compression_predicate()))
        // Refuse a request that named a DIFFERENT workspace than this engine
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
        .with_state(state);

    // Refuse a browser request that came from another origin (403), in front of
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
        crate::log!("[API] permissive CORS is on, so the same-origin gate is off with it");
        api_routes
    } else {
        api_routes.layer(axum::middleware::from_fn(browser_origin::enforce))
    };

    let router = Router::new()
        .nest("/api/v1", api_routes)
        .nest("/app", apps::ui_router().with_state(app_ui_state))
        .nest_service("/data", serve_data);

    // Unmatched (non-API, non-/app, non-/data) requests resolve the frontend
    // from the pre-built `dist/` at LUCIDOS_STATIC_DIR — the SAME serving path
    // in dev and packaged (ADR 0014 §4/§5; the dev Vite reverse-proxy is gone).
    // Real bundled assets stream straight from disk; the SPA shell (`/`,
    // `/index.html`, and any non-file navigation) is served with `<base href>`
    // stamped from the gateway's `X-Forwarded-Prefix` so the bundle's relative
    // asset refs resolve back through the gateway to this workspace. With no
    // header (direct hit / `LUCIDOS_NO_GATEWAY`) the base is `/`, identical to
    // before. No LUCIDOS_STATIC_DIR → keep the default 404 (headless API-only).
    // The served dir was resolved + pinned above (`served_frontend_handle`); read
    // the CURRENT snapshot per request so a frontend-only Apply that swapped in a
    // newer, engine-compatible generation is picked up without a respawn.
    let router = if let Some(handle) = served_frontend_handle {
        router.fallback(move |req: axum::extract::Request| {
            let static_dir = handle.read().unwrap().clone();
            serve_frontend(static_dir, req)
        })
    } else {
        router
    };

    // Prevent heuristic caching on ALL engine responses. `no-cache` means
    // "always revalidate with the server" — the browser still gets 304s when
    // files haven't changed, but never serves stale data after edits.
    // `if_not_present` preserves explicit headers (e.g. no-store on app UIs,
    // max-age on immutable static assets).
    let router = router
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        // Who is calling, on an engine that faces a network. Layered out here
        // rather than inside the `/api/v1` nest. `/app`, `/data` and the
        // frontend fallback are siblings of that nest, so a gate inside it
        // would miss all three. `/data` serves the workspace's files straight
        // off disk, which would be the whole leak.
        //
        // Inert on a loopback bind, which is every shipped topology: the layer
        // returns immediately and nothing about today's behaviour changes.
        //
        // Deliberately NOT skipped for `LUCIDOS_PERMISSIVE_CORS`, unlike the
        // origin gate below. That variable says this deployment wants
        // cross-origin browser access. It does not say it wants no
        // authentication, and reading it that way would make an escape hatch
        // for one concern silently open another.
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(local_auth::EngineAuth::resolve(bind_choice)),
            local_auth::enforce,
        ))
        .layer(axum::middleware::from_fn(request_logger));

    if permissive_cors_enabled() {
        router.layer(CorsLayer::permissive())
    } else {
        router
    }
}

/// Compress text-shaped payloads only. Already-compressed media gains nothing
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
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Absence and emptiness are different answers, and the update verb acts on
    /// the difference. A body that names no scope leaves the stored one alone;
    /// one that names `[]` clears it. Without the distinction, a `PUT` changing
    /// only the auth header emptied the set and every call through the
    /// credential started failing.
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

    /// The singular field is permanent back-compat, and it JOINS the set rather
    /// than replacing it. A caller written before the scope became a set adds a
    /// host by sending both.
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

    /// Compressing a media response would strip `Accept-Ranges`, so the
    /// exclusion list is what makes the data route seekable in a browser.
    #[test]
    fn compression_skips_media_and_still_covers_json() {
        // Above `SizeAbove`'s 32-byte floor, so content type is what decides.
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
        for value in [Some("1"), Some("true"), Some("yes"), Some("on")] {
            assert!(permissive_cors_enabled_value(value), "value: {value:?}");
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
        // Always converts to JPEG
        assert_eq!(result.mime_type, "image/jpeg");
        // Result is valid base64 that decodes to a JPEG
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&result.base64)
            .unwrap();
        assert!(decoded.starts_with(&[0xFF, 0xD8]), "must be valid JPEG");
    }

    #[test]
    fn large_dimensions_capped() {
        let img = make_test_image(4000, 3000);
        let result = img.compress();
        // Verify the result is under the limit
        assert!(
            result.base64.len() <= MAX_IMAGE_BASE64_BYTES,
            "must be under limit: {} bytes",
            result.base64.len()
        );
        // Verify dimensions were reduced
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

    /// Pins the security contract of the canonical traversal guard used by every
    /// path-accepting endpoint (data_api, proxy script runner, script triggers,
    /// import/image/email tools, …). A regression here is a path-escape bug.
    #[test]
    fn is_path_traversal_pins_security_contract() {
        // Plain relative paths within the tree are allowed.
        assert!(!is_path_traversal("scripts/auth/handshake.py"));
        assert!(!is_path_traversal("foo.py"));
        assert!(!is_path_traversal("a/b/c/d.txt"));

        // Parent-dir escapes are blocked.
        assert!(is_path_traversal("../escape"));
        assert!(is_path_traversal("../../etc/passwd"));

        // Absolute paths are blocked (Unix `/` and Windows-style `\`): joining
        // an absolute path onto the workspace root discards the prefix, so
        // `workspace.join("/etc/passwd")` would resolve to `/etc/passwd`.
        assert!(is_path_traversal("/etc/passwd"));
        assert!(is_path_traversal("\\windows\\system32"));

        // An embedded `..` segment is blocked even though it might normalize
        // back inside the tree — the guard fails closed on any `..` rather than
        // resolving the path first.
        assert!(is_path_traversal("foo/../bar"));

        // Conservative by design: `..` anywhere in the string blocks, including
        // a legitimate-looking filename like `a..b` (a false positive, but it
        // fails closed, which is safe). Pinned so the over-broad behavior stays
        // a deliberate contract rather than something a future refactor
        // "tightens" into a vulnerability.
        assert!(is_path_traversal("weird..name.txt"));

        // The guard operates on the raw string and does NOT percent-decode, so
        // a URL-encoded `%2e%2e` is a literal directory name, not `..`. That is
        // correct here: callers (e.g. the proxy script path from `apis.json`)
        // never percent-decode before this check, so `%2e%2e` resolves to a real
        // (non-existent) directory rather than an escape.
        assert!(!is_path_traversal("%2e%2e/escape"));
    }

    #[test]
    fn oversized_image_compressed_under_limit() {
        // Create a large image that will produce > 5 MB base64
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
        // Under the per-image target → no re-encode, original bytes + mime kept.
        let img = ChatImage {
            base64: "AAAA".to_string(),
            mime_type: "image/png".to_string(),
        };
        let result = img.fit_for_llm();
        assert_eq!(result.base64, "AAAA");
        assert_eq!(result.mime_type, "image/png");
    }

    #[test]
    fn fit_for_llm_compresses_oversized_image() {
        // A large image (base64 over the target) is downscaled + JPEG-re-encoded
        // until it fits, so it can never reach a provider's hard size limit.
        // Noise resists PNG compression (a smooth gradient would shrink below the
        // target and not exercise the compress path), so paint per-pixel from a
        // coordinate hash to keep the encoded payload large.
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

    /// Build a minimal JPEG with an EXIF APP1 segment containing the given
    /// orientation tag. The pixel data is a landscape `width`×`height` image,
    /// but EXIF orientation tells viewers to rotate it.
    fn make_jpeg_with_exif_orientation(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        // Encode a landscape image as raw JPEG first (RGB — JPEG doesn't support alpha)
        let img_buf: ImageBuffer<image::Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(width, height, |x, y| {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            });
        let mut jpeg_buf = std::io::Cursor::new(Vec::new());
        img_buf
            .write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg_bytes = jpeg_buf.into_inner();

        // Build EXIF APP1 segment with orientation tag (little-endian TIFF)
        let orient_lo = (orientation & 0xFF) as u8;
        let orient_hi = ((orientation >> 8) & 0xFF) as u8;
        #[rustfmt::skip]
        let exif_payload: Vec<u8> = vec![
            // "Exif\0\0"
            0x45, 0x78, 0x69, 0x66, 0x00, 0x00,
            // TIFF header: little-endian
            0x49, 0x49, // "II" = little-endian
            0x2A, 0x00, // TIFF magic (42)
            0x08, 0x00, 0x00, 0x00, // offset to first IFD (8 bytes from TIFF start)
            // IFD0
            0x01, 0x00, // 1 entry
            // Entry: Orientation (tag 0x0112)
            0x12, 0x01, // tag
            0x03, 0x00, // type: SHORT
            0x01, 0x00, 0x00, 0x00, // count: 1
            orient_lo, orient_hi, 0x00, 0x00, // value
            0x00, 0x00, 0x00, 0x00, // next IFD offset (none)
        ];

        let app1_len = (exif_payload.len() + 2) as u16; // +2 for the length field itself
        let mut result = Vec::new();
        result.extend_from_slice(&[0xFF, 0xD8]); // SOI
        result.push(0xFF);
        result.push(0xE1); // APP1 marker
        result.push((app1_len >> 8) as u8);
        result.push((app1_len & 0xFF) as u8);
        result.extend_from_slice(&exif_payload);
        result.extend_from_slice(&jpeg_bytes[2..]); // skip SOI from original JPEG
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
        // Orientation 6 (rotate 90° CW): 200×100 landscape → 100×200 portrait
        assert_compressed_dimensions(
            make_exif_chat_image(200, 100, 6),
            100,
            200,
            "EXIF orientation 6",
        );
        // Orientation 8 (rotate 270° CW): same result
        assert_compressed_dimensions(
            make_exif_chat_image(200, 100, 8),
            100,
            200,
            "EXIF orientation 8",
        );
        // Orientation 3 (rotate 180°): dimensions unchanged
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
}
