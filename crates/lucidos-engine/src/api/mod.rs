mod actor;
mod app_ui;
mod apps;
mod artifacts;
pub(crate) mod backup;
mod blobs;
mod changes;
mod chat;
mod claude_code;
mod data_api;
mod disk_usage;
mod history;
mod images;
pub(crate) mod internal;
mod knowhow;
mod mcp;
mod memory;
mod notifications;
mod plugins;
mod presence;
pub(crate) mod proxy;
mod repositories;
mod saved_contexts;
mod sdk;
mod sdk_prefs;
mod search;
mod settings;
mod threads;
mod threads_compose;
mod triggers;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
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
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use crate::core::oauth::OAuthFlowResult;
use crate::core::{
    AppManager, ArtifactManager, ConversationSnapshot, CredentialInfo, EventStore,
    OAuthAccountInfo, SessionMessage, Step,
};
use crate::engine::{CaptureResult, LucidosEngine};
use crate::memory::{FastEmbedProvider, PgVectorIndex};
use crate::scheduler::{Notification, PushSubscription, SchedulerManager};

pub type SharedEngine = Arc<LucidosEngine>;

/// Pending OAuth flows keyed by provider name, each awaiting token exchange completion.
type PendingOAuthFlows =
    Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<OAuthFlowResult>>>>;

/// Check for path traversal attempts (`..`, leading `/` or `\`)
pub(crate) fn is_path_traversal(path: &str) -> bool {
    path.contains("..") || path.starts_with('/') || path.starts_with('\\')
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
    pub embedder: Arc<FastEmbedProvider>,
    pub memory_index: Option<PgVectorIndex>,
    pub workspace_path: PathBuf,
    pub app_manager: Arc<AppManager>,
    pub scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Pending notification from SW notificationclick — simple in-memory slot
    /// that bypasses client-side storage issues on iOS Safari.
    pub pending_notification_click: Arc<std::sync::Mutex<Option<String>>>,
    /// Pending notification from SW push event — fallback for iOS where
    /// notificationclick may not fire or fires too late on warm resume.
    /// Stored with timestamp so we can expire stale pushes.
    pub pending_notification_push: Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>,
    /// Pending OAuth flows keyed by provider — the receiver resolves when the
    /// background listener receives the callback and completes token exchange.
    pub pending_oauth_flows: PendingOAuthFlows,
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
const MAX_IMAGE_BASE64_BYTES: usize = 4_500_000;

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
}

/// Compress all images for LLM consumption — reduces size and normalizes format.
pub fn compress_images(images: Vec<ChatImage>) -> Vec<ChatImage> {
    images.into_iter().map(|img| img.compress()).collect()
}

#[derive(Debug, Deserialize)]
pub struct AppCaptureRequest {
    pub request_id: String,
    pub screenshot: String, // base64 PNG
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
    #[serde(default)]
    pub use_claude_code: Option<bool>,
    #[serde(default)]
    pub cc_model: Option<String>,
    #[serde(default, alias = "message_id")]
    pub event_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Required when `mode` is `"agent"` or `"engine"`: the thread that is
    /// spawning this new thread (e.g. the CC session whose `spawn-thread` skill
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
    /// `"personal"`). When set, the receiver constructs
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
    /// is set. Often `None` from CC subprocesses, which lack access to their
    /// own tool-call event id.
    #[serde(default)]
    pub caller_event_id: Option<String>,
    #[serde(default)]
    pub conflict_change_id: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub response: String,
    pub steps: Vec<Step>,
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

#[derive(Deserialize)]
pub struct CreateCredentialRequest {
    pub service_name: String,
    pub base_url: String,
    pub auth_type: String,
    pub auth_value: String,
    #[serde(default)]
    pub auth_header: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateCredentialRequest {
    pub auth_value: String,
}

/// Generic success/error response used by credential, preference, and trigger endpoints.
#[derive(Serialize)]
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
}

impl ApiResult {
    fn ok() -> Json<Self> {
        Json(Self {
            success: true,
            error: None,
            credential_request: None,
            auth_url: None,
        })
    }
    fn err(msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            success: false,
            error: Some(msg.into()),
            credential_request: None,
            auth_url: None,
        })
    }
    fn with_auth_url(url: String) -> Json<Self> {
        Json(Self {
            success: true,
            error: None,
            credential_request: None,
            auth_url: Some(url),
        })
    }
    fn needs_credentials(provider: &str) -> Json<Self> {
        Json(Self {
            success: false,
            error: Some(format!(
                "OAuth client credentials required for {}",
                provider
            )),
            credential_request: Some(serde_json::json!({
                "service": format!("oauth:{}", provider),
                "prompt": format!("Enter your OAuth client credentials for {}.", provider),
                "base_url": format!("https://{}.com", provider),
                "auth_type": "oauth_client"
            })),
            auth_url: None,
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
}

#[derive(Deserialize)]
pub struct PushUnsubscribeRequest {
    pub endpoint: String,
}

// Query param structs for routes migrated from path params
#[derive(Deserialize)]
struct ServiceQuery {
    service: String,
}

#[derive(Deserialize)]
struct KeyQuery {
    key: String,
}

#[derive(Deserialize)]
struct ProviderQuery {
    provider: String,
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

#[derive(Deserialize)]
struct AppCommitQuery {
    commit: Option<String>,
}

#[derive(Deserialize)]
struct AppVersionsQuery {
    id: String,
    limit: Option<usize>,
    skip: Option<usize>,
}

#[derive(Deserialize)]
struct AppRestoreQuery {
    id: String,
    commit: String,
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
/// Skips noisy endpoints (health checks, SSE streams, dev proxy) to keep logs readable.
async fn request_logger(req: axum::extract::Request, next: Next) -> Response {
    let uri_path = req.uri().path();
    let should_log = match uri_path {
        "/api/health" | "/api/events" => false,
        p => p.starts_with("/api/") || p.starts_with("/app/"),
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

#[allow(clippy::too_many_arguments)]
pub fn create_router(
    engine: SharedEngine,
    pool: PgPool,
    event_store: EventStore,
    embedder: Arc<FastEmbedProvider>,
    memory_index: Option<PgVectorIndex>,
    workspace_path: PathBuf,
    scheduler: Arc<tokio::sync::Mutex<SchedulerManager>>,
    started_at: chrono::DateTime<chrono::Utc>,
) -> Router {
    // Initialize AppManager
    let app_manager =
        Arc::new(AppManager::new(&workspace_path).expect("Failed to initialize AppManager"));

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
        pending_notification_click: Arc::new(std::sync::Mutex::new(None)),
        pending_notification_push: Arc::new(std::sync::Mutex::new(None)),
        pending_oauth_flows: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    // Serve static files from data/ tree — single mount covers all subdirectories
    let serve_data = ServeDir::new(workspace_path.join(crate::core::DATA_DIR));

    // Clone state for SDK v1 routes and app UI routes (api_routes consumes state below)
    let v1_state = state.clone();
    let app_ui_state = state.clone();

    // API routes under /api/*
    // Convention: query params for identifiers, path segments only for file paths
    let api_routes = Router::new()
        .route("/health", get(history::health))
        .route("/restart", post(history::restart_engine))
        .route("/workspaces", get(history::list_workspaces))
        .route("/events", get(history::global_events))
        .route("/chat", post(chat::chat))
        .route("/chat/stream", post(chat::chat_submit))
        .route("/chat/cancel", post(chat::cancel_chat))
        .route("/chat/inject", post(chat::inject_prompt))
        .route("/claude-code/cancel", post(claude_code::claude_code_cancel))
        .route(
            "/claude-code/interrupt",
            post(claude_code::claude_code_interrupt),
        )
        .route(
            "/claude-code/control",
            post(claude_code::claude_code_control),
        )
        .route(
            "/claude-code/commands",
            get(claude_code::claude_code_commands),
        )
        .route(
            "/claude-code/apply-now",
            post(claude_code::claude_code_apply_now),
        )
        .route(
            "/claude-code/discard",
            post(claude_code::claude_code_discard),
        )
        .route(
            "/claude-code/answer-question",
            post(claude_code::claude_code_answer_question),
        )
        .route("/changes", get(changes::list_changes))
        .route("/changes/applied", get(changes::list_applied_changes))
        .route("/changes/apply-all", post(changes::apply_all_changes))
        .route("/changes/discard-all", post(changes::discard_all_changes))
        .route(
            "/changes/for-repo/:repo_id",
            get(changes::list_changes_for_repo),
        )
        .route("/changes/:id/apply", post(changes::apply_change))
        .route("/changes/:id/discard", post(changes::discard_change))
        .route("/changes/:id/revert", post(changes::revert_change))
        .route("/changes/:id/diff", get(repositories::get_change_diff))
        .route("/changes/:id/file", get(repositories::get_change_file))
        .route(
            "/threads/:thread_id/cc-diff",
            get(repositories::get_thread_cc_diff),
        )
        .route("/changes/:id", get(changes::get_change))
        .route("/notifications", get(notifications::get_notifications))
        .route(
            "/notifications/before",
            get(notifications::get_notifications_at_timestamp),
        )
        .route(
            "/notifications/read-all",
            post(notifications::mark_all_notifications_read),
        )
        .route("/notification", get(notifications::get_notification))
        .route(
            "/notification/read",
            post(notifications::mark_notification_read),
        )
        .route("/history", get(history::get_history))
        .route("/messages", get(history::get_recent_messages))
        .route("/session/messages", get(history::get_session_messages))
        .route("/commits", get(artifacts::list_commits))
        .route("/commits/before", get(artifacts::get_commit_at_timestamp))
        // Events
        .route("/events/query", get(history::query_events))
        .route("/events/types", get(history::event_types))
        .route("/events/emit", post(history::emit_event))
        // Credentials endpoints
        .route(
            "/credentials",
            get(settings::list_credentials)
                .post(settings::create_credential)
                .put(settings::update_credential)
                .delete(settings::delete_credential),
        )
        .route("/credential-value", get(settings::get_credential_value))
        // OAuth account endpoints
        .route(
            "/oauth/accounts",
            get(settings::list_oauth_accounts).delete(settings::delete_oauth_account),
        )
        .route("/oauth/reauthorize", post(settings::reauthorize_oauth))
        .route("/oauth/complete", post(settings::complete_oauth))
        // Triggers
        .route(
            "/triggers",
            get(triggers::list_triggers)
                .post(triggers::create_trigger)
                .put(triggers::update_trigger)
                .delete(triggers::delete_trigger),
        )
        .route(
            "/triggers/historical",
            get(triggers::list_historical_triggers),
        )
        .route("/knowhow", get(knowhow::list_knowhow))
        // Preferences endpoints
        .route(
            "/preferences",
            get(settings::get_preferences)
                .put(settings::set_preference)
                .delete(settings::delete_preference),
        )
        // CC tool-permission allowlist (~/.lucidos/cc-allowed-tools)
        .route(
            "/cc-allowed-tools",
            get(settings::get_cc_allowed_tools).put(settings::put_cc_allowed_tools),
        )
        // Push notification endpoints
        .route("/push/vapid-key", get(notifications::get_vapid_key))
        .route("/push/subscribe", post(notifications::push_subscribe))
        .route("/push/unsubscribe", post(notifications::push_unsubscribe))
        .route(
            "/notification-clicked",
            post(notifications::notification_clicked).get(notifications::get_notification_clicked),
        )
        .route(
            "/notification-pushed",
            post(notifications::notification_pushed).get(notifications::get_notification_pushed),
        )
        .route(
            "/notification-dismissed",
            post(notifications::notification_dismissed),
        )
        // Thread presence (focus tracking → notification suppression)
        .route("/thread-presence", post(presence::update_presence))
        // Device endpoints
        .route("/devices/register", post(settings::register_device))
        .route("/devices", get(settings::list_devices))
        .route("/devices/:device_id/name", put(settings::rename_device))
        .route("/devices/:device_id/push", put(settings::set_device_push))
        .route(
            "/devices/:device_id",
            axum::routing::delete(settings::delete_device),
        )
        // Memory endpoints
        .route("/memory/stats", get(memory::get_memory_stats))
        .route("/memory/entries", get(memory::get_memory_entries))
        .route("/memory/source", get(memory::get_memory_source))
        .route(
            "/memory/rebuild",
            post(memory::rebuild_memory).delete(memory::cancel_rebuild_memory),
        )
        // Apps endpoints
        .route(
            "/pinned-apps",
            get(settings::get_pinned_apps)
                .post(settings::pin_app)
                .delete(settings::unpin_app),
        )
        .route("/apps", get(apps::list_apps))
        .route(
            "/app",
            get(apps::get_app)
                .put(apps::update_app)
                .delete(apps::delete_app),
        )
        .route(
            "/app/:app_id/source",
            get(apps::read_app_source).put(apps::write_app_source),
        )
        .route("/app/versions", get(apps::get_app_versions))
        .route("/app/restore", post(apps::restore_app_version))
        // Email endpoints
        .route("/email/send", post(settings::send_email_confirmed))
        // MCP endpoints
        .route("/mcp/consent", post(mcp::submit_mcp_consent))
        .route("/mcp/auto-approve", put(mcp::set_mcp_auto_approve))
        .route("/mcp/servers", get(mcp::list_mcp_servers))
        .route(
            "/internal/permission-prompt",
            post(internal::permission_prompt),
        )
        .route(
            "/internal/ask-user-question",
            post(internal::ask_user_question),
        )
        .route("/internal/mark-hardened", post(internal::mark_hardened))
        .route("/internal/hardened-state", get(internal::query_hardened))
        .route("/internal/commit-made", post(internal::commit_made))
        .route("/internal/client-log", post(internal::client_log))
        .route(
            "/internal/seed-change-for-test",
            post(internal::seed_change_for_test),
        )
        // App capture endpoints
        .route("/app-capture", post(apps::submit_app_capture))
        .route("/static/html2canvas.min.js", get(apps::serve_html2canvas))
        // Backup endpoints
        .route("/backup", post(backup::create_backup))
        .route("/backup/list", get(backup::list_backups))
        .route("/backup/restore", post(backup::restore_backup))
        .route("/backup/key", get(backup::get_backup_key))
        .route("/backup/providers", get(backup::list_providers))
        .route(
            "/backup/schedule",
            get(backup::get_schedule).put(backup::set_schedule),
        )
        .route(
            "/backup/retention",
            get(backup::get_retention).put(backup::set_retention),
        )
        .route(
            "/backup/validate-workspace-name",
            get(backup::validate_workspace_name),
        )
        .route("/backup/start-workspace", post(backup::start_workspace))
        // Saved contexts endpoints
        .route(
            "/saved-contexts",
            get(saved_contexts::list_saved_contexts).post(saved_contexts::save_context),
        )
        .route(
            "/saved-context",
            get(saved_contexts::get_saved_context).delete(saved_contexts::delete_saved_context),
        )
        // Thread endpoints
        .route("/threads", get(threads::list_threads))
        .route("/threads/search", get(threads::search_threads))
        .route("/threads/save", post(threads::save_thread))
        .route("/threads/unsave", post(threads::unsave_thread))
        .route("/threads/rename", post(threads::rename_thread))
        .route("/threads/archive", post(threads::archive_thread))
        .route("/threads/suggest-title", post(threads::suggest_title))
        .route(
            "/threads/:thread_id/messages",
            get(threads::get_thread_messages),
        )
        .route(
            "/threads/:thread_id/events",
            get(threads::get_thread_events_snapshot),
        )
        .route(
            "/threads/:thread_id/continue",
            post(threads::continue_thread),
        )
        .route("/disk-usage/summary", get(disk_usage::summary))
        .route("/disk-usage/worktrees", get(disk_usage::list_worktrees))
        .route(
            "/disk-usage/worktrees/:thread_id/cleanup",
            post(disk_usage::cleanup_worktree),
        )
        .route(
            "/threads/:thread_id/images",
            get(images::list_thread_images),
        )
        .route(
            "/threads/:thread_id/images/:index",
            get(images::get_thread_image),
        )
        .route("/threads/older", get(threads::get_older_threads))
        .route("/search", get(search::search))
        .route(
            "/repositories",
            get(repositories::list_repositories).post(repositories::add_repository),
        )
        .route("/browse-directories", get(repositories::browse_directories))
        .route(
            "/repositories/:id",
            axum::routing::delete(repositories::remove_repository),
        )
        .route(
            "/repositories/:id/files",
            get(repositories::list_repo_files),
        )
        .route("/repositories/:id/file", get(repositories::get_repo_file))
        .route("/repositories/:id/diff", get(repositories::get_repo_diff))
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        // Axum's 2 MiB default rejects mobile screenshots in chat/app-capture
        // bodies with "Failed to buffer the request body". Match the v1 cap.
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(state);

    // SDK v1 API routes
    let api_v1_routes = Router::new()
        .route("/sdk.js", get(sdk::serve_sdk_js))
        .route("/sdk-iframe.css", get(sdk::serve_sdk_iframe_css))
        .route("/sdk-iframe-audio.js", get(sdk::serve_sdk_iframe_audio_js))
        .route("/sdk-prefs.js", get(sdk_prefs::serve_sdk_prefs_js))
        .route("/ui/navigate", post(sdk::ui_navigate))
        .route("/data", get(data_api::list_data))
        .route("/data/edit", post(data_api::edit_data))
        .route("/data/upload", post(data_api::upload_data))
        .route("/threads", post(threads_compose::post_thread))
        .route("/threads/:id", delete(threads_compose::delete_thread))
        .route("/threads/:id/compose", put(threads_compose::put_compose))
        .route("/threads/:id/blobs", post(blobs::post_blob))
        .route("/blobs/:hash", get(blobs::get_blob))
        .route("/blobs/:hash/preview", get(blobs::get_blob_preview))
        .route(
            "/plugins/upload-archive",
            post(plugins::upload_archive)
                .layer(DefaultBodyLimit::max(plugins::MAX_ARCHIVE_BYTES)),
        )
        // Generic API proxy — forwards to a backend configured in
        // `data/config/apis.json`. Two routes so callers can hit
        // `/proxy/sonos` (no trailing path) as well as `/proxy/sonos/play/2`.
        .route("/proxy/:name", any(proxy::proxy_handler_root))
        .route("/proxy/:name/", any(proxy::proxy_handler_root))
        .route("/proxy/:name/*path", any(proxy::proxy_handler))
        // Sibling route (not /proxy/:name/_credentials) to avoid colliding
        // with the *path wildcard above.
        .route(
            "/proxy-credentials/:name",
            get(proxy::proxy_credentials_handler),
        )
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .route(
            "/data/*path",
            get(data_api::read_data)
                .put(data_api::write_data)
                .delete(data_api::delete_data),
        )
        .with_state(v1_state);

    // App UI routes under /app/* — file serving must be path-shaped (relative
    // URLs in app HTML resolve against the document path), so this lives at
    // the top level rather than under /api/.
    let app_ui_routes = Router::new()
        .route("/:app_id/", get(apps::serve_app_ui))
        .route("/:app_id/artifacts/*path", get(apps::serve_app_artifact))
        .route("/:app_id/*path", get(apps::serve_app_file))
        .with_state(app_ui_state);

    let router = Router::new()
        .nest("/api/v1", api_v1_routes)
        .nest("/api", api_routes)
        .nest("/app", app_ui_routes)
        .nest_service("/data", serve_data);

    // In dev mode, reverse-proxy unmatched requests to Vite so the browser
    // sees a single origin (engine port) for both API and frontend.
    let router = if let Ok(vite_url) = std::env::var("LUCIDOS_DEV_PROXY") {
        router.fallback(move |req: axum::extract::Request| {
            crate::dev_proxy::proxy(vite_url.clone(), req)
        })
    } else {
        router
    };

    // Prevent heuristic caching on ALL engine responses. `no-cache` means
    // "always revalidate with the server" — the browser still gets 304s when
    // files haven't changed, but never serves stale data after edits.
    // `if_not_present` preserves explicit headers (e.g. no-store on app UIs,
    // max-age on immutable static assets).
    router
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(axum::middleware::from_fn(request_logger))
        .layer(CorsLayer::permissive())
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
    fn compress_images_applies_to_all() {
        let images = vec![make_test_image(100, 100), make_test_image(200, 200)];
        let result = compress_images(images);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].mime_type, "image/jpeg");
        assert_eq!(result[1].mime_type, "image/jpeg");
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
}
