use super::*;
use crate::engine::cc_permission::prompt_coding_agent_permission;
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
/// `AskUserQuestion` is intercepted in `agent_session::run_session` (kills
/// CC, renders a `QuestionCard`); routing it through the permission tool
/// would surface a redundant "Allow?" card stacked on top of the question
/// itself. Auto-approve at this gate so the user only sees the question.
fn should_auto_allow(tool_name: &str) -> bool {
    matches!(tool_name, "AskUserQuestion")
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

    let outcome = prompt_coding_agent_permission(
        state.engine.pool(),
        &state.engine.event_bus,
        &state.engine.pending_cc_permission,
        thread_id,
        body.tool_use_id,
        body.tool_name,
        body.input,
    )
    .await;

    Json(PermissionPromptResponse {
        allowed: outcome.allowed,
        reason: outcome.reason,
    })
    .into_response()
}

#[derive(Deserialize)]
pub(super) struct ClientLogRequest {
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// POST /api/v1/internal/client-log — fire-and-forget breadcrumb channel for
/// browser-side telemetry that needs engine-log persistence. Body capped at
/// 4KB so the engine.log tail isn't drowned by a misbehaving client.
pub(super) async fn client_log(
    headers: HeaderMap,
    Json(body): Json<ClientLogRequest>,
) -> impl IntoResponse {
    const MAX_FIELD_LEN: usize = 256;
    const MAX_DATA_LEN: usize = 4096;
    if body.category.len() > MAX_FIELD_LEN || body.message.len() > MAX_FIELD_LEN {
        return (StatusCode::BAD_REQUEST, "category/message too long").into_response();
    }
    // Value's Display is infallible — it serializes through a String buffer.
    let data_str = body.data.to_string();
    if data_str.len() > MAX_DATA_LEN {
        return (StatusCode::BAD_REQUEST, "data too large").into_response();
    }
    let ua = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    crate::log!(
        "[Client/{}] {} {} ua={}",
        body.category,
        body.message,
        data_str,
        ua
    );
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(super) struct MarkHardenedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
}

/// POST /api/v1/internal/mark-hardened — invoked by `lucidos hardened mark` from
/// the `mark-harden.sh` hook after Claude Code finishes `/harden`. Replaces
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
pub(super) struct CcEditPrereadQuery {
    pub thread_id: String,
    pub file_path: String,
    /// Tool being checked — `"Edit"` or `"Write"`. Distinguishes the
    /// deny-message wording and the diagnostic log so failures can be
    /// attributed to the responsible matcher.
    #[serde(default)]
    pub tool: Option<String>,
    /// Opaque CC-process identifier from the PreToolUse hook payload's
    /// `session_id` field. Diagnostic-only: lets the log correlate
    /// hook-allows-then-CC-rejects events with the CC process whose
    /// in-memory read-tracking drifted.
    #[serde(default)]
    pub cc_session_id: Option<String>,
}

#[derive(Serialize)]
struct CcEditPrereadResponse {
    /// True iff this thread has a prior `CodingAgentToolCalled` for `Read`
    /// or `Write` against `file_path`. The hook treats this as "CC's
    /// internal Edit pre-read tracking will accept this Edit" and allows
    /// the call through. False means the next Edit will be rejected by CC
    /// — the hook then preempts that with a clearer deny.
    has_recent_read: bool,
}

/// GET /api/v1/internal/cc-edit-preread?thread_id=<uuid>&file_path=<abs-path>
///                                  &tool=<Edit|Write>&cc_session_id=<id>
///
/// Invoked by the lucidos-cli `cc-edit-preread` PreToolUse hook from inside
/// a Claude Code subprocess every time CC's Edit or Write tool fires. The hook
/// turns a `false` response into a `permissionDecision: "deny"` with an
/// explicit "Read first, then retry" reason so the model is forced into
/// the correct loop instead of bouncing off CC's internal `<tool_use_error>
/// File has not been read yet</tool_use_error>` message.
///
/// Read state lives in the events table (every CC tool call is persisted
/// as `CodingAgentToolCalled`), so the lookup survives engine restart and
/// session resume — same boundary as CC's own session tracking. `Write`
/// is included because CC accepts an Edit on a file it just Wrote.
/// `Edit` itself is NOT included: a prior failed Edit would otherwise
/// satisfy the check and bypass the loop the hook exists to break.
pub(super) async fn cc_edit_preread_check(
    State(state): State<AppState>,
    Query(q): Query<CcEditPrereadQuery>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&q.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };
    let tool = q.tool.as_deref().unwrap_or("Edit");

    let row: Result<Option<bool>, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM events \
             WHERE thread_id = $1 \
               AND event_type = 'CodingAgentToolCalled' \
               AND payload->>'name' IN ('Read', 'Write') \
               AND payload->'args'->>'file_path' = $2 \
         )",
    )
    .bind(thread_id)
    .bind(&q.file_path)
    .fetch_optional(state.engine.pool())
    .await;

    let has_recent_read = match row {
        Ok(Some(b)) => b,
        Ok(None) => false,
        Err(e) => {
            crate::log!(
                "[CcEditPreread] DB lookup failed for thread {} path {}: {} — falling back to allow to avoid blocking on engine error",
                thread_id,
                q.file_path,
                e
            );
            // Fail open: don't turn a transient DB error into a hard deny
            // for every Edit. CC's own internal check is still authoritative.
            true
        }
    };

    crate::log!(
        "[CcEditPreread] check tool={} thread={} path={} cc_session={} has_recent_read={}",
        tool,
        thread_id,
        q.file_path,
        q.cc_session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("<unset>"),
        has_recent_read,
    );

    Json(CcEditPrereadResponse { has_recent_read }).into_response()
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
/// `should_propose_change_at_idle`). This endpoint exists so those tests can
/// exercise the apply endpoint against a real projection-resident change
/// without recreating the entire CC turn flow.
///
/// Hardened so it can't be abused on a production instance:
/// 1. Refuses outright in release builds (`cfg!(debug_assertions)` is false).
///    The route is mounted unconditionally so `cargo build --release` doesn't
///    silently miss it; the guard returns 404 instead.
/// 2. Path-validates `repo_root`, `branch_name`, and every entry of `files`
///    against `..`, leading `/`, and leading `\` per the rust.md path-validation
///    rule, so even a dev-build instance reachable from the network can't be
///    coaxed into running git ops outside a sane path.
pub(super) async fn seed_change_for_test(
    State(state): State<AppState>,
    Json(body): Json<SeedChangeForTestRequest>,
) -> impl IntoResponse {
    if !cfg!(debug_assertions) {
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

    let answers =
        crate::engine::agent_question::build_hook_answers(&answer_kinds, &body.questions);
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
        .route(
            "/internal/permission-prompt",
            post(permission_prompt),
        )
        .route(
            "/internal/ask-user-question",
            post(ask_user_question),
        )
        .route("/internal/mark-hardened", post(mark_hardened))
        .route("/internal/hardened-state", get(query_hardened))
        .route(
            "/internal/cc-edit-preread",
            get(cc_edit_preread_check),
        )
        .route("/internal/client-log", post(client_log))
        .route(
            "/internal/seed-change-for-test",
            post(seed_change_for_test),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn askuserquestion_is_auto_allowed_to_avoid_redundant_card() {
        // The engine intercepts AskUserQuestion in run_session and renders
        // the QuestionCard directly; routing it through the permission gate
        // would stack a redundant "Allow?" card on top of the question.
        assert!(
            should_auto_allow("AskUserQuestion"),
            "AskUserQuestion must short-circuit the permission gate"
        );
    }

    #[test]
    fn other_tools_are_not_auto_allowed() {
        // Bash, Edit, Write, etc. must continue to render permission cards
        // so the user can deny them. Only AskUserQuestion is special.
        for tool in ["Edit", "Bash", "Write", "Read", "Glob", "Grep", "Skill"] {
            assert!(
                !should_auto_allow(tool),
                "{} must NOT short-circuit the permission gate",
                tool
            );
        }
    }
}
