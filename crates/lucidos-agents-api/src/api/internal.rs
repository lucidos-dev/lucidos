use super::*;
use crate::api::error::ApiError;
use crate::diagnostic_log::CLIENT_LOG_MAX_BATCH;
use crate::engine::cc_permission::{prompt_coding_agent_permission, CodingAgentPermissionInput};
use crate::engine::claude_code::control::ControlExt;
use crate::engine::event_bus::BusEvent;
use crate::engine::git_ops::{PlanApprovalError, PlanMarkerKind};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

#[derive(Deserialize)]
pub(super) struct PermissionPromptRequest {
    pub thread_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Serialize)]
struct PermissionPromptResponse {
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

fn should_auto_allow(tool_name: &str) -> bool {
    crate::runtime::is_user_question_tool(tool_name)
}

/// INVARIANT: POST /api/v1/internal/permission-prompt — invoked by the lucidos-cli
/// `mcp-permission-server` subprocess (spawned by CC) when CC asks for
/// tool-call permission.
///
/// Behaves like `AskUserQuestion`: the engine emits a persisted event,
/// renders an inline card, and waits **indefinitely** for the user's answer.
/// No timeout on this handler — a timed-out denial would just push CC's
/// model into a retry that surfaces another card. CC's `MCP_TIMEOUT` and
/// `MCP_TOOL_TIMEOUT` env vars (both set to 24h in `runtime::claude_code`)
/// are the only practical bounds.
///
/// The dedup / session-allow / emit / wait core is shared with the Codex
/// app-server approval bridge — see
/// `engine::cc_permission::prompt_coding_agent_permission`. This handler
/// only adds the CC-specific auto-allow gate and the HTTP wire shape.
pub(super) async fn permission_prompt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PermissionPromptRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };
    // INVARIANT: An "Allow for this thread" here widens what that thread's agent may do,
    // so another thread must not raise the card.
    if asks_for_another_thread(&headers, thread_id) {
        crate::log_legacy!(
            "[PermissionPrompt] refused {thread_id}/{}: the caller's token names another thread",
            body.tool_use_id
        );
        return Json(PermissionPromptResponse {
            allowed: false,
            reason: Some(
                "A permission card lands on the thread that asks it, and this request named \
                 another thread."
                    .to_string(),
            ),
        })
        .into_response();
    }

    if should_auto_allow(&body.tool_name) {
        return Json(PermissionPromptResponse {
            allowed: true,
            reason: None,
        })
        .into_response();
    }

    let worktree_path = crate::engine::cc_permission::lookup_session_worktree(
        &state.engine.agent_sessions,
        thread_id,
    )
    .await;

    let outcome = prompt_coding_agent_permission(
        state.engine.pool(),
        &state.engine.event_bus,
        &state.engine.pending_cc_permission,
        &state.workspace_path,
        worktree_path.as_deref(),
        Some(&state.engine),
        CodingAgentPermissionInput {
            thread_id,
            tool_use_id: body.tool_use_id,
            tool_name: body.tool_name,
            input: body.input,
        },
    )
    .await;

    Json(PermissionPromptResponse {
        allowed: outcome.allowed,
        reason: outcome.reason,
    })
    .into_response()
}

/// INVARIANT: POST /api/v1/internal/restart-intent: the workspace gateway telling this
/// engine that a HUMAN asked for the teardown it is about to signal, and which
/// device they were on. Called immediately before the picker's Restart / Stop
/// sends `SIGUSR1` (`lucidos-gateway` `server.rs`, `notify_restart_intent`).
///
/// Without it the two restart paths are indistinguishable at teardown. The
/// in-workspace *Switch to new version* stashes its actor in its own handler
/// (`/api/v1/restart`) and the boundary emit reads it back, so those threads
/// settle `paused` with "Paused by restart" and auto-resume; a picker Restart
/// stashed nothing, so the identical teardown read as a crash and settled
/// `failed` with "Response interrupted" and no resume. This endpoint is the
/// missing half of that signal, and nothing downstream changes: the actor lands
/// in the same slot the switch handler writes, and `abort_in_flight_for_restart`
/// cannot tell the two apart (which is the point).
///
/// **It only stashes.** No respawn, no signal, no event, so it cannot recurse
/// with `/api/v1/restart` (whose respawn is what asks the gateway to call here).
///
/// Two refusals, both narrowing to "the gateway, on behalf of a named device":
///
///  * A request that came THROUGH the gateway proxy is rejected (403). A page on
///    the gateway origin, including an app in its own tab, could otherwise
///    set this engine's restart actor and so defeat the crash-loop protection in
///    *cause-gated resume*. See `base_path::arrived_through_gateway_proxy` for
///    why provenance rather than peer address is the discriminator.
///  * A caller with no resolvable DEVICE is rejected (400).
///    `user_actor` falls back to `Api { mode: Human }` with no device
///    id, and that is not the switch fingerprint (which needs
///    `actor.kind = 'device'`), so stashing it would not resume anything while
///    still replacing the honest "System" attribution with an API caller. The
///    gateway skips the call entirely when it has no device to name; this is the
///    engine-side floor under that.
pub(super) async fn restart_intent(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if crate::core::base_path::arrived_through_gateway_proxy(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "restart-intent is not reachable through the gateway proxy",
        )
            .into_response();
    }
    let actor = crate::api::actor::user_actor(&headers, None);
    let Some(actor @ crate::engine::thread_events::MessageOrigin::Device { .. }) = actor else {
        return (
            StatusCode::BAD_REQUEST,
            "restart-intent requires a device actor",
        )
            .into_response();
    };
    // INVARIANT: First writer wins: an in-workspace *Switch* stashed its own actor before it
    // asked the gateway for this respawn, and that one is the click's.
    let stashed = state.engine.stash_restart_actor(Some(actor));
    crate::log_legacy!(
        "[Restart] Gateway restart intent recorded (stashed={})",
        stashed
    );
    StatusCode::NO_CONTENT.into_response()
}

pub(super) async fn oauth_callback(
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> impl IntoResponse {
    if !may_deliver_oauth_callback(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "the OAuth callback is delivered by the workspace gateway only",
        )
            .into_response();
    }
    use crate::core::oauth_callback::{deliver_callback, CallbackDelivery};
    let (status, page) = match deliver_callback(query.as_deref().unwrap_or("")).await {
        CallbackDelivery::Accepted { state, page } if state.is_error() => {
            (StatusCode::BAD_REQUEST, page)
        }
        CallbackDelivery::Accepted { page, .. } => (StatusCode::OK, page),
        CallbackDelivery::Unclaimed { page } => (StatusCode::NOT_FOUND, page),
    };
    (status, axum::response::Html(page)).into_response()
}

fn may_deliver_oauth_callback(headers: &HeaderMap) -> bool {
    !crate::core::base_path::arrived_through_gateway_proxy(headers)
        && crate::api::local_auth::is_local_process(headers)
}

#[derive(Deserialize)]
pub(super) struct ClientLogRequest {
    pub category: String,
    #[serde(default)]
    pub code: Option<String>,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

const CLIENT_LOG_MAX_FIELD_LEN: usize = 256;
const CLIENT_LOG_MAX_DATA_LEN: usize = 4096;

/// INVARIANT: Validate one breadcrumb's caps, returning its serialized `data` on success.
/// `Err(reason)` on a cap violation (the caller maps it to a 400). Pure — does
/// NOT log, so a batch can validate every entry before committing any line.
fn validate_client_entry(entry: &ClientLogRequest) -> Result<String, &'static str> {
    if entry.category.len() > CLIENT_LOG_MAX_FIELD_LEN
        || entry.message.len() > CLIENT_LOG_MAX_FIELD_LEN
        || entry
            .code
            .as_ref()
            .is_some_and(|code| code.len() > CLIENT_LOG_MAX_FIELD_LEN)
    {
        return Err("category/message too long");
    }
    let data_str = entry.data.to_string();
    if data_str.len() > CLIENT_LOG_MAX_DATA_LEN {
        return Err("data too large");
    }
    Ok(data_str)
}

/// INVARIANT: Only this closed vocabulary can cross the maintainer boundary. A client
/// cannot mark arbitrary strings shareable by choosing a JSON field name.
/// Unknown/older codes remain useful to the owner as private values.
enum ClientToken<'a> {
    Known(&'static str),
    Private(&'a str),
}

impl std::fmt::Display for ClientToken<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(value) => f.write_str(value),
            Self::Private(value) => f.write_str(value),
        }
    }
}

impl lucidos_diagnostic_log::LogValue for ClientToken<'_> {
    fn is_private(&self) -> bool {
        matches!(self, Self::Private(_))
    }
}

fn client_token<'a>(value: &'a str, vocabulary: &[&'static str]) -> ClientToken<'a> {
    vocabulary
        .iter()
        .copied()
        .find(|known| *known == value)
        .map(ClientToken::Known)
        .unwrap_or(ClientToken::Private(value))
}

const CLIENT_CATEGORIES: &[&str] = &[
    "sw",
    crate::core::prefs::THEME_MODE.key(),
    "perf",
    "error",
    "lifecycle",
    "deeplink",
    "nav",
    "widget",
    "chunk",
    "device-database",
    "scroll",
    "restore-admission",
    "event-stream",
    "native-tap",
    "devices",
    "mobile",
    "page-health",
    "safe-area",
    "seen-target",
    "ipc",
    "connection",
    "chat",
    "toast-press",
    "render",
    "shell-scroll",
    "composer-press",
    "follow",
    "composer-typing",
    "app-link",
    "app-fullscreen",
    "storage-pressure",
];

const CLIENT_CODES: &[&str] = &[
    "quota",
    "uncaught",
    "unhandledrejection",
    "startup",
    "dispatch",
    "probe",
    "load_failed",
    "fetch_path_slow",
    "fetch_path_wedged",
    "released_on_hide",
    "open_blocked",
    "restore",
    "failed",
    "stalled",
    "admitted_after_stall",
    "drain",
    "hand_over_failed",
    "cleared_ghost_focus",
    "focus-reveal",
    "sample",
    "stall",
    "invoke-failed",
    "invoke-recovered",
    "state_changed",
    "send_retried",
    "thread_render_probe",
    "rebuild_corrupted_thread",
    "snap-back",
    "short",
    "outcome",
    "reading",
    "settled",
    "missed-frame",
    "asset_failed",
    "push",
    "notificationclick",
    "fouc",
    "pageshow",
    "route_thread_hash_landed",
    "route_thread_hash_not_found",
    "route_thread_hash_failed",
    "route_notifications_hash",
    "route_thread_hash",
    "handle_hash_location",
    "external-url-blocked",
    "external-url-opened-on-retry",
    "link-click-after-swipe",
    "thread-load",
    "thread-backfill",
    "main-thread-stall",
    "interaction",
    "loaf",
    "longtask",
    "perf-probe-support",
    "boot",
    "navigation",
    "group",
    "thread-render",
    "thread-rerender",
    "open",
    "answer",
];

fn client_line(entry: &ClientLogRequest, data: &str, ua: &str) -> lucidos_diagnostic_log::Line {
    use lucidos_diagnostic_log::{line, private};
    line!(
        "[Client/{}] {} {} code={} ua={}",
        client_token(&entry.category, CLIENT_CATEGORIES),
        private(&entry.message),
        private(data),
        client_token(entry.code.as_deref().unwrap_or(""), CLIENT_CODES),
        private(ua)
    )
}

fn user_agent(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

pub(super) async fn client_log(
    headers: HeaderMap,
    Json(body): Json<ClientLogRequest>,
) -> impl IntoResponse {
    match validate_client_entry(&body) {
        Ok(data_str) => {
            crate::typed_log_line(
                "Client",
                client_line(&body, &data_str, user_agent(&headers)),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(reason) => (StatusCode::BAD_REQUEST, reason).into_response(),
    }
}

pub(super) async fn client_logs(
    headers: HeaderMap,
    Json(body): Json<Vec<ClientLogRequest>>,
) -> impl IntoResponse {
    if body.len() > CLIENT_LOG_MAX_BATCH {
        return (StatusCode::BAD_REQUEST, "too many entries").into_response();
    }
    let mut serialized = Vec::with_capacity(body.len());
    for entry in &body {
        match validate_client_entry(entry) {
            Ok(data_str) => serialized.push(data_str),
            Err(reason) => return (StatusCode::BAD_REQUEST, reason).into_response(),
        }
    }
    let ua = user_agent(&headers);
    for (entry, data_str) in body.iter().zip(serialized) {
        crate::typed_log_line("Client", client_line(entry, &data_str, ua));
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(super) struct MarkHardenedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
    /// INVARIANT: `fast` or `full`. Absent means `full`, which is every marker a CLI
    /// predating the tier writes.
    #[serde(default)]
    pub tier: Option<String>,
}

fn tier_refusal(
    requested: crate::engine::git_ops::HardenTier,
    expected: crate::engine::git_ops::HardenTier,
) -> Option<&'static str> {
    (!requested.covers(expected)).then_some(
        "This branch needs the full /harden: it is not an integrated sub-thread's, or its \
         work became a change of its own. A fast marker would not count for its proposal or \
         its Apply. Run the full /harden, then `lucidos hardened mark`.",
    )
}

#[derive(Deserialize)]
pub(super) struct CodingAgentDiffRefreshRequest {
    pub thread_id: String,
    pub repo_root: String,
    pub branch_name: String,
}

#[derive(Serialize)]
struct CodingAgentDiffRefreshResponse {
    has_diff: bool,
    changed: bool,
}

/// INVARIANT: Whether `branch_name` carries un-applied work, for the durable change
/// state.
///
/// `Err` means git could not answer: a spawn failure, the 30s timeout, or a
/// branch ref that no longer resolves. The caller must then leave the state
/// alone. Writing the empty answer clears the Diff button for a thread that
/// just committed. A probe that could not run is UNKNOWN, never a "no"
/// (`.claude/rules/rust.md`).
///
/// `proposal_files_for_branch` cannot serve here. It folds a git failure into
/// an empty file list, which the caller then reads as "no diff".
async fn probe_branch_has_diff(
    repo_root: &std::path::Path,
    branch_name: &str,
) -> Result<bool, String> {
    if !crate::engine::git_ops::has_unmerged_authored_commits(repo_root, branch_name).await {
        return Ok(false);
    }
    let files =
        crate::engine::git_ops::branch_changed_files_checked(repo_root, branch_name).await?;
    Ok(!files.is_empty())
}

/// INVARIANT: POST /api/v1/internal/coding-agent-diff-refresh — invoked by the
/// coding-agent worktree's git post-commit hook via `lucidos
/// coding-agent-diff-hook`.
///
/// This is a live UI refresh only: it reconciles the change state from git
/// truth and broadcasts the updated aggregate when the state changes. It
/// does NOT emit `ChangeProposed`, create a `changes` row, or mark the turn as
/// ready for Apply. Formal proposal still happens when the coding-agent turn
/// idles.
///
/// A probe git could not answer takes 503 and writes nothing. The hook
/// discards the body, and the next commit re-runs the refresh.
pub(super) async fn coding_agent_diff_refresh(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<CodingAgentDiffRefreshRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    match crate::core::subprocess_origin::subprocess_origin(&headers) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            source_thread_id: Some(source_thread_id),
            ..
        } if source_thread_id == thread_id => {}
        _ => return (StatusCode::FORBIDDEN, "Invalid subprocess origin").into_response(),
    }

    let branch_name = body.branch_name.trim();
    if is_dangerous_git_ref(branch_name) {
        return (StatusCode::BAD_REQUEST, "Invalid branch_name").into_response();
    }

    if body.repo_root.trim().is_empty()
        || body.repo_root.contains('\0')
        || body.repo_root.split(['/', '\\']).any(|seg| seg == "..")
    {
        return (StatusCode::BAD_REQUEST, "Invalid repo_root").into_response();
    }
    let repo_root = std::path::PathBuf::from(body.repo_root);
    let has_diff = match probe_branch_has_diff(&repo_root, branch_name).await {
        Ok(has_diff) => has_diff,
        Err(e) => {
            crate::log_legacy!(
                "[Internal] coding-agent diff probe could not answer for thread {} branch {}: {}",
                thread_id,
                branch_name,
                e
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("coding-agent diff probe could not answer: {}", e),
            )
                .into_response();
        }
    };

    match state
        .engine
        .event_bus
        .refresh_branch_work_and_broadcast(thread_id, has_diff)
        .await
    {
        Ok(changed) => Json(CodingAgentDiffRefreshResponse { has_diff, changed }).into_response(),
        Err(e) => {
            crate::log_legacy!(
                "[Internal] coding-agent diff refresh failed for thread {} branch {}: {}",
                thread_id,
                branch_name,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("coding-agent diff refresh: {}", e),
            )
                .into_response()
        }
    }
}

/// INVARIANT: Whether this caller may write the hardening marker or the plan marker,
/// which the Harden and Apply gates read.
///
/// Two credentials qualify, and each proves a process on this machine:
///
///  * a verified thread-bound origin token. That is `lucidos` run from a
///    coding-agent session or from an engine-spawned script.
///  * the machine-local token, on a hop that did not come through the gateway
///    proxy. That is `lucidos` run from the user's own shell, and the e2e suite.
///
/// The proxy check is load-bearing. The gateway adds its own local token to
/// every request it proxies. Without the check, a paired device or any page on
/// the gateway origin would qualify. A device id alone qualifies nobody: any
/// caller on the loopback port can register one.
fn may_write_markers(headers: &HeaderMap) -> bool {
    use crate::core::subprocess_origin::{subprocess_origin, SubprocessOrigin};
    if matches!(
        subprocess_origin(headers),
        SubprocessOrigin::Subprocess { .. }
    ) {
        return true;
    }
    !crate::core::base_path::arrived_through_gateway_proxy(headers)
        && crate::api::local_auth::is_local_process(headers)
}

fn marker_write_refusal(headers: &HeaderMap) -> Option<axum::response::Response> {
    (!may_write_markers(headers)).then(|| {
        (
            StatusCode::FORBIDDEN,
            "the hardening and plan markers are written by the `lucidos` CLI from a \
             Lucidos-spawned process or a local shell, never by a plain loopback or \
             gateway caller",
        )
            .into_response()
    })
}

fn caller_thread(headers: &HeaderMap) -> Option<String> {
    match crate::core::subprocess_origin::subprocess_origin(headers) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => source_thread_id.map(|id| id.to_string()),
        crate::core::subprocess_origin::SubprocessOrigin::NotSubprocess => None,
    }
}

async fn off_tracked_branch_refusal(
    state: &AppState,
    repo_root: &std::path::Path,
    caller_thread: Option<&str>,
    branch_name: &str,
) -> Option<axum::response::Response> {
    let thread = caller_thread?;
    let tracked = match crate::engine::git_ops::tracked_branch_elsewhere(
        state.engine.pool(),
        repo_root,
        thread,
        branch_name,
    )
    .await
    {
        Ok(tracked) => tracked?,
        Err(e) => {
            return Some(
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("could not read this thread's tracked branch: {e}"),
                )
                .into_response(),
            )
        }
    };
    Some(
        ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "This thread's Apply lands only branch '{tracked}', and you are on \
                 '{branch_name}'. A plan marker here would never reach Apply. Run \
                 `git switch {tracked}`, bring any work you want applied, and re-run this \
                 command. Never cut a new branch for new work in this thread: \
                 '{tracked}' is the only one Lucidos applies."
            ),
        )
        .with_reason("marker-off-tracked-branch")
        .into_response(),
    )
}

pub(super) async fn mark_hardened(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<MarkHardenedRequest>,
) -> impl IntoResponse {
    use crate::engine::git_ops::HardenTier;
    if let Some(refusal) = marker_write_refusal(&headers) {
        return refusal;
    }
    let tier = match body.tier.as_deref() {
        None => HardenTier::Full,
        Some(raw) => match HardenTier::parse(raw) {
            Some(tier) => tier,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("unknown tier `{raw}`: expected `fast` or `full`"),
                )
                    .into_response()
            }
        },
    };
    let expected =
        crate::engine::integrated::expected_harden_tier(state.engine.pool(), &body.branch_name)
            .await;
    if let Some(refusal) = tier_refusal(tier, expected) {
        return (StatusCode::CONFLICT, refusal).into_response();
    }
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    match state
        .engine
        .record_hardened(&repo_root, &body.branch_name, &body.head_sha, tier)
        .await
    {
        Ok(()) => {
            let report = match calling_thread(&headers) {
                Some(thread_id) => HardenedReport {
                    stopped_background_tasks: state
                        .engine
                        .stop_background_tasks_of(thread_id)
                        .await,
                    still_waiting_on: state
                        .engine
                        .list_event_waits_for_thread(thread_id)
                        .await
                        .into_iter()
                        .map(|w| w.reason)
                        .collect(),
                },
                None => HardenedReport::default(),
            };
            Json(report.to_json()).into_response()
        }
        Err(e) => {
            crate::log_legacy!(
                "[Internal] record_hardened failed for {}: {}",
                body.branch_name,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("record_hardened: {}", e),
            )
                .into_response()
        }
    }
}

fn calling_thread(headers: &HeaderMap) -> Option<uuid::Uuid> {
    use crate::core::subprocess_origin::{subprocess_origin, SubprocessOrigin};
    match subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => source_thread_id,
        SubprocessOrigin::NotSubprocess => None,
    }
}

#[derive(Debug, Default)]
struct HardenedReport {
    stopped_background_tasks: Vec<crate::engine::event_wait::StoppedTask>,
    still_waiting_on: Vec<String>,
}

impl HardenedReport {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "stopped_background_tasks": self
                .stopped_background_tasks
                .iter()
                .map(|t| serde_json::json!({
                    "task_id": t.task_id,
                    "label": t.label,
                    "ended_with_others": t.ended_with_others,
                }))
                .collect::<Vec<_>>(),
            "still_waiting_on": self.still_waiting_on,
        })
    }
}

#[derive(Deserialize)]
pub(super) struct QueryHardenedQuery {
    pub repo_root: String,
    pub branch_name: String,
}

#[derive(Serialize)]
struct QueryHardenedResponse {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tier: Option<&'static str>,
    expected_tier: &'static str,
}

#[derive(Serialize)]
struct ReviewedHeadsResponse {
    heads: Vec<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct StartPlanRequest {
    #[serde(default)]
    kind: crate::engine::thread_events::PlanKind,
}

pub(super) async fn start_plan(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<StartPlanRequest>,
) -> axum::response::Response {
    if let Some(refusal) = marker_write_refusal(&headers) {
        return refusal;
    }
    let Some(thread_id) = caller_thread(&headers).and_then(|id| id.parse().ok()) else {
        return (
            StatusCode::BAD_REQUEST,
            "planned start requires a calling thread",
        )
            .into_response();
    };
    match crate::engine::plan_lifecycle::start(&state.engine.event_bus, thread_id, body.kind).await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("planned start: {e}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct MarkPlannedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
    pub state: String,
    #[serde(default)]
    pub kind: crate::engine::thread_events::PlanKind,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
async fn record_fix_marker(
    bus: &crate::engine::event_bus::EventBus,
    thread: Option<uuid::Uuid>,
    repo_root: &std::path::Path,
    branch: &str,
    kind: PlanMarkerKind,
    reason: Option<&str>,
    files: &[String],
    head: &str,
    plan_kind: crate::engine::thread_events::PlanKind,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::git_ops::PlanMarkerWrite;
    use crate::engine::thread_events::{PlanApprovalKind, PlanScope};

    let marker = PlanMarkerWrite::new(
        repo_root, branch, kind, None, reason, files, head, plan_kind,
    )?;
    let approval_kind = match kind {
        PlanMarkerKind::AcknowledgedSimple => PlanApprovalKind::SimpleFix,
        PlanMarkerKind::BoundedSecurityFix => PlanApprovalKind::BoundedSecurityFix,
        _ => return Err("expected a fix marker".into()),
    };
    let Some(thread) = thread else {
        marker.write(bus.pool()).await?;
        return Ok(());
    };
    if crate::engine::event_bus::EventBus::get_thread_type(bus.pool(), &thread).await?
        != crate::engine::thread_lifecycle::ThreadType::CodingAgent
    {
        marker.write(bus.pool()).await?;
        return Ok(());
    }
    let event = ThreadEvent::PlanApproved {
        kind: approval_kind,
        branch_name: Some(branch.into()),
        plan_path: String::new(),
        plan_sha: String::new(),
        scope: PlanScope::Implement,
        repo_root: Some(marker.repo_root().into()),
        plan_version: None,
        inherited_from_event_id: None,
    };
    if !bus
        .emit_with_mutation(thread, event, move |conn| {
            Box::pin(async move { marker.write(conn).await })
        })
        .await?
    {
        return Err("fix marker evidence was refused".into());
    }
    Ok(())
}

/// INVARIANT: POST /api/v1/internal/mark-planned — invoked by `lucidos planned mark` from
/// inside a coding-agent subprocess (and by the `implementation-plan` skill
/// after it writes the plan file). Records the durable Planned marker that the
/// `cc-plan-gate` PreToolUse hook and the Apply floor read. Mirrors
/// `mark_hardened`.
///
/// A rejected file list answers 400 with the rejection's own text, not 500. The
/// caller is an agent that can fix the call, so the body has to say how. A 500
/// reads as an engine fault and gets retried unchanged. A branch the calling
/// thread's Apply never lands answers 409 `marker-off-tracked-branch`.
pub(super) async fn mark_planned(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<MarkPlannedRequest>,
) -> impl IntoResponse {
    use crate::engine::git_ops::validate_plan_files;
    if let Some(refusal) = marker_write_refusal(&headers) {
        return refusal;
    }
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    // INVARIANT: Strict on the WRITE path. The lenient `parse` reads an unknown value as
    // `planned`, which on a write would record "a human approved this" for a
    // typo. `bounded-security-fix` is the one that matters.
    let Some(kind) = PlanMarkerKind::parse_strict(&body.state) else {
        let msg = format!(
            "Unknown plan-marker state {:?}. Use one of: proposed, planned, plan_only, \
             acknowledged_simple, bounded_security_fix (snake_case, not kebab-case).",
            body.state
        );
        crate::log_legacy!(
            "[Internal] mark-planned refused for {}: {}",
            body.branch_name,
            msg
        );
        return (StatusCode::BAD_REQUEST, msg).into_response();
    };
    let files = match validate_plan_files(kind, &body.files) {
        Ok(files) => files,
        Err(rejection) => {
            crate::log_legacy!(
                "[Internal] mark-planned refused for {}: {}",
                body.branch_name,
                rejection
            );
            return (StatusCode::BAD_REQUEST, rejection.to_string()).into_response();
        }
    };
    if let Some(refusal) = off_tracked_branch_refusal(
        &state,
        &repo_root,
        caller_thread(&headers).as_deref(),
        &body.branch_name,
    )
    .await
    {
        return refusal;
    }
    let thread = caller_thread(&headers).and_then(|id| uuid::Uuid::parse_str(&id).ok());
    let result = if let (PlanMarkerKind::Proposed, Some(thread), Some(path)) =
        (kind, thread, body.plan_path.as_deref())
    {
        crate::engine::plan_lifecycle::propose(
            &state.engine.event_bus,
            thread,
            &repo_root,
            &body.branch_name,
            path,
            &body.head_sha,
            body.reason.as_deref(),
            body.kind,
        )
        .await
    } else if matches!(
        kind,
        PlanMarkerKind::AcknowledgedSimple | PlanMarkerKind::BoundedSecurityFix
    ) {
        record_fix_marker(
            &state.engine.event_bus,
            thread,
            &repo_root,
            &body.branch_name,
            kind,
            body.reason.as_deref(),
            &files,
            &body.head_sha,
            body.kind,
        )
        .await
        .map(|()| false)
    } else {
        crate::engine::git_ops::record_planned_with_kind(
            &state.pool,
            &repo_root,
            &body.branch_name,
            kind,
            body.plan_path.as_deref(),
            body.reason.as_deref(),
            &files,
            &body.head_sha,
            body.kind,
        )
        .await
        .map(|()| false)
    };
    match result {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            crate::log_legacy!(
                "[Internal] record_planned failed for {}: {}",
                body.branch_name,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("record_planned: {}", e),
            )
                .into_response()
        }
    }
}

pub(super) async fn query_planned(
    State(state): State<AppState>,
    Query(q): Query<QueryHardenedQuery>,
) -> impl IntoResponse {
    use crate::engine::git_ops::PlanMarkerState;
    let repo_root = std::path::PathBuf::from(&q.repo_root);
    let marker = state
        .engine
        .plan_marker_state(&repo_root, &q.branch_name)
        .await;
    let (label, kind) = match marker {
        PlanMarkerState::Present(k) if k.satisfies_gate() => ("SATISFIED", Some(k.as_db())),
        PlanMarkerState::Present(k) => ("PROPOSED", Some(k.as_db())),
        PlanMarkerState::Changed => ("CHANGED", None),
        PlanMarkerState::Missing => ("MISSING", None),
        PlanMarkerState::Unreadable => ("UNREADABLE", None),
    };
    Json(QueryPlannedResponse { state: label, kind }).into_response()
}

#[derive(Serialize)]
struct QueryPlannedResponse {
    state: &'static str,
    kind: Option<&'static str>,
}

#[derive(Deserialize)]
pub(super) struct ApprovePlanRequest {
    pub repo_root: String,
    pub branch_name: String,
    #[serde(default)]
    pub plan_only: bool,
}

#[derive(Serialize)]
struct ApprovePlanResponse {
    approved: bool,
    plan_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_path: Option<String>,
}

/// INVARIANT: Approve the recorded plan for the scope the user's answer grants.
/// Design approvals always stay plan-only. A missing approval returns 409
/// `no-plan-approval`; a changed batch binding returns 409 `plan-changed`.
/// An unreadable answer returns 500. Off-branch writes return 409
/// `marker-off-tracked-branch`, as on `mark-planned`.
pub(super) async fn approve_plan(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ApprovePlanRequest>,
) -> impl IntoResponse {
    if let Some(refusal) = marker_write_refusal(&headers) {
        return refusal;
    }
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    let caller_thread = caller_thread(&headers);
    if let Some(refusal) = off_tracked_branch_refusal(
        &state,
        &repo_root,
        caller_thread.as_deref(),
        &body.branch_name,
    )
    .await
    {
        return refusal;
    }
    match state
        .engine
        .approve_plan(
            &repo_root,
            &body.branch_name,
            caller_thread.as_deref(),
            body.plan_only,
        )
        .await
    {
        Ok(approved) => Json(ApprovePlanResponse {
            plan_only: approved.as_ref().is_some_and(|a| a.plan_only),
            approved: approved.is_some(),
            plan_path: approved.and_then(|a| a.plan_path),
        })
        .into_response(),
        Err(e @ PlanApprovalError::NotApproved) => {
            ApiError::new(StatusCode::CONFLICT, e.to_string())
                .with_reason("no-plan-approval")
                .into_response()
        }
        Err(e @ PlanApprovalError::PlanChanged) => {
            ApiError::new(StatusCode::CONFLICT, e.to_string())
                .with_reason("plan-changed")
                .into_response()
        }
        Err(e) => {
            crate::log_legacy!(
                "[Internal] approve_plan failed for {}: {}",
                body.branch_name,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("approve_plan: {}", e),
            )
                .into_response()
        }
    }
}

/// INVARIANT: GET /api/v1/internal/hardened-state?repo_root=...&branch_name=... — invoked by
/// `lucidos hardened query` so the `harden.md` skill and `pre-push.sh` hook
/// share the DB-backed marker that `mark-hardened` writes. `lucidos hardened
/// sha`, `tier` and `expected-tier` read the same answer.
pub(super) async fn query_hardened(
    State(state): State<AppState>,
    Query(q): Query<QueryHardenedQuery>,
) -> impl IntoResponse {
    use crate::engine::git_ops::{lands_plan_files_only, HardenMarker, HardenMarkerState};
    let repo_root = std::path::PathBuf::from(&q.repo_root);
    let marker = state.engine.harden_marker(&repo_root, &q.branch_name).await;
    let expected =
        crate::engine::integrated::expected_harden_tier(state.engine.pool(), &q.branch_name).await;
    let marker_state = marker.state_for(expected);
    // INVARIANT: Unknown landing files read as code to harden, never as a plan.
    let plan_only = !matches!(marker_state, HardenMarkerState::Fresh)
        && lands_plan_files_only(
            &state
                .engine
                .landing_files(&repo_root, &q.branch_name)
                .await
                .unwrap_or_default(),
        );
    let label = match marker_state {
        HardenMarkerState::Fresh => "FRESH",
        _ if plan_only => "NOT_NEEDED",
        HardenMarkerState::Stale => "STALE",
        HardenMarkerState::Missing => "MISSING",
    };
    let tier = marker.tier().map(|t| t.as_str());
    let head_sha = match marker {
        HardenMarker::Recorded { head_sha, .. } => Some(head_sha),
        HardenMarker::Missing => None,
    };
    Json(QueryHardenedResponse {
        state: label,
        head_sha,
        tier,
        expected_tier: expected.as_str(),
    })
    .into_response()
}

/// INVARIANT: GET /api/v1/internal/reviewed-heads?repo_root=...&branch_name=..., invoked
/// by `lucidos hardened reviewed-heads`. Lists the heads other `/harden` runs
/// certified for the branch (`engine::reviewed_heads`), which `/harden` Phase
/// 0.4 hands to `scripts/harden-scope.sh` as already reviewed.
pub(super) async fn query_reviewed_heads(
    State(state): State<AppState>,
    Query(q): Query<QueryHardenedQuery>,
) -> impl IntoResponse {
    match crate::engine::reviewed_heads::reviewed_heads(state.engine.pool(), &q.branch_name).await {
        Ok(heads) => Json(ReviewedHeadsResponse { heads }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reviewed_heads: {e}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct SeedChangeForTestRequest {
    pub change_id: String,
    pub thread_id: String,
    pub branch_name: String,
    pub repo_root: String,
    pub description: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub requires_restart: bool,
    #[serde(default)]
    pub hardened: bool,
    #[serde(default)]
    pub planned: Option<bool>,
}

pub(super) async fn seed_change_for_test(
    State(state): State<AppState>,
    Json(body): Json<SeedChangeForTestRequest>,
) -> impl IntoResponse {
    if !cfg!(any(debug_assertions, feature = "e2e-test-hooks")) {
        return (StatusCode::NOT_FOUND, "test-only endpoint").into_response();
    }

    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };
    if Uuid::parse_str(&body.change_id).is_err() {
        return (StatusCode::BAD_REQUEST, "Invalid change_id").into_response();
    }
    if body.repo_root.is_empty() || body.repo_root.split(['/', '\\']).any(|seg| seg == "..") {
        return (StatusCode::BAD_REQUEST, "repo_root: empty or contains '..'").into_response();
    }
    if let Some(bad) = reject_unsafe_relative(&body.branch_name) {
        return (StatusCode::BAD_REQUEST, format!("branch_name: {bad}")).into_response();
    }
    for f in &body.files {
        if let Some(bad) = reject_unsafe_relative(f) {
            return (StatusCode::BAD_REQUEST, format!("files entry: {bad}")).into_response();
        }
    }

    let repo_root_for_marker = body.repo_root.clone();
    let branch_for_marker = body.branch_name.clone();
    let record_marker = body.planned != Some(false);

    let result = state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ChangeProposed {
                content: crate::engine::thread_events::ChangeContent::from_files(&body.files),
                change_id: body.change_id.clone(),
                description: Some(body.description),
                files: body.files,
                requires_restart: body.requires_restart,
                origin: None,
                commit_sha: None,
                branch_name: body.branch_name,
                repo_root: body.repo_root,
                hardened: body.hardened,
                incomplete: false,
                set_aside: false,
                path: String::new(),
                diff: String::new(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await;

    if record_marker {
        if let Err(e) = state
            .engine
            .record_planned(
                &std::path::PathBuf::from(&repo_root_for_marker),
                &branch_for_marker,
                crate::engine::git_ops::PlanMarkerKind::AcknowledgedSimple,
                None,
                Some("seeded test change"),
                &[],
                "seeded",
            )
            .await
        {
            crate::log_legacy!(
                "[Internal] seed-change-for-test record_planned failed: {}",
                e
            );
        }
    }

    match result {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "change_id": body.change_id })),
        )
            .into_response(),
        Err(e) => {
            crate::log_legacy!("[Internal] seed-change-for-test emit failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("emit failed: {e}"),
            )
                .into_response()
        }
    }
}

fn reject_unsafe_relative(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("empty");
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Some("must not be absolute");
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Some("must not contain '..' segment");
    }
    None
}

#[derive(Deserialize)]
pub(super) struct AskUserQuestionRequest {
    pub thread_id: String,
    pub tool_use_id: String,
    pub session_id: String,
    pub questions: serde_json::Value,
}

#[derive(Serialize)]
struct AskUserQuestionResponse {
    questions: serde_json::Value,
    answers: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    quiet: bool,
}

pub(in crate::api) fn asks_for_another_thread(
    headers: &axum::http::HeaderMap,
    thread_id: Uuid,
) -> bool {
    matches!(
        crate::core::subprocess_origin::subprocess_origin(headers),
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess { source_thread_id, .. }
            if source_thread_id != Some(thread_id)
    )
}

pub(super) async fn ask_user_question(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AskUserQuestionRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };
    if asks_for_another_thread(&headers, thread_id) {
        crate::log_legacy!(
            "[AskUserQuestion] refused {thread_id}/{}: the caller's token names another thread",
            body.tool_use_id
        );
        return Json(AskUserQuestionResponse {
            questions: body.questions,
            answers: serde_json::Value::Object(serde_json::Map::new()),
            refusal: Some(
                "A question card lands on the thread that asks it, and this request named \
                 another thread. Ask from inside your own thread."
                    .to_string(),
            ),
            quiet: false,
        })
        .into_response();
    }

    // INVARIANT: Taken now, so an MCP ask refused below still takes its ordinal.
    let flushes = state.engine.question_flushes(thread_id).await;
    let ask = flushes.as_ref().map(|f| f.ask(&body.tool_use_id));

    let asked = body.questions.clone();
    let refuse = |questions, refusal| {
        Json(AskUserQuestionResponse {
            questions,
            answers: serde_json::Value::Object(serde_json::Map::new()),
            refusal: Some(refusal),
            quiet: false,
        })
        .into_response()
    };
    use crate::engine::permission::prepare::{prepared_ask, PreparedAsk};
    let (questions, permission_request) = match prepared_ask(
        &state.pool,
        thread_id,
        &body.questions,
    )
    .await
    {
        Ok(PreparedAsk::Ordinary) => {
            use crate::engine::integrated::read_thread_is_integrated;
            let refusal = match read_thread_is_integrated(&state.pool, thread_id).await {
                    Ok(false) => None,
                    Ok(true) => Some(
                        "An integrated sub-thread reports decisions to its parent. Run \
                         `lucidos wake-parent yes`, then end your turn with the decision and options \
                         in your report. Your parent can answer by messaging this thread.",
                    ),
                    Err(e) => {
                        crate::log_legacy!("[AskUserQuestion] integration lookup for {thread_id}: {e}");
                        Some("Could not check whether this thread is integrated. Retry the question tool.")
                    }
                };
            if let Some(reason) = refusal {
                return refuse(body.questions, reason.to_string());
            }
            (body.questions, None)
        }
        Ok(PreparedAsk::Request { request, questions }) => (questions, Some(request)),
        Err(refusal) => return refuse(body.questions, refusal),
    };

    let text_persisted = async move {
        if let (Some(flushes), Some(ask)) = (flushes, ask) {
            flushes.wait_for(ask).await;
        }
    };
    if let Some(refusal) = crate::engine::question_card_gate::refuse_coding_agent_card(
        &state.pool,
        thread_id,
        &body.tool_use_id,
        &asked,
        text_persisted,
    )
    .await
    {
        crate::log_legacy!(
            "[AskUserQuestion] refused {thread_id}/{}: {refusal:?}",
            body.tool_use_id
        );
        return Json(AskUserQuestionResponse {
            questions,
            answers: serde_json::Value::Object(serde_json::Map::new()),
            refusal: Some(refusal.text()),
            quiet: false,
        })
        .into_response();
    }

    let outcome = state
        .engine
        .walk_question_batch(
            thread_id,
            &body.tool_use_id,
            &questions,
            None,
            crate::engine::agent_question::QuestionAsker {
                cc_session_id: body.session_id.clone(),
                channel: EventChannel::ClaudeCode,
                permission_request,
            },
        )
        .await;

    let (answer_kinds, quiet) = match outcome {
        Ok(o) => (o.answer_kinds, o.quiet),
        Err(e) => {
            // INVARIANT: Both callers turn a 500 into a failed tool call carrying this
            // text. It tells the agent to ask again with the tool
            // (`card_not_shown_reason` in lucidos-cli).
            crate::log_legacy!(
                "[AskUserQuestion] walk failed for {thread_id}/{}: {e}",
                body.tool_use_id
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if answer_kinds.is_empty() {
        crate::log_legacy!(
            "[AskUserQuestion] CC sent zero questions for {thread_id}/{}",
            body.tool_use_id
        );
        return Json(AskUserQuestionResponse {
            questions,
            answers: serde_json::Value::Object(serde_json::Map::new()),
            refusal: None,
            quiet: false,
        })
        .into_response();
    }

    let answers = crate::engine::agent_question::build_hook_answers(
        &answer_kinds,
        &questions,
        crate::engine::agent_question::AnswerImages::BlobPaths(state.engine.workspace_path()),
    );
    Json(AskUserQuestionResponse {
        questions,
        answers,
        refusal: None,
        quiet,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct RequestAppCaptureForTest {
    pub thread_id: String,
    pub app_id: String,
    pub device_id: Option<String>,
    pub save_as_artifact: Option<String>,
}

pub(super) async fn request_app_capture_for_test(
    State(state): State<AppState>,
    Json(body): Json<RequestAppCaptureForTest>,
) -> impl IntoResponse {
    use crate::engine::tools::app_capture::{app_ui_tool_impl, CAPTURE_TIMEOUT};
    if !cfg!(any(debug_assertions, feature = "e2e-test-hooks")) {
        return (StatusCode::NOT_FOUND, "test-only endpoint").into_response();
    }
    let Ok(thread_id) = Uuid::parse_str(&body.thread_id) else {
        return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response();
    };
    let artifacts = match crate::core::ArtifactManager::new(state.workspace_path.clone()) {
        Ok(artifacts) => artifacts,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let mut args = serde_json::json!({ "app_id": body.app_id });
    if let Some(path) = body.save_as_artifact {
        args["save_as_artifact"] = path.into();
    }
    let result = app_ui_tool_impl(
        &state.engine.event_bus,
        &state.pool,
        &state.engine.pending_captures,
        &artifacts,
        &args,
        thread_id,
        false,
        body.device_id.as_deref(),
        CAPTURE_TIMEOUT,
    )
    .await;
    Json(serde_json::json!({ "result": result })).into_response()
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/permission-prompt", post(permission_prompt))
        .route("/internal/ask-user-question", post(ask_user_question))
        .route("/internal/restart-intent", post(restart_intent))
        .route(
            lucidos_gateway_constants::OAUTH_DELIVERY_ROUTE,
            get(oauth_callback),
        )
        .route("/internal/mark-hardened", post(mark_hardened))
        .route(
            "/internal/coding-agent-diff-refresh",
            post(coding_agent_diff_refresh),
        )
        .route("/internal/hardened-state", get(query_hardened))
        .route("/internal/reviewed-heads", get(query_reviewed_heads))
        .route("/internal/start-plan", post(start_plan))
        .route("/internal/mark-planned", post(mark_planned))
        .route("/internal/planned-state", get(query_planned))
        .route("/internal/approve-plan", post(approve_plan))
        .route("/internal/client-log", post(client_log))
        .route("/internal/client-logs", post(client_logs))
        .route("/internal/seed-change-for-test", post(seed_change_for_test))
        .route(
            "/internal/request-app-capture-for-test",
            post(request_app_capture_for_test),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fix_markers_record_thread_evidence_without_human_approval() {
        use crate::engine::{event_bus::EventBus, thread_events::PlanKind};
        use crate::test_support::{
            seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
        };

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = tempfile::tempdir().unwrap();
        let files = ["src/lib.rs".into()];

        for (kind, branch, files, evidence) in [
            (
                PlanMarkerKind::AcknowledgedSimple,
                "simple",
                &[][..],
                "simple-fix",
            ),
            (
                PlanMarkerKind::BoundedSecurityFix,
                "bounded",
                &files[..],
                "bounded-security-fix",
            ),
        ] {
            record_fix_marker(
                &bus,
                Some(thread),
                repo.path(),
                branch,
                kind,
                None,
                files,
                "head",
                PlanKind::Implementation,
            )
            .await
            .unwrap();
            let payload: serde_json::Value = sqlx::query_scalar(
                "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved' AND payload->>'branch_name' = $2",
            ).bind(thread).bind(branch).fetch_one(&pool).await.unwrap();
            assert_eq!(payload["kind"], evidence);
            assert_eq!(payload["scope"], "implement");
        }
        assert!(!crate::engine::approved_plan::approved_this_turn_and_idle(&pool, thread).await);

        let chat = seed_chat_thread(&bus, "mark a fix").await;
        for (caller, branch) in [(None, "threadless"), (Some(chat), "chat")] {
            record_fix_marker(
                &bus,
                caller,
                repo.path(),
                branch,
                PlanMarkerKind::AcknowledgedSimple,
                None,
                &[],
                "head",
                PlanKind::Implementation,
            )
            .await
            .unwrap();
            let marker: String =
                sqlx::query_scalar("SELECT state FROM planned_branches WHERE branch_name = $1")
                    .bind(branch)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(marker, "acknowledged_simple");
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PlanApproved'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        teardown_test_db(&db).await;
    }

    #[test]
    fn a_fast_mark_is_refused_on_a_branch_that_needs_the_full_run() {
        use crate::engine::git_ops::HardenTier::{Fast, Full};
        assert!(tier_refusal(Fast, Full).is_some());
        assert!(tier_refusal(Fast, Fast).is_none());
        assert!(tier_refusal(Full, Fast).is_none());
        assert!(tier_refusal(Full, Full).is_none());
    }

    #[test]
    fn every_question_tool_is_auto_allowed_to_avoid_redundant_card() {
        for tool in [
            crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL,
            crate::runtime::CC_MCP_ASK_USER_QUESTION_TOOL,
        ] {
            assert!(
                should_auto_allow(tool),
                "{tool} must short-circuit the permission gate"
            );
        }
    }

    #[test]
    fn other_tools_are_not_auto_allowed() {
        for tool in [
            "Edit",
            "Bash",
            "Write",
            "Read",
            "Glob",
            "Grep",
            "Skill",
            crate::runtime::CC_PERMISSION_PROMPT_TOOL,
        ] {
            assert!(
                !should_auto_allow(tool),
                "{tool} must NOT short-circuit the permission gate"
            );
        }
    }

    fn marker_headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(*name, value.parse().expect("header value"));
        }
        h
    }

    fn minted_origin_token(thread_id: Option<Uuid>) -> String {
        crate::core::subprocess_origin::init_agent_origin_secret(
            "internal-marker-test-secret".to_string(),
        );
        crate::core::subprocess_origin::mint_agent_origin_token(thread_id, 0, None)
            .expect("secret installed at least once")
    }

    #[test]
    fn a_plain_loopback_caller_cannot_write_a_marker() {
        crate::api::local_auth::publish_test_local_token();
        assert!(!may_write_markers(&HeaderMap::new()));
        assert!(!may_write_markers(&marker_headers(&[(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            "self-registered-device",
        )])));
        assert!(marker_write_refusal(&HeaderMap::new())
            .is_some_and(|r| r.status() == StatusCode::FORBIDDEN));
    }

    #[test]
    fn a_forged_origin_token_or_wrong_local_token_is_refused() {
        crate::api::local_auth::publish_test_local_token();
        let token = minted_origin_token(Some(Uuid::new_v4()));
        let (prefix, _mac) = token.rsplit_once('.').expect("minted token has a mac");
        let forged = format!("{prefix}.{}", "0".repeat(64));
        assert!(!may_write_markers(&marker_headers(&[(
            crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
            &forged,
        )])));
        assert!(!may_write_markers(&marker_headers(&[(
            lucidos_local_token::HEADER_LOCAL_TOKEN,
            "not-the-machine-token",
        )])));
    }

    #[test]
    fn a_lucidos_spawned_process_may_write_a_marker() {
        for thread in [Some(Uuid::new_v4()), None] {
            let token = minted_origin_token(thread);
            assert!(
                may_write_markers(&marker_headers(&[(
                    crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
                    &token,
                )])),
                "a minted origin token for thread {thread:?} must qualify"
            );
        }
    }

    #[test]
    fn a_local_process_on_the_direct_hop_may_write_a_marker() {
        let local = crate::api::local_auth::publish_test_local_token();
        assert!(may_write_markers(&marker_headers(&[(
            lucidos_local_token::HEADER_LOCAL_TOKEN,
            local,
        )])));
    }

    #[test]
    fn only_the_gateway_hop_may_deliver_an_oauth_callback() {
        let local = crate::api::local_auth::publish_test_local_token();
        let token = (lucidos_local_token::HEADER_LOCAL_TOKEN, local);
        assert!(may_deliver_oauth_callback(&marker_headers(&[token])));
        assert!(!may_deliver_oauth_callback(&marker_headers(&[
            token,
            ("x-forwarded-prefix", "/dev/"),
            (crate::core::subprocess_origin::HEADER_DEVICE_ID, "a-device"),
        ])));
        assert!(!may_deliver_oauth_callback(&marker_headers(&[])));
    }

    #[test]
    fn only_a_thread_bound_caller_names_a_thread_to_stop_work_on() {
        let thread = Uuid::new_v4();
        let bound = minted_origin_token(Some(thread));
        assert_eq!(
            calling_thread(&marker_headers(&[(
                crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
                &bound,
            )])),
            Some(thread)
        );
        let threadless = minted_origin_token(None);
        assert_eq!(
            calling_thread(&marker_headers(&[(
                crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
                &threadless,
            )])),
            None
        );
        let local = crate::api::local_auth::publish_test_local_token();
        assert_eq!(
            calling_thread(&marker_headers(&[(
                lucidos_local_token::HEADER_LOCAL_TOKEN,
                local,
            )])),
            None
        );
    }

    #[test]
    fn the_hardened_report_carries_both_lists_even_when_empty() {
        assert_eq!(
            HardenedReport::default().to_json(),
            serde_json::json!({ "stopped_background_tasks": [], "still_waiting_on": [] })
        );
        let report = HardenedReport {
            stopped_background_tasks: vec![crate::engine::event_wait::StoppedTask {
                task_id: "t1".into(),
                label: "make lint".into(),
                ended_with_others: vec![],
            }],
            still_waiting_on: vec!["the release build to finish".into()],
        };
        assert_eq!(
            report.to_json(),
            serde_json::json!({
                "stopped_background_tasks": [
                    { "task_id": "t1", "label": "make lint", "ended_with_others": [] }
                ],
                "still_waiting_on": ["the release build to finish"],
            })
        );
    }

    #[test]
    fn a_request_through_the_gateway_proxy_cannot_write_a_marker() {
        let local = crate::api::local_auth::publish_test_local_token();
        assert!(!may_write_markers(&marker_headers(&[
            (lucidos_local_token::HEADER_LOCAL_TOKEN, local),
            (
                crate::core::subprocess_origin::HEADER_DEVICE_ID,
                "paired-phone"
            ),
            ("x-forwarded-prefix", "/dev/"),
        ])));
    }

    fn entry(category: &str, message: &str, data: serde_json::Value) -> ClientLogRequest {
        ClientLogRequest {
            category: category.into(),
            code: None,
            message: message.into(),
            data,
        }
    }

    #[tokio::test]
    async fn client_log_handlers_persist_classification_and_validate_batches_atomically() {
        use lucidos_diagnostic_log::{writer::Record, Destination};
        const CHILD: &str = "CLIENT_LOG_HANDLER_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD) {
            crate::diagnostic_log::start(std::path::Path::new(&root)).unwrap();
            let payload = serde_json::json!({
                "category": "error", "code": "uncaught", "message": "PRIVATE_ROUTE_MESSAGE",
                "data": {"detail": "PRIVATE_ROUTE_DATA"}
            });
            let entry = || serde_json::from_value::<ClientLogRequest>(payload.clone()).unwrap();
            let mut headers = HeaderMap::new();
            headers.insert("user-agent", "PRIVATE_ROUTE_UA".parse().unwrap());
            assert_eq!(
                client_log(headers.clone(), Json(entry()))
                    .await
                    .into_response()
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                client_logs(headers.clone(), Json(vec![entry(), entry()]))
                    .await
                    .into_response()
                    .status(),
                StatusCode::NO_CONTENT
            );
            let mut invalid = entry();
            invalid.code = Some("x".repeat(CLIENT_LOG_MAX_FIELD_LEN + 1));
            assert_eq!(
                client_logs(headers, Json(vec![entry(), invalid]))
                    .await
                    .into_response()
                    .status(),
                StatusCode::BAD_REQUEST
            );
            assert!(lucidos_diagnostic_log::flush(
                std::time::Duration::from_secs(2)
            ));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "api::internal::tests::client_log_handlers_persist_classification_and_validate_batches_atomically", "--nocapture"])
            .env(CHILD, root.path())
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::fs::read_to_string(
            crate::diagnostic_log::dir_for(root.path())
                .join(lucidos_diagnostic_log::writer::FILE_NAME),
        )
        .unwrap();
        let records: Vec<Record> = text
            .lines()
            .map(|line| serde_json::from_str::<Record>(line).unwrap())
            .filter(|record| record.label == "Client")
            .collect();
        assert_eq!(records.len(), 3, "invalid batch must write nothing");
        for record in records {
            assert!(record.line.is_some());
            assert!(record.ts.ends_with('Z'));
            let rendered = record.render(Destination::Maintainer);
            assert!(rendered.contains("code=uncaught"));
            assert!(!rendered.contains("PRIVATE_"));
        }
    }

    #[test]
    fn client_lines_keep_codes_but_never_share_message_data_or_user_agent() {
        use lucidos_diagnostic_log::Destination;
        let mut e = entry(
            "error",
            "PRIVATE_CLIENT_MESSAGE",
            serde_json::json!({
                "url": "PRIVATE_CLIENT_DATA", "status": 500
            }),
        );
        e.code = Some("uncaught".into());
        let line = client_line(
            &e,
            &validate_client_entry(&e).unwrap(),
            "PRIVATE_USER_AGENT",
        );
        let maintainer = line.render(Destination::Maintainer);
        assert!(maintainer.contains("[Client/error]"));
        assert!(maintainer.contains("code=uncaught"));
        assert!(!maintainer.contains("PRIVATE_"), "{maintainer}");
        let owner = line.render(Destination::Owner);
        for marker in [
            "PRIVATE_CLIENT_MESSAGE",
            "PRIVATE_CLIENT_DATA",
            "PRIVATE_USER_AGENT",
        ] {
            assert!(owner.contains(marker));
        }
    }

    #[test]
    fn client_supplied_classifications_cannot_make_arbitrary_text_shareable() {
        use lucidos_diagnostic_log::Destination;
        let mut e = entry(
            "PRIVATE_CATEGORY",
            "PRIVATE_MESSAGE",
            serde_json::Value::Null,
        );
        e.code = Some("PRIVATE_CODE".into());
        let line = client_line(&e, "null", "");
        assert!(!line.render(Destination::Maintainer).contains("PRIVATE_"));
        assert!(line.render(Destination::Owner).contains("PRIVATE_CODE"));
        let e = entry("error", "PRIVATE_LEGACY_MESSAGE", serde_json::Value::Null);
        assert!(!client_line(&e, "null", "")
            .render(Destination::Maintainer)
            .contains("PRIVATE_"));
    }

    #[test]
    fn validate_client_entry_accepts_normal_breadcrumb_and_returns_serialized_data() {
        let e = entry(
            "perf",
            "open",
            serde_json::json!({ "eventCount": 7847, "openMs": 1234 }),
        );
        let data = validate_client_entry(&e).expect("normal entry valid");
        assert!(
            data.contains("7847") && data.contains("openMs"),
            "returns serialized data for logging"
        );
    }

    async fn repo_on_main() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::engine::git_ops::git_cmd;
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().to_path_buf();
        git_cmd(&["init", "-q", "-b", "main"], &repo).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &repo)
            .await
            .unwrap();
        std::fs::write(repo.join("init.txt"), "initial").unwrap();
        git_cmd(&["add", "-A"], &repo).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "initial"], &repo)
            .await
            .unwrap();
        (tmp, repo)
    }

    async fn commit_on_branch(repo: &std::path::Path, branch: &str, file: &str) {
        use crate::engine::git_ops::git_cmd;
        git_cmd(&["checkout", "-q", "-b", branch], repo)
            .await
            .unwrap();
        std::fs::write(repo.join(file), "body").unwrap();
        git_cmd(&["add", "-A"], repo).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "work"], repo)
            .await
            .unwrap();
        git_cmd(&["checkout", "-q", "main"], repo).await.unwrap();
    }

    #[tokio::test]
    async fn a_diff_probe_git_cannot_answer_is_unknown_rather_than_no_diff() {
        let (_tmp, repo) = repo_on_main().await;

        let err = probe_branch_has_diff(&repo, "gone-branch")
            .await
            .expect_err("an unresolvable ref is a git failure, not an empty diff");
        assert!(
            err.contains("gone-branch"),
            "the error should name the failing range, got: {err}"
        );
        assert!(
            crate::engine::git_ops::proposal_files_for_branch(&repo, "gone-branch")
                .await
                .is_none(),
            "the swallowing probe still answers this 'no diff', which is why it \
             must not feed the durable flag"
        );
    }

    #[tokio::test]
    async fn the_diff_probe_answers_yes_for_committed_work_and_no_for_a_bare_branch() {
        use crate::engine::git_ops::git_cmd;
        let (_tmp, repo) = repo_on_main().await;

        commit_on_branch(&repo, "with-work", "added.txt").await;
        assert_eq!(
            probe_branch_has_diff(&repo, "with-work").await,
            Ok(true),
            "a branch with a committed file has a diff"
        );

        git_cmd(&["branch", "bare", "main"], &repo).await.unwrap();
        assert_eq!(
            probe_branch_has_diff(&repo, "bare").await,
            Ok(false),
            "a branch level with main has nothing to show"
        );
    }

    #[test]
    fn validate_client_entry_rejects_oversized_fields_and_data() {
        let long = "x".repeat(CLIENT_LOG_MAX_FIELD_LEN + 1);
        assert_eq!(
            validate_client_entry(&entry(&long, "m", serde_json::Value::Null)),
            Err("category/message too long")
        );
        let mut oversized_code = entry("error", "uncaught", serde_json::Value::Null);
        oversized_code.code = Some(long);
        assert!(validate_client_entry(&oversized_code).is_err());
        let big = serde_json::json!({ "blob": "y".repeat(CLIENT_LOG_MAX_DATA_LEN) });
        assert_eq!(
            validate_client_entry(&entry("perf", "m", big)),
            Err("data too large")
        );
    }
}
