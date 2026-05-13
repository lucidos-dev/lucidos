use super::*;
use crate::engine::cc_permission::{
    CcPermissionEntry, CcPermissionState, DedupKey, DENIAL_REASON, SESSION_ALLOW_REASON,
};
use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
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

/// POST /api/internal/permission-prompt — invoked by the lucidos-cli
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
/// Concurrent identical requests (same `thread_id` + `tool_name` + `input`)
/// dedup onto one canonical entry: the first emits the event, every
/// subsequent identical request subscribes to the same broadcast. One click
/// answers them all.
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

    // Pre-prompt session-allow check: if the user previously clicked "Allow
    // for this thread" on a request whose `Session` pattern matches this one,
    // skip the prompt entirely and answer the MCP call immediately. The
    // matching pattern is derived from this prompt's input, so it works for
    // tools/paths CC's `--allowedTools` doesn't reach (notably `.claude/`
    // and `.git/` writes, which CC always routes through the prompt).
    let session_pattern = derive_allow_pattern(&body.tool_name, &body.input, AllowScope::Session);
    let is_session_allowed = match session_pattern.as_deref() {
        Some(p) => {
            let pending = state.engine.pending_cc_permission.lock().unwrap();
            pending.matches_session_allow(thread_id, p)
        }
        None => false,
    };
    if is_session_allowed {
        return Json(PermissionPromptResponse {
            allowed: true,
            reason: Some(SESSION_ALLOW_REASON.to_string()),
        })
        .into_response();
    }

    let canonical_input =
        serde_json::to_string(&body.input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, body.tool_name.clone(), canonical_input);
    let summary = build_summary(&body.tool_name, &body.input);

    let (request_id, mut rx, is_canonical) = {
        let mut pending = state.engine.pending_cc_permission.lock().unwrap();
        register_or_attach(
            &mut pending,
            dedup_key,
            thread_id,
            body.tool_name.clone(),
            body.input.clone(),
        )
    };

    if is_canonical {
        state
            .engine
            .event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPermissionRequest {
                        request_id: request_id.clone(),
                        tool_use_id: body.tool_use_id,
                        tool_name: body.tool_name,
                        input: body.input,
                        summary,
                    },
                    meta: EventMeta::NONE,
                },
                "[Internal] CodingAgentPermissionRequest",
            )
            .await;
    }

    // Wait forever for the user. The paired `CodingAgentPermissionResolved`
    // is emitted by `submit_mcp_consent` (so it fires once per click, not
    // once per deduped listener).
    let allowed = rx.recv().await.unwrap_or(false);
    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };

    Json(PermissionPromptResponse { allowed, reason }).into_response()
}

/// Look up `dedup_key`. If a canonical entry already exists (a duplicate
/// concurrent request), subscribe and reuse its `request_id`. Otherwise
/// create a fresh entry, register both indexes, and return its receiver.
///
/// Returns `(request_id, receiver, is_canonical)`. The caller emits the
/// `CodingAgentPermissionRequest` event only when `is_canonical` is true.
fn register_or_attach(
    state: &mut CcPermissionState,
    dedup_key: DedupKey,
    thread_id: Uuid,
    tool_name: String,
    input: serde_json::Value,
) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
    // Opportunistic sweep: each new prompt is a chance to evict orphans
    // whose HTTP handlers were canceled (CC died, MCP request aborted) and
    // would otherwise leak until engine restart.
    state.gc_dead_entries();
    if let Some(entry) = state.by_dedup_key.get(&dedup_key) {
        return (entry.request_id.clone(), entry.tx.subscribe(), false);
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::broadcast::channel(1);
    state.by_dedup_key.insert(
        dedup_key.clone(),
        CcPermissionEntry {
            thread_id,
            request_id: request_id.clone(),
            tool_name,
            input,
            tx,
        },
    );
    state.by_request_id.insert(request_id.clone(), dedup_key);
    (request_id, rx, true)
}

#[derive(Deserialize)]
pub(super) struct ClientLogRequest {
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// POST /api/internal/client-log — fire-and-forget breadcrumb channel for
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

/// POST /api/internal/mark-hardened — invoked by `lucidos hardened mark` from
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

/// GET /api/internal/hardened-state?repo_root=...&branch_name=... — invoked by
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

/// GET /api/internal/cc-edit-preread?thread_id=<uuid>&file_path=<abs-path>
///
/// Invoked by the lucidos-cli `cc-edit-preread` PreToolUse hook from inside
/// a CC subprocess every time CC's Edit tool fires. The hook turns a
/// `false` response into a `permissionDecision: "deny"` with an explicit
/// "Read first, then retry Edit" reason, so the model is forced into the
/// correct loop instead of bouncing off CC's internal `<tool_use_error>
/// File has not been read yet</tool_use_error>` message.
///
/// Read state lives in the events table (we track every CC tool call as
/// `CodingAgentToolCalled`), so the lookup survives engine restart and
/// session resume — same boundary as CC's own session tracking. We
/// include `Write` because CC accepts an Edit on a file it has just
/// Written (creating implies knowing the content). We deliberately do
/// not include `Edit` itself: a prior failed Edit would otherwise
/// satisfy the check and bypass the loop we're trying to break.
pub(super) async fn cc_edit_preread_check(
    State(state): State<AppState>,
    Query(q): Query<CcEditPrereadQuery>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&q.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

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

    Json(CcEditPrereadResponse { has_recent_read }).into_response()
}

#[derive(Deserialize)]
pub(super) struct CommitMadeRequest {
    pub thread_id: String,
    pub sha: String,
}

#[derive(Serialize)]
struct CommitMadeResponse {
    emitted: bool,
}

/// POST /api/internal/commit-made — invoked by the per-worktree
/// `post-commit` git hook installed when CC's worktree is created.
///
/// Phase 4.2 of the CC resume architecture: emit one `ChangeProposed`
/// event per commit (in real time as CC commits), instead of one
/// aggregated `ChangeProposed` at end-of-turn. The end-of-turn path
/// still writes a row to the `changes` table (Phase 10 will drop it),
/// but no longer emits a per-turn aggregate event.
///
/// Validates the thread_id refers to a real thread, then resolves the
/// worktree path either from the in-memory agent session map (the common
/// case — CC is alive while the hook fires) or via `git worktree list`
/// fallback. Runs `git show --stat --format=%s <sha>` to extract the
/// commit subject and changed-file list, then emits a `ChangeProposed`
/// event keyed by `commit_sha`.
///
/// The hook is best-effort: if the engine is down, the curl call in the
/// hook fails silently (`|| true`). End-of-turn re-scan still creates the
/// `changes` table row, so Apply still works — only the granular
/// per-commit event is lost. Acceptable for now.
pub(super) async fn commit_made(
    State(state): State<AppState>,
    Json(body): Json<CommitMadeRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    // Validate the thread exists. Don't trust the hook's payload — a stray
    // hook from a deleted worktree should not be able to inject events into
    // arbitrary threads.
    let exists: bool = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)",
    )
    .bind(thread_id)
    .fetch_one(state.engine.pool())
    .await
    {
        Ok(b) => b,
        Err(e) => {
            crate::log!("[Internal] commit-made thread lookup failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response();
        }
    };
    if !exists {
        return (StatusCode::NOT_FOUND, "Unknown thread_id").into_response();
    }

    if body.sha.trim().is_empty() || body.sha.len() > 64 || !body.sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return (StatusCode::BAD_REQUEST, "Invalid sha").into_response();
    }

    // Resolve worktree path. Live session is the common case; fall back to
    // the workspace's worktrees_dir if the session has already exited.
    let (worktree_path, branch_name): (std::path::PathBuf, Option<String>) = {
        let sessions = state.engine.agent_sessions.lock().await;
        match sessions.get(&thread_id) {
            Some(s) => match (&s.worktree_path, &s.branch_name) {
                (Some(wt), branch) => (wt.clone(), branch.clone()),
                _ => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "No worktree path on live session",
                    )
                        .into_response();
                }
            },
            None => {
                // No live session — best-effort lookup is out of scope here;
                // skip emission. End-of-turn DB row still records the change.
                crate::log!(
                    "[Internal] commit-made for thread {} with no live session — skipping per-commit emit",
                    thread_id
                );
                return Json(CommitMadeResponse { emitted: false }).into_response();
            }
        }
    };

    let branch = branch_name.unwrap_or_else(|| "<unknown>".to_string());
    match emit_change_proposed_for_commit(
        &state.engine.event_bus,
        thread_id,
        &worktree_path,
        &body.sha,
        &branch,
    )
    .await
    {
        Ok(()) => Json(CommitMadeResponse { emitted: true }).into_response(),
        Err(CommitEmitError::GitShowFailed(msg)) => {
            crate::log!(
                "[Internal] git show failed for {} in {}: {}",
                body.sha,
                worktree_path.display(),
                msg
            );
            (StatusCode::BAD_REQUEST, "git show failed").into_response()
        }
        Err(CommitEmitError::GitError(e)) => {
            crate::log!("[Internal] git show errored: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "git error").into_response()
        }
    }
}

#[derive(Debug)]
enum CommitEmitError {
    GitShowFailed(String),
    GitError(String),
}

/// Core of the `commit-made` endpoint extracted for testability.
///
/// Reads the commit's subject + changed-file list via `git show`, then
/// emits a `ChangeProposed` event keyed by `commit_sha`. The HTTP handler
/// is a thin wrapper that resolves thread/worktree, then calls this.
async fn emit_change_proposed_for_commit(
    event_bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    worktree_path: &std::path::Path,
    sha: &str,
    branch: &str,
) -> Result<(), CommitEmitError> {
    let show_output = crate::engine::git_ops::git_cmd(
        &[
            "show",
            "--stat",
            "--format=%s",
            "--name-only",
            "-z",
            sha,
        ],
        worktree_path,
    )
    .await
    .map_err(CommitEmitError::GitError)?;

    if !show_output.status.success() {
        return Err(CommitEmitError::GitShowFailed(
            String::from_utf8_lossy(&show_output.stderr).trim().to_string(),
        ));
    }

    let (subject, files) = parse_show_output(&String::from_utf8_lossy(&show_output.stdout));
    let requires_restart = crate::engine::git_ops::files_require_restart(&files);

    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    change_id: String::new(),
                    description: Some(subject),
                    files,
                    requires_restart,
                    origin: None,
                    commit_sha: Some(sha.to_string()),
                    branch_name: branch.to_string(),
                    // Per-commit emits don't know the parent repo root —
                    // the projection fills this in from the matching
                    // aggregate row (same change_id) when computing.
                    repo_root: String::new(),
                    hardened: false,
                    incomplete: false,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::CodingAgent),
                    ..EventMeta::NONE
                },
            },
            "[Internal] ChangeProposed (per-commit)",
        )
        .await;

    crate::log!(
        "[Internal] Per-commit ChangeProposed emitted for thread {} sha {} (branch {})",
        thread_id,
        sha,
        branch
    );
    Ok(())
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

/// POST /api/internal/seed-change-for-test — emit an aggregate `ChangeProposed`
/// directly via the live EventBus, populating the `ChangesProjection` (and the
/// `changes` table row inside the same commit tx) without going through the
/// per-commit hook flow. The hook flow requires a live `agent_sessions` entry,
/// which integration tests can't set up from outside the engine process.
///
/// Used only by the api e2e tests in `crates/lucidos-e2e/tests/api_support/changes_test.rs`.
/// Production code emits ChangeProposed via `commit_made` (per-commit) or via
/// the agent session's end-of-turn aggregation. This endpoint exists so those
/// tests can exercise the apply endpoint against a real projection-resident
/// change without recreating the entire CC commit flow.
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
    if body.repo_root.is_empty()
        || body.repo_root.split(['/', '\\']).any(|seg| seg == "..")
    {
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
                channel: Some(EventChannel::CodingAgent),
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

/// Parse `git show --stat --format=%s --name-only -z <sha>` output into
/// `(subject, files)`. The first line is the commit subject; subsequent
/// NUL-separated tokens are the changed file paths (until `--stat` output
/// begins). We strip empties and limit to a sensible cap to defend against
/// pathological commits.
fn parse_show_output(out: &str) -> (String, Vec<String>) {
    // The output for `--format=%s --name-only -z` is:
    //   <subject>\n
    //   \n
    //   <file1>\0<file2>\0...<fileN>\0
    //   --stat human output (when --stat is also requested)
    //
    // Splitting on '\0' gives us file paths cleanly. The subject is always
    // the first line of the first \0-separated chunk.
    let mut lines = out.split('\0');
    let first = lines.next().unwrap_or_default();
    let mut iter = first.lines();
    let subject = iter.next().unwrap_or_default().trim().to_string();
    // Remaining lines in the first chunk (before the first \0) are file
    // paths separated by newlines, since git emits each path followed by \0.
    let mut files: Vec<String> = iter
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    for chunk in lines {
        // Each subsequent chunk is one file path, possibly followed by
        // --stat human output. The path is the first line.
        if let Some(path) = chunk.lines().next() {
            let path = path.trim();
            if !path.is_empty() && !path.starts_with(' ') && !files.contains(&path.to_string()) {
                files.push(path.to_string());
            }
        }
        if files.len() >= 256 {
            break;
        }
    }
    (subject, files)
}

fn build_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let arg = [
        "file_path",
        "path",
        "command",
        "notebook_path",
        "skill",
        "url",
        "pattern",
    ]
    .iter()
    .find_map(|k| input.get(k).and_then(|v| v.as_str()))
    .unwrap_or("");
    let display_name = match tool_name {
        "Skill" => "skill",
        _ => tool_name,
    };
    if arg.is_empty() {
        display_name.to_string()
    } else {
        format!("{} {}", display_name, arg)
    }
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

/// Per-question tool_use_id used in `UserQuestionAsked` / `UserQuestionAnswered`.
/// CC sends one outer `tool_use_id` per `AskUserQuestion` call regardless of how
/// many questions are inside; the engine renders them sequentially (one card on
/// screen at a time) and needs a unique key per individual question for the wait
/// registry, the answered-already crash-recovery lookup, and the partial unique
/// index on `events_user_question_answered_unique`. Synthesizing
/// `{outer}#q{i}` keeps each card independent without touching the DB schema.
fn synth_question_id(outer: &str, index: usize) -> String {
    format!("{outer}#q{index}")
}

/// POST /api/internal/ask-user-question — invoked by the lucidos-cli
/// `ask-user-question-hook` subcommand from inside a CC subprocess when CC
/// fires the `AskUserQuestion` PreToolUse hook.
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

    let parser_input = serde_json::json!({ "questions": body.questions });
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input);
    if parsed.is_empty() {
        // CC sent zero questions — return an empty answers map; CC will read
        // the tool result and decide what to do (typically: reissue with a
        // real question). We deliberately don't 400 because the hook
        // protocol doesn't model errors — a non-200 leaves CC stuck. Log
        // because a misbehaving CC could otherwise flood this silently.
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

    let session_id = body.session_id.clone();
    let outer_tool_use_id = body.tool_use_id.clone();
    let total = parsed.len();
    let mut answer_kinds: Vec<serde_json::Value> = Vec::with_capacity(total);
    let mut first_canceled_index: Option<usize> = None;
    for (i, q) in parsed.into_iter().enumerate() {
        let sub_id = synth_question_id(&body.tool_use_id, i);

        // Register FIRST. If the lookup ran first and the user answered
        // between lookup and register, the broadcast send would find no
        // subscriber and we'd block forever on `recv`. Registering first
        // guarantees the wake either lands in the channel buffer (caught by
        // `recv`) or fires before we get here (caught by the lookup below).
        let mut waiter = state.engine.question_wait_registry.register(&sub_id).await;

        // Crash-recovery fast path: user already answered this question on
        // the previous engine instance.
        match lookup_existing_answer(state.engine.pool(), thread_id, &sub_id).await {
            Ok(Some(prior)) => {
                state.engine.question_wait_registry.forget(&sub_id).await;
                let canceled = is_canceled_answer(&prior);
                answer_kinds.push(prior);
                if canceled {
                    first_canceled_index = Some(i);
                    break;
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                state.engine.question_wait_registry.forget(&sub_id).await;
                crate::log!(
                    "[AskUserQuestion] DB lookup failed for {thread_id}/{sub_id}: {e}"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB lookup for prior UserQuestionAnswered failed",
                )
                    .into_response();
            }
        }

        // Skip emit if a UserQuestionAsked already exists for this sub_id —
        // we are re-entering after engine restart and the card is still open.
        let already_asked =
            match user_question_already_asked(state.engine.pool(), thread_id, &sub_id).await {
                Ok(b) => b,
                Err(e) => {
                    state.engine.question_wait_registry.forget(&sub_id).await;
                    crate::log!(
                        "[AskUserQuestion] DB lookup failed for {thread_id}/{sub_id}: {e}"
                    );
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DB lookup for prior UserQuestionAsked failed",
                    )
                        .into_response();
                }
            };
        if !already_asked {
            if let Err(e) = state
                .engine
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::UserQuestionAsked {
                        tool_use_id: sub_id.clone(),
                        cc_session_id: session_id.clone(),
                        question: q.question,
                        options: q.options,
                        worktree_path: None,
                        multi_select: q.multi_select,
                    },
                    meta: EventMeta::NONE,
                })
                .await
            {
                state.engine.question_wait_registry.forget(&sub_id).await;
                crate::log!(
                    "[AskUserQuestion] Failed to emit UserQuestionAsked for {thread_id}/{sub_id}: {e}"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to persist UserQuestionAsked",
                )
                    .into_response();
            }
        }

        // Block until UserQuestionAnswered fires (no timeout — same as MCP permission).
        let payload = match waiter.recv().await {
            Ok(p) => p,
            Err(_) => {
                state.engine.question_wait_registry.forget(&sub_id).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "wait registry channel closed",
                )
                    .into_response();
            }
        };
        state.engine.question_wait_registry.forget(&sub_id).await;

        let canceled = is_canceled_answer(&payload.answers);
        answer_kinds.push(payload.answers);
        if canceled {
            // User canceled mid-multi-question. Don't render any more cards;
            // `build_hook_answers` pads the rest with `(canceled)` so CC sees
            // the cancel for every remaining question.
            first_canceled_index = Some(i);
            break;
        }
    }

    // Persist Canceled markers for any sub_ids the loop short-circuited past.
    // Without this, an engine restart between the cancel and CC processing the
    // hook result would re-fire the hook, the per-question crash-recovery
    // lookup would see no answer for the trailing sub_ids, and we'd re-emit
    // UserQuestionAsked for cards the user already implicitly canceled.
    if let Some(canceled_at) = first_canceled_index {
        for j in (canceled_at + 1)..total {
            let remaining_sub = synth_question_id(&outer_tool_use_id, j);
            state
                .engine
                .event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::UserQuestionAnswered {
                            tool_use_id: remaining_sub,
                            answer: crate::engine::thread_events::AnswerKind::Canceled,
                        },
                        meta: EventMeta {
                            channel: Some(EventChannel::CodingAgent),
                            ..EventMeta::NONE
                        },
                    },
                    "[AskUserQuestion] cancel padding for remaining sub_id",
                )
                .await;
        }
    }

    let answers = build_hook_answers(&answer_kinds, &body.questions);
    Json(AskUserQuestionResponse {
        questions: body.questions,
        answers,
    })
    .into_response()
}

/// True when an `AnswerKind` JSON object has `kind == "Canceled"`. Used to
/// short-circuit the multi-question loop: once any question is canceled, we
/// stop emitting further cards and let `build_hook_answers` pad the rest.
fn is_canceled_answer(answer_kind: &serde_json::Value) -> bool {
    answer_kind.get("kind").and_then(|k| k.as_str()) == Some("Canceled")
}

/// Look up the most recent `UserQuestionAnswered` for `tool_use_id` and return
/// the inner `answer` (an `AnswerKind` JSON object). `Ok(None)` if no answer
/// is persisted yet. DB errors propagate so the caller can surface them
/// instead of silently treating a transient failure as "no prior answer".
async fn lookup_existing_answer(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload->'answer' FROM events
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered'
           AND payload->>'tool_use_id' = $2
         LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.filter(|v| !v.is_null()))
}

/// True iff there's already a `UserQuestionAsked` for this `tool_use_id` (used
/// to suppress duplicate emits on hook re-fire after an engine restart). DB
/// errors propagate so a transient failure isn't read as "no prior question".
async fn user_question_already_asked(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM events
           WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'
             AND payload->>'tool_use_id' = $2
         )",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_one(pool)
    .await
}

/// Look up the label for a synthesized option_id (`opt-N`) from a specific
/// question's options array. `question_index` is the 0-based position of the
/// question in CC's outer `questions` array — option ids restart at `opt-0`
/// per question (see `parse_one_question` in `agent_session::parsing`), so
/// we need to know which question's options to consult. Falls back to the
/// bare `opt_id` on miss so the model sees a recognizable error rather than
/// a silent drop.
fn lookup_option_label(
    opt_id: &str,
    cc_questions: &serde_json::Value,
    question_index: usize,
) -> String {
    opt_id
        .strip_prefix("opt-")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|idx| {
            cc_questions
                .as_array()
                .and_then(|arr| arr.get(question_index))
                .and_then(|q| q.get("options"))
                .and_then(|opts| opts.as_array())
                .and_then(|opts| opts.get(idx))
                .and_then(|opt| opt.get("label"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| opt_id.to_string())
}

/// Convert one `AnswerKind` JSON value (`{"kind": "Selected", ...}` etc.)
/// into the hook's per-question answer string. Looks up labels from the
/// `question_index`-th question's options. Falls back to the option_id /
/// text on lookup miss. `Canceled` produces a `(canceled)` marker rather
/// than an empty string — an empty answer causes CC's model to read the
/// question as unanswered and re-invoke the tool in a loop.
///
/// `MultiSelected` joins the resolved labels with `", "` — CC's `answers`
/// schema is `additionalProperties: string`, so the joined form is the right
/// shape for the model to read back as multiple chosen options. When the
/// payload also carries a non-empty `text` (freetext typed in the prompt
/// textarea while the question was on screen), it joins on the same `", "`
/// after the labels — CC sees one comma-joined answer.
fn answer_kind_to_hook_value(
    answer_kind: &serde_json::Value,
    cc_questions: &serde_json::Value,
    question_index: usize,
) -> serde_json::Value {
    let kind = answer_kind
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("");
    match kind {
        "Selected" => {
            let opt_id = answer_kind
                .get("option_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            serde_json::Value::String(lookup_option_label(opt_id, cc_questions, question_index))
        }
        "MultiSelected" => {
            let mut parts: Vec<String> = answer_kind
                .get("option_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|id| lookup_option_label(id, cc_questions, question_index))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(text) = answer_kind.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
            serde_json::Value::String(parts.join(", "))
        }
        "FreeText" => answer_kind
            .get("text")
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new())),
        "Canceled" => serde_json::Value::String("(canceled)".to_string()),
        _ => serde_json::Value::String(format!("(unknown answer kind: {})", kind)),
    }
}

/// Build the combined `{question_text: answer_label}` map CC expects in the
/// hook tool result. Walks every question CC originally sent, pairing it
/// with the corresponding collected answer (by index). When a question has
/// no collected answer yet — the loop short-circuited on cancellation — we
/// emit `(canceled)` for that question. Same reason as the per-question
/// Canceled branch above: an empty/missing entry would make CC's model read
/// the question as unanswered and retry the whole tool call.
///
/// CC's hook output schema is a JSON object keyed by question text, which
/// implicitly assumes unique texts across the call. A duplicate text would
/// silently overwrite the prior answer, dropping it on the floor — same UX
/// failure mode as the bug this whole feature fixes (CC sees fewer answers
/// than questions and "re-asks"). We disambiguate duplicates with a
/// `" (#i)"` suffix where `i` is the 1-based question index. CC won't
/// recognize the suffixed key against its outgoing tool input, but every
/// answer is at least carried — and the log makes it visible.
fn build_hook_answers(
    answer_kinds: &[serde_json::Value],
    cc_questions: &serde_json::Value,
) -> serde_json::Value {
    let Some(arr) = cc_questions.as_array() else {
        return serde_json::Value::Object(serde_json::Map::new());
    };
    let mut map = serde_json::Map::with_capacity(arr.len());
    for (i, q) in arr.iter().enumerate() {
        let raw_text = q
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown question)")
            .to_string();
        let key = if map.contains_key(&raw_text) {
            crate::log!(
                "[AskUserQuestion] duplicate question text in CC tool input — disambiguating with suffix: {raw_text:?}"
            );
            format!("{raw_text} (#{})", i + 1)
        } else {
            raw_text
        };
        let value = match answer_kinds.get(i) {
            Some(ans) => answer_kind_to_hook_value(ans, cc_questions, i),
            None => serde_json::Value::String("(canceled)".to_string()),
        };
        map.insert(key, value);
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod ask_user_question_tests {
    use super::*;

    fn questions() -> serde_json::Value {
        serde_json::json!([{
            "question": "Fav color?",
            "options": [
                {"label": "Red", "description": ""},
                {"label": "Blue", "description": ""}
            ]
        }])
    }

    fn three_questions() -> serde_json::Value {
        serde_json::json!([
            {
                "question": "Fav color?",
                "options": [{"label": "Red"}, {"label": "Blue"}],
            },
            {
                "question": "Fav animal?",
                "options": [{"label": "Cat"}, {"label": "Dog"}],
            },
            {
                "question": "Pick all toppings",
                "multiSelect": true,
                "options": [{"label": "Cheese"}, {"label": "Olives"}],
            },
        ])
    }

    #[test]
    fn selected_answer_resolves_to_label() {
        let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-1"});
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Blue"}));
    }

    #[test]
    fn free_text_passes_through() {
        let answer = serde_json::json!({"kind": "FreeText", "text": "purple"});
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "purple"}));
    }

    #[test]
    fn canceled_returns_explicit_marker_not_empty_object() {
        // Empty `{}` would be read as "unanswered" by CC's model, causing an
        // infinite re-invocation loop. The marker terminates the call.
        let answer = serde_json::json!({"kind": "Canceled"});
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "(canceled)"}));
    }

    #[test]
    fn missing_label_falls_back_to_option_id() {
        let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-9"});
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "opt-9"}));
    }

    #[test]
    fn multi_selected_joins_labels_with_comma_space() {
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-0", "opt-1"]
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Red, Blue"}));
    }

    #[test]
    fn multi_selected_unknown_id_falls_back_to_id() {
        // Mirrors single-Selected fallback — keeps the error visible to the
        // model rather than silently dropping it.
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-0", "opt-9"]
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Red, opt-9"}));
    }

    #[test]
    fn multi_selected_single_id_yields_one_label() {
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-1"]
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Blue"}));
    }

    #[test]
    fn multi_selected_with_text_appends_after_labels() {
        // Prompt-row Submit folded the textarea contents into the answer.
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-0", "opt-1"],
            "text": "and also purple",
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Red, Blue, and also purple"}));
    }

    #[test]
    fn multi_selected_with_only_text_yields_just_text() {
        // No toggles — answer collapses to the freetext alone.
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": [],
            "text": "freeform answer",
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "freeform answer"}));
    }

    #[test]
    fn multi_selected_with_empty_text_omits_trailing_separator() {
        // Empty `text` must NOT add a trailing ", " — the separator only
        // appears between non-empty parts.
        let answer = serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-0"],
            "text": "",
        });
        let out = build_hook_answers(&[answer], &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Red"}));
    }

    // ---- multi-question coverage (build_hook_answers + per-question label lookup) ----

    #[test]
    fn build_hook_answers_pairs_each_question_with_its_own_options() {
        // Per-question option ids restart at opt-0; the lookup must consult
        // the matching question's options array, not always questions[0].
        let answers = vec![
            serde_json::json!({"kind": "Selected", "option_id": "opt-1"}),
            serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
            serde_json::json!({
                "kind": "MultiSelected",
                "option_ids": ["opt-0", "opt-1"],
            }),
        ];
        let out = build_hook_answers(&answers, &three_questions());
        assert_eq!(
            out,
            serde_json::json!({
                "Fav color?": "Blue",
                "Fav animal?": "Cat",
                "Pick all toppings": "Cheese, Olives",
            })
        );
    }

    #[test]
    fn build_hook_answers_pads_missing_answers_with_canceled_marker() {
        // Loop short-circuited on cancel after Q1 answered. Q2 + Q3 must
        // surface as `(canceled)` — never empty/missing keys, which CC reads
        // as "unanswered" and retries the whole tool call.
        let answers = vec![serde_json::json!({"kind": "Selected", "option_id": "opt-0"})];
        let out = build_hook_answers(&answers, &three_questions());
        assert_eq!(
            out,
            serde_json::json!({
                "Fav color?": "Red",
                "Fav animal?": "(canceled)",
                "Pick all toppings": "(canceled)",
            })
        );
    }

    #[test]
    fn build_hook_answers_handles_zero_questions() {
        let out = build_hook_answers(&[], &serde_json::json!([]));
        assert_eq!(out, serde_json::json!({}));
    }

    #[test]
    fn lookup_option_label_uses_question_index_not_first() {
        // Direct test of the helper to guard against a regression to the
        // "always read questions[0]" bug — labels in Q2 should resolve via
        // questions[1].options.
        let questions = three_questions();
        assert_eq!(
            lookup_option_label("opt-1", &questions, 1),
            "Dog",
            "must look up Q2's options[1], not Q1's"
        );
        assert_eq!(
            lookup_option_label("opt-0", &questions, 2),
            "Cheese",
            "must look up Q3's options[0]"
        );
    }

    #[test]
    fn synth_question_id_format_is_outer_hash_q_index() {
        // The hash separator + `q` prefix must stay stable — the wait
        // registry, the per-question UserQuestionAsked emit, and the
        // crash-recovery answer lookup all key on this exact string.
        assert_eq!(synth_question_id("toolu_xyz", 0), "toolu_xyz#q0");
        assert_eq!(synth_question_id("toolu_xyz", 12), "toolu_xyz#q12");
    }

    #[test]
    fn is_canceled_answer_only_matches_explicit_canceled_kind() {
        assert!(is_canceled_answer(&serde_json::json!({"kind": "Canceled"})));
        assert!(!is_canceled_answer(
            &serde_json::json!({"kind": "Selected", "option_id": "opt-0"})
        ));
        assert!(!is_canceled_answer(&serde_json::json!({})));
        assert!(!is_canceled_answer(&serde_json::Value::Null));
    }

    #[test]
    fn build_hook_answers_disambiguates_duplicate_question_texts() {
        // CC's hook output is keyed by question text. If CC sends two
        // questions with identical text, a naive `Map::insert` would
        // overwrite the first answer — same UX failure as the bug this
        // feature fixes. Disambiguate with `" (#i)"` so every answer is
        // carried back to CC.
        let dupe_questions = serde_json::json!([
            {"question": "Pick one", "options": [{"label": "A"}]},
            {"question": "Pick one", "options": [{"label": "B"}]},
        ]);
        let answers = vec![
            serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
            serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
        ];
        let out = build_hook_answers(&answers, &dupe_questions);
        let obj = out.as_object().expect("object");
        assert_eq!(obj.len(), 2, "both answers must survive — got {out}");
        assert_eq!(obj.get("Pick one"), Some(&serde_json::json!("A")));
        assert_eq!(obj.get("Pick one (#2)"), Some(&serde_json::json!("B")));
    }
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

    #[test]
    fn build_summary_uses_file_path() {
        let s = build_summary(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md", "old_string": "x" }),
        );
        assert_eq!(s, "Edit /tmp/foo.md");
    }

    #[test]
    fn build_summary_falls_back_to_command() {
        let s = build_summary("Bash", &serde_json::json!({ "command": "ls -la" }));
        assert_eq!(s, "Bash ls -la");
    }

    #[test]
    fn build_summary_returns_tool_name_when_no_arg_field() {
        let s = build_summary("WeirdTool", &serde_json::json!({ "foo": 1 }));
        assert_eq!(s, "WeirdTool");
    }

    #[test]
    fn build_summary_uses_skill_for_skill_tool() {
        let s = build_summary("Skill", &serde_json::json!({ "skill": "update-config" }));
        assert_eq!(s, "skill update-config");
    }

    #[test]
    fn build_summary_uses_url_for_webfetch() {
        let s = build_summary(
            "WebFetch",
            &serde_json::json!({ "url": "https://example.com", "prompt": "x" }),
        );
        assert_eq!(s, "WebFetch https://example.com");
    }

    fn register(state: &mut CcPermissionState, key: DedupKey) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
        let tool_name = key.1.clone();
        register_or_attach(
            state,
            key,
            Uuid::nil(),
            tool_name,
            serde_json::json!({}),
        )
    }

    #[test]
    fn register_or_attach_creates_canonical_entry_first_time() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (request_id, _rx, is_canonical) = register(&mut state, key.clone());
        assert!(is_canonical, "first request must be canonical");
        assert!(state.by_dedup_key.contains_key(&key));
        assert!(state.by_request_id.contains_key(&request_id));
    }

    #[test]
    fn register_or_attach_returns_existing_request_id_for_duplicate() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (first_id, _rx1, first_canonical) = register(&mut state, key.clone());
        let (second_id, _rx2, second_canonical) = register(&mut state, key.clone());
        assert!(first_canonical);
        assert!(!second_canonical, "duplicate must not be canonical");
        assert_eq!(
            first_id, second_id,
            "duplicate must reuse the canonical request_id"
        );
    }

    #[test]
    fn register_or_attach_stores_tool_name_and_input_on_canonical_entry() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Skill".into(), "{\"skill\":\"x:y\"}".into());
        let (request_id, _rx, _) = register_or_attach(
            &mut state,
            key,
            Uuid::nil(),
            "Skill".into(),
            serde_json::json!({"skill": "x:y"}),
        );
        let entry = state.take(&request_id).unwrap();
        assert_eq!(entry.tool_name, "Skill");
        assert_eq!(entry.input, serde_json::json!({"skill": "x:y"}));
    }

    #[tokio::test]
    async fn duplicate_subscribers_both_receive_the_answer() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (id, mut rx1, _) = register(&mut state, key.clone());
        let (_, mut rx2, _) = register(&mut state, key.clone());

        // Resolve via the same path the consent endpoint uses.
        let entry = state.take(&id).expect("entry must be present");
        let _ = entry.tx.send(true);

        assert!(rx1.recv().await.unwrap());
        assert!(rx2.recv().await.unwrap());
    }

    #[test]
    fn parse_show_output_extracts_subject_and_files() {
        // Subject on line 1, blank line, then NUL-separated file paths.
        let raw = "feat: add x\n\nsrc/main.rs\0src/lib.rs\0";
        let (subject, files) = parse_show_output(raw);
        assert_eq!(subject, "feat: add x");
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn parse_show_output_handles_empty_files() {
        let raw = "fix: empty commit\n\n";
        let (subject, files) = parse_show_output(raw);
        assert_eq!(subject, "fix: empty commit");
        assert!(files.is_empty());
    }

    /// Phase 4.2 contract: every commit produces its own `ChangeProposed`
    /// event keyed by `commit_sha`. This test runs three real commits in a
    /// real worktree, calls the helper that the HTTP endpoint dispatches
    /// to, and asserts three distinct ChangeProposed events flow through
    /// the EventBus with the matching SHAs and subjects.
    #[tokio::test]
    async fn each_commit_emits_change_proposed_event() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::git_ops::git_cmd;
        use crate::engine::thread_events::ThreadEvent;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        // Set up a real repo with a main branch, then a worktree on a CC
        // branch. Three commits land on the CC branch.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_cmd(&["init", "-b", "main"], &repo).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &repo).await.unwrap();
        std::fs::write(repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &repo).await.unwrap();

        let wt = tmp.path().join("wt");
        git_cmd(
            &[
                "worktree",
                "add",
                wt.to_str().unwrap(),
                "-b",
                "claude-code/per-commit-test",
            ],
            &repo,
        )
        .await
        .unwrap();
        // Worktrees inherit user config in CI; ensure committer ident.
        git_cmd(&["config", "user.email", "test@example.com"], &wt)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &wt).await.unwrap();

        let mut shas: Vec<String> = Vec::new();
        let subjects = ["first commit", "second commit", "third commit"];
        for (i, subject) in subjects.iter().enumerate() {
            let f = format!("file_{}.txt", i);
            std::fs::write(wt.join(&f), format!("contents {}", i)).unwrap();
            git_cmd(&["add", &f], &wt).await.unwrap();
            git_cmd(&["commit", "-m", subject], &wt).await.unwrap();
            let sha_output = git_cmd(&["rev-parse", "HEAD"], &wt).await.unwrap();
            assert!(sha_output.status.success());
            let sha = String::from_utf8_lossy(&sha_output.stdout).trim().to_string();
            shas.push(sha);
        }

        let thread_id = Uuid::new_v4();

        // Seed a CC SessionStarted so the EventBus lifecycle classifier
        // recognizes the thread as a CC thread (ChangeProposed is rejected
        // on Chat-classified threads).
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                session_id: "test-sid".into(),
                branch: "claude-code/per-commit-test".into(),
                repo_id: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        // Subscribe BEFORE emitting so we don't miss events.
        let mut sub = bus.subscribe();

        for sha in &shas {
            emit_change_proposed_for_commit(
                &bus,
                thread_id,
                &wt,
                sha,
                "claude-code/per-commit-test",
            )
            .await
            .unwrap();
        }

        // Drain the broadcast: collect ChangeProposed events for our thread.
        let mut received: Vec<(String, Option<String>)> = Vec::new();
        // Allow a small drain budget — emits are synchronous-ish but the
        // broadcast reader may need to schedule.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while received.len() < shas.len() && tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(500), sub.recv()).await {
                Ok(Ok(emitted)) => {
                    if let BusEvent::Thread {
                        thread_id: tid,
                        event,
                        ..
                    } = emitted.typed
                    {
                        if tid != thread_id {
                            continue;
                        }
                        if let ThreadEvent::ChangeProposed {
                            commit_sha,
                            description,
                            ..
                        } = event
                        {
                            if let Some(sha) = commit_sha {
                                received.push((sha, description));
                            }
                        }
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }

        assert_eq!(
            received.len(),
            shas.len(),
            "expected {} per-commit ChangeProposed events, got {} ({:?})",
            shas.len(),
            received.len(),
            received,
        );

        // Each emitted SHA must appear once and only once.
        for (i, sha) in shas.iter().enumerate() {
            let matched: Vec<&(String, Option<String>)> =
                received.iter().filter(|(s, _)| s == sha).collect();
            assert_eq!(
                matched.len(),
                1,
                "expected exactly one ChangeProposed for sha {}, got {:?}",
                sha,
                matched,
            );
            // Subject (description) on the event must match the commit subject.
            assert_eq!(
                matched[0].1.as_deref(),
                Some(subjects[i]),
                "ChangeProposed for sha {} should carry subject {:?}",
                sha,
                subjects[i],
            );
        }

        teardown_test_db(&db_name).await;
    }
}
