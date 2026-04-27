use super::*;
use crate::engine::cc_permission::{CcPermissionEntry, CcPermissionState, DedupKey, DENIAL_REASON};
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

    let canonical_input =
        serde_json::to_string(&body.input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, body.tool_name.clone(), canonical_input);
    let summary = build_summary(&body.tool_name, &body.input);

    let (request_id, mut rx, is_canonical) = {
        let mut pending = state.engine.pending_cc_permission.lock().unwrap();
        register_or_attach(&mut pending, dedup_key, thread_id)
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
            tx,
        },
    );
    state.by_request_id.insert(request_id.clone(), dedup_key);
    (request_id, rx, true)
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
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)",
    )
    .bind(thread_id)
    .fetch_one(state.engine.pool())
    .await
    .unwrap_or(false);
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
pub(crate) enum CommitEmitError {
    GitShowFailed(String),
    GitError(String),
}

/// Core of the `commit-made` endpoint extracted for testability.
///
/// Reads the commit's subject + changed-file list via `git show`, then
/// emits a `ChangeProposed` event keyed by `commit_sha`. The HTTP handler
/// is a thin wrapper that resolves thread/worktree, then calls this.
pub(crate) async fn emit_change_proposed_for_commit(
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

/// POST /api/internal/ask-user-question — invoked by the lucidos-cli
/// `ask-user-question-hook` subcommand from inside a CC subprocess when CC
/// fires the `AskUserQuestion` PreToolUse hook. Long-polls until the user
/// answers (or returns immediately if a prior `UserQuestionAnswered` already
/// exists for this `tool_use_id`, for crash recovery).
pub(super) async fn ask_user_question(
    State(state): State<AppState>,
    Json(body): Json<AskUserQuestionRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    // Register the waiter FIRST. If the lookup ran first and the user
    // answered between lookup and register, the broadcast send would find no
    // subscriber and the hook would block forever on `recv`. Registering
    // first guarantees the wake either lands in the channel buffer (caught by
    // `recv`) or fires before we get here (caught by the lookup below).
    let mut waiter = state
        .engine
        .question_wait_registry
        .register(&body.tool_use_id)
        .await;

    // Crash-recovery fast path: user already answered this tool_use_id while
    // the previous engine instance was alive.
    let prior_answer =
        match lookup_existing_answer(state.engine.pool(), thread_id, &body.tool_use_id).await {
            Ok(opt) => opt,
            Err(e) => {
                state
                    .engine
                    .question_wait_registry
                    .forget(&body.tool_use_id)
                    .await;
                crate::log!(
                    "[AskUserQuestion] DB lookup failed for {}/{}: {}",
                    thread_id,
                    body.tool_use_id,
                    e
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB lookup for prior UserQuestionAnswered failed",
                )
                    .into_response();
            }
        };
    if let Some(answer_kind_json) = prior_answer {
        state
            .engine
            .question_wait_registry
            .forget(&body.tool_use_id)
            .await;
        let answers = answer_kind_to_hook_answers(&answer_kind_json, &body.questions);
        return Json(AskUserQuestionResponse {
            questions: body.questions,
            answers,
        })
        .into_response();
    }

    // Translate hook's questions[0] into engine's singular (question, options)
    // shape using the existing parser. Parser input shape is { questions: [...] }.
    let parser_input = serde_json::json!({ "questions": body.questions });
    let (question, options) =
        crate::engine::agent_session::parse_ask_user_question_input(&parser_input);

    // Skip emit if a UserQuestionAsked already exists for this tool_use_id —
    // we are re-entering after engine restart and the UI still has the card open.
    let already_asked =
        match user_question_already_asked(state.engine.pool(), thread_id, &body.tool_use_id).await
        {
            Ok(b) => b,
            Err(e) => {
                state
                    .engine
                    .question_wait_registry
                    .forget(&body.tool_use_id)
                    .await;
                crate::log!(
                    "[AskUserQuestion] DB lookup failed for {}/{}: {}",
                    thread_id,
                    body.tool_use_id,
                    e
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
                    tool_use_id: body.tool_use_id.clone(),
                    cc_session_id: body.session_id,
                    question,
                    options,
                    worktree_path: None,
                },
                meta: EventMeta::NONE,
            })
            .await
        {
            state
                .engine
                .question_wait_registry
                .forget(&body.tool_use_id)
                .await;
            crate::log!(
                "[AskUserQuestion] Failed to emit UserQuestionAsked for {}/{}: {}",
                thread_id,
                body.tool_use_id,
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to persist UserQuestionAsked",
            )
                .into_response();
        }
    }

    // 4. Block until UserQuestionAnswered fires (no timeout — same as MCP permission).
    let payload = match waiter.recv().await {
        Ok(p) => p,
        Err(_) => {
            state
                .engine
                .question_wait_registry
                .forget(&body.tool_use_id)
                .await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "wait registry channel closed",
            )
                .into_response();
        }
    };
    state
        .engine
        .question_wait_registry
        .forget(&body.tool_use_id)
        .await;

    let answers = answer_kind_to_hook_answers(&payload.answers, &body.questions);
    Json(AskUserQuestionResponse {
        questions: body.questions,
        answers,
    })
    .into_response()
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

/// Convert an `AnswerKind` JSON value (`{"kind": "Selected", "option_id": "opt-0"}`
/// etc.) into CC's expected hook output shape `{question_text: chosen_label}`.
/// Looks up the label from the original CC `questions` JSON. Falls back to the
/// option_id / text on lookup miss. `Canceled` produces a `(canceled)` marker
/// answer rather than an empty object — empty `{}` causes CC's model to read
/// the question as unanswered and re-invoke the tool in a loop.
fn answer_kind_to_hook_answers(
    answer_kind: &serde_json::Value,
    cc_questions: &serde_json::Value,
) -> serde_json::Value {
    let question_text = cc_questions
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|q| q.get("question"))
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown question)")
        .to_string();

    let kind = answer_kind
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("");
    let value = match kind {
        "Selected" => {
            let opt_id = answer_kind
                .get("option_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let label = opt_id
                .strip_prefix("opt-")
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|idx| {
                    cc_questions
                        .as_array()
                        .and_then(|arr| arr.first())
                        .and_then(|q| q.get("options"))
                        .and_then(|opts| opts.as_array())
                        .and_then(|opts| opts.get(idx))
                        .and_then(|opt| opt.get("label"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| opt_id.to_string());
            serde_json::Value::String(label)
        }
        "FreeText" => answer_kind
            .get("text")
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new())),
        "Canceled" => serde_json::Value::String("(canceled)".to_string()),
        _ => serde_json::Value::String(format!("(unknown answer kind: {})", kind)),
    };

    serde_json::json!({ question_text: value })
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

    #[test]
    fn selected_answer_resolves_to_label() {
        let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-1"});
        let out = answer_kind_to_hook_answers(&answer, &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "Blue"}));
    }

    #[test]
    fn free_text_passes_through() {
        let answer = serde_json::json!({"kind": "FreeText", "text": "purple"});
        let out = answer_kind_to_hook_answers(&answer, &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "purple"}));
    }

    #[test]
    fn canceled_returns_explicit_marker_not_empty_object() {
        // Empty `{}` would be read as "unanswered" by CC's model, causing an
        // infinite re-invocation loop. The marker terminates the call.
        let answer = serde_json::json!({"kind": "Canceled"});
        let out = answer_kind_to_hook_answers(&answer, &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "(canceled)"}));
    }

    #[test]
    fn missing_label_falls_back_to_option_id() {
        let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-9"});
        let out = answer_kind_to_hook_answers(&answer, &questions());
        assert_eq!(out, serde_json::json!({"Fav color?": "opt-9"}));
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

    #[test]
    fn register_or_attach_creates_canonical_entry_first_time() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (request_id, _rx, is_canonical) = register_or_attach(&mut state, key.clone(), Uuid::nil());
        assert!(is_canonical, "first request must be canonical");
        assert!(state.by_dedup_key.contains_key(&key));
        assert!(state.by_request_id.contains_key(&request_id));
    }

    #[test]
    fn register_or_attach_returns_existing_request_id_for_duplicate() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (first_id, _rx1, first_canonical) =
            register_or_attach(&mut state, key.clone(), Uuid::nil());
        let (second_id, _rx2, second_canonical) =
            register_or_attach(&mut state, key.clone(), Uuid::nil());
        assert!(first_canonical);
        assert!(!second_canonical, "duplicate must not be canonical");
        assert_eq!(
            first_id, second_id,
            "duplicate must reuse the canonical request_id"
        );
    }

    #[tokio::test]
    async fn duplicate_subscribers_both_receive_the_answer() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (id, mut rx1, _) = register_or_attach(&mut state, key.clone(), Uuid::nil());
        let (_, mut rx2, _) = register_or_attach(&mut state, key.clone(), Uuid::nil());

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
