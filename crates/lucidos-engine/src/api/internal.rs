use super::*;
use crate::engine::cc_permission::{prompt_coding_agent_permission, CodingAgentPermissionInput};
use crate::engine::event_bus::BusEvent;
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

/// Tools the engine handles before CC's permission gate can render a card.
/// A question tool is intercepted in `agent_session::run_session` (which
/// renders a `QuestionCard` from the `UserQuestionAsked` this module's
/// `ask_user_question` handler emits); routing it through the permission tool
/// would surface a redundant "Allow?" card stacked on top of the question
/// itself. Auto-approve at this gate so the user only sees the question.
///
/// The names come from [`crate::runtime::is_user_question_tool`], shared with
/// the tool-call suppression in `run_session` so the two gates cannot know
/// different sets. They did, and CC reaching the question tool over MCP fell
/// through both: see that function.
fn should_auto_allow(tool_name: &str) -> bool {
    crate::runtime::is_user_question_tool(tool_name)
}

/// POST /api/v1/internal/permission-prompt — invoked by the lucidos-cli
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
    Json(body): Json<PermissionPromptRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    if should_auto_allow(&body.tool_name) {
        return Json(PermissionPromptResponse {
            allowed: true,
            reason: None,
        })
        .into_response();
    }

    // A file write landing inside this thread's own worktree skips the card —
    // resolve the root the same way the Codex bridge does.
    let worktree_path = crate::engine::cc_permission::lookup_session_worktree(
        &state.engine.agent_sessions,
        thread_id,
    )
    .await;

    let outcome = prompt_coding_agent_permission(
        state.engine.pool(),
        &state.engine.event_bus,
        &state.engine.pending_cc_permission,
        &state.engine.trigger_configs,
        &state.workspace_path,
        worktree_path.as_deref(),
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

/// POST /api/v1/internal/restart-intent: the workspace gateway telling this
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
///    the gateway origin, including a same-origin app iframe, could otherwise
///    set this engine's restart actor and so defeat the crash-loop protection in
///    *cause-gated resume*. See `base_path::arrived_through_gateway_proxy` for
///    why provenance rather than peer address is the discriminator.
///  * A caller with no resolvable DEVICE is rejected (400).
///    `user_actor_resolved` falls back to `Api { mode: Human }` with no device
///    id, and that is not the switch fingerprint (which needs
///    `actor.kind = 'device'`), so stashing it would not resume anything while
///    still replacing the honest "System" attribution with an API caller. The
///    gateway skips the call entirely when it has no device to name; this is the
///    engine-side floor under that.
pub(super) async fn restart_intent(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if crate::api::base_path::arrived_through_gateway_proxy(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "restart-intent is not reachable through the gateway proxy",
        )
            .into_response();
    }
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let Some(actor @ crate::engine::thread_events::MessageOrigin::Device { .. }) = actor else {
        return (
            StatusCode::BAD_REQUEST,
            "restart-intent requires a device actor",
        )
            .into_response();
    };
    // First writer wins: an in-workspace *Switch* stashed its own actor before it
    // asked the gateway for this respawn, and that one is the click's.
    let stashed = state.engine.stash_restart_actor(Some(actor));
    crate::log!(
        "[Restart] Gateway restart intent recorded (stashed={})",
        stashed
    );
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(super) struct ClientLogRequest {
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

const CLIENT_LOG_MAX_FIELD_LEN: usize = 256;
const CLIENT_LOG_MAX_DATA_LEN: usize = 4096;
/// Max entries one batched `/internal/client-logs` request may carry, so a
/// misbehaving client can't flood the engine.log tail in a single POST. The
/// frontend perf queue flushes well under this.
const CLIENT_LOG_MAX_BATCH: usize = 100;

/// Validate one breadcrumb's caps, returning its serialized `data` on success.
/// `Err(reason)` on a cap violation (the caller maps it to a 400). Pure — does
/// NOT log, so a batch can validate every entry before committing any line.
fn validate_client_entry(entry: &ClientLogRequest) -> Result<String, &'static str> {
    if entry.category.len() > CLIENT_LOG_MAX_FIELD_LEN
        || entry.message.len() > CLIENT_LOG_MAX_FIELD_LEN
    {
        return Err("category/message too long");
    }
    // Value's Display is infallible — it serializes through a String buffer.
    let data_str = entry.data.to_string();
    if data_str.len() > CLIENT_LOG_MAX_DATA_LEN {
        return Err("data too large");
    }
    Ok(data_str)
}

fn user_agent(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// POST /api/v1/internal/client-log — fire-and-forget breadcrumb channel for
/// browser-side telemetry that needs engine-log persistence. Body capped at
/// 4KB so the engine.log tail isn't drowned by a misbehaving client.
pub(super) async fn client_log(
    headers: HeaderMap,
    Json(body): Json<ClientLogRequest>,
) -> impl IntoResponse {
    match validate_client_entry(&body) {
        Ok(data_str) => {
            crate::log!(
                "[Client/{}] {} {} ua={}",
                body.category,
                body.message,
                data_str,
                user_agent(&headers)
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(reason) => (StatusCode::BAD_REQUEST, reason).into_response(),
    }
}

/// POST /api/v1/internal/client-logs — batched variant of `client-log` so a
/// client-side queue (e.g. the perf instrumentation) can flush many breadcrumbs
/// in one request instead of one POST per sample. Each valid entry is logged as
/// its own `[Client/<category>]` line; the array length and per-entry sizes are
/// capped. Validation is atomic — if any entry is over-cap the whole batch is
/// rejected (400) and nothing is logged.
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
        crate::log!(
            "[Client/{}] {} {} ua={}",
            entry.category,
            entry.message,
            data_str,
            ua
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(super) struct MarkHardenedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
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

/// POST /api/v1/internal/coding-agent-diff-refresh — invoked by the
/// coding-agent worktree's git post-commit hook via `lucidos
/// coding-agent-diff-hook`.
///
/// This is a live UI refresh only: it reconciles `coding_agent_has_diff` from
/// git truth and broadcasts the updated aggregate when the flag changes. It
/// does NOT emit `ChangeProposed`, create a `changes` row, or mark the turn as
/// ready for Apply. Formal proposal still happens when the coding-agent turn
/// idles.
pub(super) async fn coding_agent_diff_refresh(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<CodingAgentDiffRefreshRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    match crate::api::actor::subprocess_origin(&headers) {
        crate::api::actor::SubprocessOrigin::Subprocess {
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
    let has_diff = crate::engine::git_ops::proposal_files_for_branch(&repo_root, branch_name)
        .await
        .is_some();

    match state
        .engine
        .event_bus
        .set_coding_agent_has_diff_and_broadcast(thread_id, has_diff)
        .await
    {
        Ok(changed) => Json(CodingAgentDiffRefreshResponse { has_diff, changed }).into_response(),
        Err(e) => {
            crate::log!(
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

/// POST /api/v1/internal/mark-hardened, invoked by `lucidos hardened mark`,
/// which `/harden` Phase 5 runs once every phase completes. Replaces
/// the prior worktree-keyed file marker, which was lost when stale-session
/// recovery removed the worktree before the apply check ran.
pub(super) async fn mark_hardened(
    State(state): State<AppState>,
    Json(body): Json<MarkHardenedRequest>,
) -> impl IntoResponse {
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    match state
        .engine
        .record_hardened(&repo_root, &body.branch_name, &body.head_sha)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            crate::log!(
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

#[derive(Deserialize)]
pub(super) struct QueryHardenedQuery {
    pub repo_root: String,
    pub branch_name: String,
}

#[derive(Serialize)]
struct QueryHardenedResponse {
    state: &'static str,
}

/// Body for `POST /api/v1/internal/mark-planned`. `state` is `"proposed"` (a
/// plan was written and awaits the user's approval — what the skill records),
/// `"planned"` (legacy/approved), `"acknowledged_simple"` (local fix), or
/// `"bounded_security_fix"` (an unattended run's scoped security fix).
/// `plan_path` accompanies the plan states, `reason` the other two, and `files`
/// the bounded fix alone (each ignored for the kinds it does not belong to).
#[derive(Deserialize)]
pub(super) struct MarkPlannedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
    pub state: String,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Repo-relative paths a `bounded_security_fix` is confined to. Must be
    /// empty for every other state.
    #[serde(default)]
    pub files: Vec<String>,
}

/// POST /api/v1/internal/mark-planned — invoked by `lucidos planned mark` from
/// inside a coding-agent subprocess (and by the `implementation-plan` skill
/// after it writes the plan file). Records the durable Planned marker that the
/// `cc-plan-gate` PreToolUse hook and the Apply floor read. Mirrors
/// `mark_hardened`.
///
/// A rejected file list answers 400 with the rejection's own text, not 500. The
/// caller is an agent that can fix the call, so the body has to say how. A 500
/// reads as an engine fault and gets retried unchanged.
pub(super) async fn mark_planned(
    State(state): State<AppState>,
    Json(body): Json<MarkPlannedRequest>,
) -> impl IntoResponse {
    use crate::engine::git_ops::{validate_plan_files, PlanMarkerKind};
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    // Strict on the WRITE path. The lenient `parse` reads an unknown value as
    // `planned`, which on a write would record "a human approved this" for a
    // typo. `bounded-security-fix` is the one that matters.
    let Some(kind) = PlanMarkerKind::parse_strict(&body.state) else {
        let msg = format!(
            "Unknown plan-marker state {:?}. Use one of: proposed, planned, \
             acknowledged_simple, bounded_security_fix (snake_case, not kebab-case).",
            body.state
        );
        crate::log!(
            "[Internal] mark-planned refused for {}: {}",
            body.branch_name,
            msg
        );
        return (StatusCode::BAD_REQUEST, msg).into_response();
    };
    let files = match validate_plan_files(kind, &body.files) {
        Ok(files) => files,
        Err(rejection) => {
            crate::log!(
                "[Internal] mark-planned refused for {}: {}",
                body.branch_name,
                rejection
            );
            return (StatusCode::BAD_REQUEST, rejection.to_string()).into_response();
        }
    };
    match state
        .engine
        .record_planned(
            &repo_root,
            &body.branch_name,
            kind,
            body.plan_path.as_deref(),
            body.reason.as_deref(),
            &files,
            &body.head_sha,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            crate::log!(
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

/// GET /api/v1/internal/planned-state?repo_root=...&branch_name=... — invoked by
/// `lucidos planned state` (printing) and the `cc-plan-gate` hook (deciding).
/// Returns `{ state: "SATISFIED" | "PROPOSED" | "MISSING", kind: "planned" |
/// "acknowledged_simple" | "proposed" | "bounded_security_fix" | null }`. The
/// three-way `state` lets the hook distinguish an awaiting-approval `PROPOSED`
/// marker (deny: "get approval, then run `planned approve`") from a `MISSING`
/// one (deny: "run the skill"). `kind` is diagnostic: the hook decides on
/// `state` alone, and the file bound a `bounded_security_fix` carries is
/// enforced at the Apply floor rather than here.
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
        PlanMarkerState::Missing => ("MISSING", None),
    };
    Json(QueryPlannedResponse { state: label, kind }).into_response()
}

#[derive(Serialize)]
struct QueryPlannedResponse {
    state: &'static str,
    kind: Option<&'static str>,
}

/// Body for `POST /api/v1/internal/approve-plan`.
#[derive(Deserialize)]
pub(super) struct ApprovePlanRequest {
    pub repo_root: String,
    pub branch_name: String,
}

#[derive(Serialize)]
struct ApprovePlanResponse {
    /// Whether a `proposed` row was flipped to `planned`. `false` means there
    /// was nothing to approve (no row, or it was already planned / simple).
    approved: bool,
}

/// POST /api/v1/internal/approve-plan — invoked by `lucidos planned approve`
/// after the user approves a proposed plan in chat. Flips the branch's marker
/// from `proposed` to `planned` so the cc-plan-gate hook and the Apply floor
/// pass. Idempotent: re-approving an already-`planned` (or `simple`) branch is a
/// no-op that reports `approved: false`.
pub(super) async fn approve_plan(
    State(state): State<AppState>,
    Json(body): Json<ApprovePlanRequest>,
) -> impl IntoResponse {
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    match state
        .engine
        .approve_plan(&repo_root, &body.branch_name)
        .await
    {
        Ok(approved) => Json(ApprovePlanResponse { approved }).into_response(),
        Err(e) => {
            crate::log!(
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

/// GET /api/v1/internal/hardened-state?repo_root=...&branch_name=... — invoked by
/// `lucidos hardened query` so the `harden.md` skill and `pre-push.sh` hook
/// share the DB-backed marker that `mark-hardened` writes.
pub(super) async fn query_hardened(
    State(state): State<AppState>,
    Query(q): Query<QueryHardenedQuery>,
) -> impl IntoResponse {
    use crate::engine::git_ops::HardenMarkerState;
    let repo_root = std::path::PathBuf::from(&q.repo_root);
    let label = match state
        .engine
        .harden_marker_state(&repo_root, &q.branch_name)
        .await
    {
        HardenMarkerState::Fresh => "FRESH",
        HardenMarkerState::Stale => "STALE",
        HardenMarkerState::Missing => "MISSING",
    };
    Json(QueryHardenedResponse { state: label }).into_response()
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
    /// Whether to also record a Planned marker for the seeded branch. Defaults
    /// to `true` (omitted) so existing apply tests — which target the harden
    /// gate or apply mechanics, not the plan floor — keep passing. The negative
    /// plan-floor test passes `false` to exercise the block.
    #[serde(default)]
    pub planned: Option<bool>,
}

/// POST /api/v1/internal/seed-change-for-test — emit an aggregate `ChangeProposed`
/// directly via the live EventBus, populating the `ChangesProjection` (and the
/// `changes` table row inside the same commit tx) without going through the
/// agent session lifecycle. The aggregate emit flow (`propose_change`) requires
/// a live `agent_sessions` entry, which integration tests can't set up from
/// outside the engine process.
///
/// Used only by the api e2e tests in `crates/lucidos-e2e/tests/api_support/changes_test.rs`.
/// Production code emits the aggregate `ChangeProposed` exclusively via the
/// agent session's end-of-turn aggregation (`propose_change`, gated by
/// `may_touch_change_state_at_idle`). This endpoint exists so those tests can
/// exercise the apply endpoint against a real projection-resident change
/// without recreating the entire CC turn flow.
///
/// Hardened so it can't be abused on a production instance:
/// 1. Refuses unless the build is a dev build (`debug_assertions`) OR was compiled
///    with the `e2e-test-hooks` cargo feature — the feature the e2e harness passes
///    (`scripts/lib/e2e.sh` `ENGINE_BUILD_FEATURES`), so a RELEASE e2e engine can
///    still seed. This mirrors the `#[cfg(feature = "e2e-test-hooks")]` gating on
///    `api/notifications.rs`'s test endpoints. A prod/packaged build
///    (`cargo tauri build`, no feature) sets neither flag, so the guard returns
///    404. The route stays mounted unconditionally so a release build doesn't
///    silently miss it.
/// 2. Path-validates `repo_root`, `branch_name`, and every entry of `files`
///    against `..`, leading `/`, and leading `\` per the rust.md path-validation
///    rule, so even a dev-build instance reachable from the network can't be
///    coaxed into running git ops outside a sane path.
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
    // repo_root may be absolute (it's a filesystem path to a git repo), but
    // must not contain `..` segments. branch_name and per-file paths are
    // relative-ish — reject absolute and traversal both.
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

    // Capture the marker key before `body` fields are moved into the event.
    let repo_root_for_marker = body.repo_root.clone();
    let branch_for_marker = body.branch_name.clone();
    let record_marker = body.planned != Some(false);

    let result = state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ChangeProposed {
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
                path: String::new(),
                diff: String::new(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await;

    // Record a Planned marker for the seeded branch unless explicitly opted
    // out (the negative plan-floor test). Mirrors how `hardened` lets seeded
    // changes satisfy the harden gate — seeded "ready to apply" changes would
    // carry a marker in reality. `record_planned` canonicalizes `repo_root` the
    // same way the Apply floor does, so the lookup matches.
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
            crate::log!(
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
            crate::log!("[Internal] seed-change-for-test emit failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("emit failed: {e}"),
            )
                .into_response()
        }
    }
}

/// Reject relative paths that escape their parent (`..`), are absolute
/// (leading `/` or `\`), or are empty. For values like branch names and
/// per-file paths inside a repo. Returns the rejection reason or `None`.
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
    /// Pass-through of CC's `tool_input.questions` array. The endpoint stores
    /// nothing beyond the lifetime of this handler call; the CC hook will POST
    /// it again on engine restart (crash recovery).
    pub questions: serde_json::Value,
}

#[derive(Serialize)]
struct AskUserQuestionResponse {
    /// Echoed verbatim into the hook's `updatedInput.questions`.
    questions: serde_json::Value,
    /// `{question_text: chosen_label}` — the hook's `updatedInput.answers`.
    answers: serde_json::Value,
}

/// POST /api/v1/internal/ask-user-question — two callers, same contract:
/// the lucidos-cli `ask-user-question-hook` subcommand (from inside a Claude
/// Code subprocess when CC fires the `AskUserQuestion` PreToolUse hook) and
/// the lucidos-cli `mcp-permission-server`'s `ask_user_question` MCP tool
/// (from inside a Codex session — one question per call, answer returned as
/// the MCP tool result so the turn continues in place).
///
/// CC's tool schema accepts 1–4 questions per call. Lucidos renders them
/// **one at a time** so the user only ever sees a single question card on
/// screen — see `synth_question_id` for the per-question key scheme.
/// The handler walks the question list sequentially: register a waiter for
/// question `i`, fast-path return any prior `UserQuestionAnswered` from a
/// pre-restart session, otherwise emit `UserQuestionAsked` and long-poll
/// until the user picks. Once every question has an answer (or one was
/// canceled and we short-circuit), the combined `{question_text: label}`
/// map goes back to CC as a single tool result — CC sees one tool call.
pub(super) async fn ask_user_question(
    State(state): State<AppState>,
    Json(body): Json<AskUserQuestionRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    let outcome = state
        .engine
        .walk_question_batch(
            thread_id,
            &body.tool_use_id,
            &body.questions,
            body.session_id.clone(),
            EventChannel::ClaudeCode,
        )
        .await;

    let answer_kinds = match outcome {
        Ok(o) => o.answer_kinds,
        Err(e) => {
            // The hook protocol doesn't model errors — a non-200 leaves
            // CC stuck. But we DO want to surface infrastructure
            // failures; the alternative (silently returning an empty
            // answers map) makes CC re-ask in a loop. 500 here lets the
            // hook stderr-log the failure and the user retries.
            crate::log!(
                "[AskUserQuestion] walk failed for {thread_id}/{}: {e}",
                body.tool_use_id
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    // Empty answers — CC sent zero questions. Return the empty map; CC
    // will read the tool result and decide what to do (typically: reissue
    // with a real question).
    if answer_kinds.is_empty() {
        crate::log!(
            "[AskUserQuestion] CC sent zero questions for {thread_id}/{}",
            body.tool_use_id
        );
        return Json(AskUserQuestionResponse {
            questions: body.questions,
            answers: serde_json::Value::Object(serde_json::Map::new()),
        })
        .into_response();
    }

    let answers = crate::engine::agent_question::build_hook_answers(&answer_kinds, &body.questions);
    Json(AskUserQuestionResponse {
        questions: body.questions,
        answers,
    })
    .into_response()
}

/// Routes for the `/internal/*` surface (engine-internal hooks used by CC
/// sessions and the e2e harness).
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/permission-prompt", post(permission_prompt))
        .route("/internal/ask-user-question", post(ask_user_question))
        .route("/internal/restart-intent", post(restart_intent))
        .route("/internal/mark-hardened", post(mark_hardened))
        .route(
            "/internal/coding-agent-diff-refresh",
            post(coding_agent_diff_refresh),
        )
        .route("/internal/hardened-state", get(query_hardened))
        .route("/internal/mark-planned", post(mark_planned))
        .route("/internal/planned-state", get(query_planned))
        .route("/internal/approve-plan", post(approve_plan))
        .route("/internal/client-log", post(client_log))
        .route("/internal/client-logs", post(client_logs))
        .route("/internal/seed-change-for-test", post(seed_change_for_test))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_question_tool_is_auto_allowed_to_avoid_redundant_card() {
        // The engine intercepts a question tool in run_session and renders the
        // QuestionCard directly; routing it through the permission gate would
        // stack a redundant "Allow?" card on top of the question.
        //
        // BOTH names CC can reach, not just its native one. The MCP tool is
        // advertised to CC by the very permission server this endpoint serves,
        // so a question raised through it used to ask the user for permission
        // to ask them a question.
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
        // Bash, Edit, Write, etc. must continue to render permission cards so
        // the user can deny them. `approve` is the sharpest case: it is the
        // OTHER tool on the same MCP server, and auto-allowing by server rather
        // than by tool would nullify the permission gate entirely.
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

    fn entry(category: &str, message: &str, data: serde_json::Value) -> ClientLogRequest {
        ClientLogRequest {
            category: category.into(),
            message: message.into(),
            data,
        }
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

    #[test]
    fn validate_client_entry_rejects_oversized_fields_and_data() {
        let long = "x".repeat(CLIENT_LOG_MAX_FIELD_LEN + 1);
        assert_eq!(
            validate_client_entry(&entry(&long, "m", serde_json::Value::Null)),
            Err("category/message too long")
        );
        let big = serde_json::json!({ "blob": "y".repeat(CLIENT_LOG_MAX_DATA_LEN) });
        assert_eq!(
            validate_client_entry(&entry("perf", "m", big)),
            Err("data too large")
        );
    }
}
