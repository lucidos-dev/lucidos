use super::{CcCommandsResult, LucidosEngine};
use crate::engine::thread_events::{ActorMode, EngineReason, MessageOrigin};
use crate::engine::types::CcCommandsInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// Look up a repo's cached CC commands by its on-disk path.
///
/// Returns the cached entry for `repo_path`, or empty defaults on miss.
/// **Never falls back to another repo's entry** — surfacing skills from a
/// repo the user did not select would mislead the compose-view menu, which
/// is the bug this helper exists to prevent.
pub(crate) fn lookup_repo_commands_in_cache(
    cache: &HashMap<String, CcCommandsInfo>,
    repo_path: &str,
) -> CcCommandsInfo {
    cache.get(repo_path).cloned().unwrap_or_default()
}

// Empty by default: a fresh install grants nothing implicitly. Users build
// their allowlist via the per-prompt "Always allow" buttons (which append to
// `~/.lucidos/cc-allowed-tools`) or by editing the file directly via the
// settings UI. Editing this list only affects fresh installs — existing users
// keep whatever they already wrote to the file.
const DEFAULT_CC_ALLOWED_TOOLS: &[&str] = &[];

// Tools whose bare entry in `--allowedTools` cannot be respected by CC.
// Two reasons a tool ends up here:
//   * Edit / Write / NotebookEdit — `--permission-mode acceptEdits` always
//     sends them through `--permission-prompt-tool` for the paths CC keeps
//     protected (`.claude/` and `.git/`, which never auto-approve in any
//     mode), and the rest of the worktree's in-cwd writes are auto-approved
//     before the engine ever sees them. A bare `Edit` line in
//     `cc-allowed-tools` does nothing useful in either case.
//   * ExitPlanMode — CC always routes plan-mode exit through the permission
//     prompt regardless of `--allowedTools`, because the plan must be
//     reviewed by the user before the assistant continues. A bare
//     `ExitPlanMode` line never suppresses the card.
// The "Always allow" broad button is hidden for these tools (see
// `BROAD_ALLOW_INEFFECTIVE` in `PermissionCard.tsx`); users wanting in-thread
// persistence should use the session-allow button instead, which the engine
// intercepts before CC's gate.
const BROAD_ALLOW_INEFFECTIVE: &[&str] = &["Edit", "ExitPlanMode", "NotebookEdit", "Write"];

// Tools whose `AllowScope::Session` pattern is per-file, derived from the
// input's `file_path` / `notebook_path` field rather than the tool name.
// Overlaps with but is not identical to `BROAD_ALLOW_INEFFECTIVE`: this set
// is "tools where remembering one path doesn't imply remembering all paths,"
// while `BROAD_ALLOW_INEFFECTIVE` is "tools whose bare allowlist entry is a
// lie." Edit/Write/NotebookEdit are in both; ExitPlanMode is only in the
// latter (no per-path identifier — its session pattern is the bare tool
// name). Mirrors the TS-side `SESSION_PATH_TOOLS` constant in
// `PermissionCard.tsx`.
const SESSION_PATH_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit"];

const CC_ALLOWED_TOOLS_FILE: &str = "cc-allowed-tools";
const CC_ALLOWED_TOOLS_HEADER: &str =
    "# One pattern per line. Lines starting with '#' are ignored.\n";

/// Where a granted "Always allow" click is remembered.
///
///   * `Narrow` / `Broad` — persisted to `~/.lucidos/cc-allowed-tools` and
///     handed to CC via `--allowedTools` on every spawn. Survives engine
///     restart, but only takes effect for tools/paths CC actually respects.
///   * `Session` — kept in memory on `CcPermissionState::session_allows`,
///     scoped to one thread. Lost on engine restart. Works for *every* tool
///     and *every* path (including CC's own protected paths like `.claude/`
///     and `.git/`), because the engine intercepts before the prompt fires.
///
/// Wire form: `"narrow"` / `"broad"` / `"session"` (snake_case enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowScope {
    Narrow,
    Broad,
    Session,
}

/// Derive the pattern to record when the user grants an "Always allow"-style
/// click. Interpretation depends on `scope`:
///
///   * `Broad` / `Narrow` → appended verbatim to `cc-allowed-tools` so it
///     reaches CC as `--allowedTools` on the next spawn. `Broad` returns
///     `Some(tool_name)` only for tools whose bare entry actually bypasses
///     CC's prompt routing — for `BROAD_ALLOW_INEFFECTIVE` (Edit / Write /
///     NotebookEdit) it returns `None` because CC ignores those bare entries
///     for protected paths and auto-approves them everywhere else, so
///     persisting would mislead the user. `Narrow` returns `Some` only for
///     tools with a meaningful sub-scope in the input:
///       * `Skill { skill: "plugin:name" }` → `Skill(plugin:*)`
///       * `Bash  { command: "git status" }` → `Bash(git:*)`
///
///     All other tools return `None` for `Narrow` (the UI hides that button).
///
///   * `Session` → stored on `CcPermissionState::session_allows` and matched
///     exact-string against patterns derived from future prompts in the same
///     thread. Always returns `Some(_)` so any prompt can be remembered for
///     the rest of the thread, including CC-protected paths the persisted
///     scopes can't reach:
///       * `Edit | Write` → `Tool(<file_path>)` (per-file)
///       * `NotebookEdit` → `NotebookEdit(<notebook_path>)`
///       * `Bash` → `Bash(<first-token>:*)` (same as narrow)
///       * `Skill` → `Skill(<plugin>:*)` (same as narrow)
///       * everything else → bare `tool_name`
pub(crate) fn derive_allow_pattern(
    tool_name: &str,
    input: &serde_json::Value,
    scope: AllowScope,
) -> Option<String> {
    match scope {
        AllowScope::Broad => {
            if BROAD_ALLOW_INEFFECTIVE.contains(&tool_name) {
                return None;
            }
            Some(tool_name.to_string())
        }
        AllowScope::Narrow => narrow_subscope(tool_name, input),
        AllowScope::Session => {
            if SESSION_PATH_TOOLS.contains(&tool_name) {
                let path_key = if tool_name == "NotebookEdit" {
                    "notebook_path"
                } else {
                    "file_path"
                };
                let path = input.get(path_key).and_then(|v| v.as_str())?;
                if path.is_empty() {
                    return None;
                }
                return Some(format!("{}({})", tool_name, path));
            }
            if let Some(narrow) = narrow_subscope(tool_name, input) {
                return Some(narrow);
            }
            // Bare tool name — session scope is engine-side, so the
            // `BROAD_ALLOW_INEFFECTIVE` constraint that applies to persisted
            // patterns doesn't apply: the engine's pre-prompt check fires
            // before CC's gate ever runs, regardless of CC's behavior.
            Some(tool_name.to_string())
        }
    }
}

/// Narrow `--allowedTools`-style sub-scope for tools whose input carries a
/// meaningful identifier. Returns `None` for tools without one — Narrow
/// callers treat that as "no narrow button"; Session callers fall back to
/// the bare tool name.
fn narrow_subscope(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Skill" => {
            let skill = input.get("skill").and_then(|v| v.as_str())?;
            let plugin = skill.split_once(':').map(|(p, _)| p).unwrap_or(skill);
            if plugin.is_empty() {
                return None;
            }
            Some(format!("Skill({}:*)", plugin))
        }
        "Bash" => {
            let command = input.get("command").and_then(|v| v.as_str())?;
            let first = command.split_whitespace().next()?;
            if first.is_empty() {
                return None;
            }
            Some(format!("Bash({}:*)", first))
        }
        _ => None,
    }
}

/// Append `pattern` to `<user_dir>/cc-allowed-tools` if not already present.
/// Creates the file (with the header comment) if it doesn't exist. Atomic
/// write via tmp + rename. No-op when `user_dir` is `None`.
pub(crate) fn append_allowed_tool_pattern(
    user_dir: Option<&Path>,
    pattern: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(dir) = user_dir else {
        return Ok(());
    };
    let path = dir.join(CC_ALLOWED_TOOLS_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CC_ALLOWED_TOOLS_HEADER.to_string(),
        Err(e) => return Err(e.into()),
    };
    if existing
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#') && l == pattern)
    {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(pattern);
    next.push('\n');
    write_allowed_tools_file(dir, &next)
}

/// Read the raw contents of `<user_dir>/cc-allowed-tools`. Returns the seeded
/// header for a missing file (mirrors what `cc_allowed_tools` would produce)
/// so the settings UI shows something coherent even before the first prompt.
pub(crate) fn read_allowed_tools_file(
    user_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = user_dir.join(CC_ALLOWED_TOOLS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CC_ALLOWED_TOOLS_HEADER.to_string()),
        Err(e) => Err(e.into()),
    }
}

/// Atomically write the raw contents of `<user_dir>/cc-allowed-tools`.
pub(crate) fn write_allowed_tools_file(
    user_dir: &Path,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(user_dir)?;
    let path = user_dir.join(CC_ALLOWED_TOOLS_FILE);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Resolve the comma-separated tool allowlist for `claude --allowedTools`.
///
/// Reads `<user_dir>/cc-allowed-tools` if present (one entry per line, blank
/// lines and `#` comments ignored). On first call, seeds the file with the
/// header comment so the user has something to discover and edit. Falls back
/// to the empty default if `user_dir` is `None` or any IO fails.
pub(crate) fn cc_allowed_tools(user_dir: Option<&Path>) -> String {
    let default = || DEFAULT_CC_ALLOWED_TOOLS.join(",");
    let Some(dir) = user_dir else {
        return default();
    };
    let path = dir.join(CC_ALLOWED_TOOLS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(contents) => contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(e) = std::fs::create_dir_all(dir)
                .and_then(|_| std::fs::write(&path, CC_ALLOWED_TOOLS_HEADER))
            {
                log!("[ClaudeCode] Failed to seed {}: {}", path.display(), e);
            }
            default()
        }
        Err(e) => {
            log!(
                "[ClaudeCode] Failed to read {}: {} — using compiled default",
                path.display(),
                e
            );
            default()
        }
    }
}

pub(crate) const AUTO_HARDEN_MESSAGE: &str = "Run /harden now.";

/// A canceled CC subprocess returns `Ok(_)`, so the inner future succeeding
/// is not enough to prove `/harden` ran. The marker is the only signal that
/// survives a kill — it's only written by the skill's final phase.
pub(crate) fn hardening_succeeded(
    cancel_token_cancelled: bool,
    marker_present: bool,
) -> bool {
    !cancel_token_cancelled && marker_present
}

/// Sentinel error message returned when a CC resume attempt produces an empty
/// Result immediately — the session was stale (e.g. expired after idle timeout).
/// Callers should retry with a fresh session.
pub(crate) const STALE_RESUME_ERROR: &str = "CC_STALE_RESUME";

/// Marker file written to each CC worktree identifying the owning workspace.
pub(crate) const WORKTREE_WORKSPACE_MARKER: &str = ".lucidos-workspace";

/// Engine-injected runtime directory under every workspace.
/// `ensure_workspace_bin_symlink` writes `.lucidos/bin/lucidos` (the CLI
/// symlink) here. External repos rarely gitignore `.lucidos/`, so without
/// the exclude every auto-commit drags the symlink along as a fake "diff".
/// `branch_changed_files` filters the same prefix so already-committed
/// instances also stop showing up.
pub(crate) const RUNTIME_PATH_PREFIX: &str = ".lucidos/";

/// Paths the engine writes into every CC worktree as runtime artifacts.
/// Each is appended to the worktree's `.git/info/exclude` at session start so
/// external repos never accumulate Lucidos-internal files in their git. Files
/// stay visible on disk (CC reads them); git just doesn't see them. No-op for
/// the Lucidos repo itself, where the skill file is intentionally tracked —
/// gitignore rules are silent for already-tracked paths.
pub(crate) const WORKTREE_EXCLUDE_PATHS: &[&str] = &[
    WORKTREE_WORKSPACE_MARKER,
    ".claude/skills/lucidos-cli/",
    RUNTIME_PATH_PREFIX,
];

/// True for paths the engine injects into every CC worktree (see
/// `WORKTREE_EXCLUDE_PATHS`). Trailing-`/` entries match by directory prefix;
/// other entries match exactly so `.lucidos-workspace-archive` doesn't
/// false-positive against `.lucidos-workspace`.
pub(crate) fn is_engine_injected_path(path: &str) -> bool {
    WORKTREE_EXCLUDE_PATHS.iter().any(|entry| {
        if let Some(dir) = entry.strip_suffix('/') {
            path == dir || path.starts_with(entry)
        } else {
            path == *entry
        }
    })
}

use super::agent_session::build_merge_prompt;
use super::change_ops::{CodingAgent, LiveSessionInfo};

/// Parameters for spawning a new CC thread.
///
/// `caller_title` — if Some(non-empty), used as the thread title and LLM
/// title generation is skipped. If None, a truncated-prompt placeholder is
/// emitted and an LLM-generated title replaces it asynchronously.
pub(crate) struct SpawnAgentThreadParams {
    pub prompt: String,
    pub user_images: Option<Vec<crate::api::ChatImage>>,
    pub device_id: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub spawning_event_id: Option<Uuid>,
    pub repo_id: Option<String>,
    pub caller_title: Option<String>,
}

impl CodingAgent for LucidosEngine {
    async fn is_running_for(&self, thread_id: Uuid) -> bool {
        self.is_agent_running_for(thread_id).await
    }

    fn spawn_hardening(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        self.spawn_hardening_session(
            thread_id,
            worktree_path,
            branch_name,
            auto_apply_change_id,
            actor,
        );
    }

    async fn live_session_info(&self, thread_id: Uuid) -> Option<LiveSessionInfo> {
        let guard = self.agent_sessions.lock().await;
        guard.get(&thread_id).and_then(|s| {
            if s.process_exited {
                return None;
            }
            Some(LiveSessionInfo {
                worktree_path: s.worktree_path.clone()?,
                idle_notify: s.idle_notify.clone(),
                msg_tx: s.msg_tx.clone(),
            })
        })
    }

    async fn merge_via_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        session: &LiveSessionInfo,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        self.merge_via_cc_session(
            thread_id,
            change_id,
            wt_path,
            branch_name,
            repo_root,
            &session.idle_notify,
            &session.msg_tx,
        )
        .await
    }

    fn spawn_merge_session(&self, thread_id: Uuid, change_id: Uuid, description: &str) {
        let engine = self.clone_arc();
        let description = description.to_string();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let change = match engine.changes().get_by_id(change_id).await {
                Some(c) => c,
                None => {
                    log!(
                        "[MergeConflict] Change {} not found for spawn_merge_session",
                        change_id
                    );
                    return;
                }
            };

            // `files` is the change's full file list, not necessarily
            // conflicting — Tier 2 has no worktree yet to probe.
            engine
                .emit_merge_conflict_detected(thread_id, change_id, change.files.clone())
                .await;

            let prompt = build_merge_prompt(
                &change.branch_name,
                Some("You are running in a temporary merge worktree."),
                Some(&description),
            );

            let origin_id = match engine.emit_automated_prompt(thread_id, &prompt, None).await {
                Ok(id) => id,
                Err(e) => {
                    log!(
                        "[MergeConflict] Failed to emit merge prompt for thread {}: {}",
                        thread_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Failed to emit prompt: {}", e),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[MergeConflict] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    &prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    Some(change_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
            match result {
                Ok(_) => {
                    log!(
                        "[MergeConflict] CC session completed for change {}",
                        change_id
                    );
                    engine.broadcast_changes_updated().await;
                }
                Err(e) => {
                    log!(
                        "[MergeConflict] CC session failed for change {}: {}",
                        change_id,
                        e
                    );
                    emit_background_task_failure(
                        &engine,
                        thread_id,
                        &e,
                        "[MergeConflict] spawn_merge_session failure",
                    )
                    .await;
                }
            }
        });
    }

    async fn lookup_session_id_for_resume(&self, thread_id: Uuid) -> Option<String> {
        match sqlx::query_scalar(
            "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                log!(
                    "[ClaudeCode] lookup_session_id_for_resume({}) DB error: {} — falling back to fresh session",
                    thread_id,
                    e
                );
                None
            }
        }
    }

    async fn run_merge_session_tier2(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let merge_prompt = build_merge_prompt("main", None, Some(description));
        let origin_id = self
            .emit_automated_prompt(thread_id, &merge_prompt, None)
            .await?;
        let request_id = Uuid::new_v4();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        self.run_direct_agent(
            request_id,
            thread_id,
            &merge_prompt,
            None,
            origin_id,
            None,
            &cancel_token,
            Some(change_id),
            Some((wt_path.to_path_buf(), branch_name.to_string())),
            None,
            None,
            resume_token,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }
}

/// Returns true iff `thread_summaries.status` is currently `'running'`.
/// Best-effort gate — caller and a concurrent settler can still both pass
/// this check before either's emit lands; treat as a duplicate-reducer,
/// not a guarantee.
pub(crate) async fn thread_is_running(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(status.as_deref()
        == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
}

/// Emit `ResponseFailed` for a background task that errored, but only if the
/// projection still shows the thread as `running`. Prevents double-terminal
/// in the common case where Stop/Discard already settled the thread; see
/// `thread_is_running` for the race caveat.
pub(crate) async fn emit_background_task_failure(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    error: impl std::fmt::Display,
    label: &str,
) {
    if thread_is_running(engine.pool(), thread_id)
        .await
        .unwrap_or(false)
    {
        engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                        error: error.to_string(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                label,
            )
            .await;
    }
}

/// Re-exported so call sites that already import from `claude_code` don't have
/// to chase the enum into `types.rs`.
pub use crate::engine::types::StopReason;

/// Emit a terminal `ResponseAborted` event for a thread the projection still
/// considers `running` but for which no live agent session or in-process loop
/// remains. Both callers (`stop_agent`, `interrupt_agent`) are user buttons
/// (Stop / Apply / Discard / Archive / Interrupt) — but no live response
/// exists to *cancel*, so this is system-driven cleanup of stuck projection
/// state. The user's actor flows onto the event so the chip reads "You" (the
/// user *did* push the button); the cause is `StaleSettle` so the summary
/// reads "Settled stuck response" rather than "Restarted" or "Response
/// interrupted".
///
/// Returns `Ok(true)` if an event was emitted, `Ok(false)` if the thread was
/// already settled (or doesn't exist).
///
/// Direct emit (rather than `emit_response_aborted`) so the caller can
/// observe `Err` and propagate to the HTTP handler.
pub(crate) async fn settle_stuck_running_thread(
    pool: &sqlx::PgPool,
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if !thread_is_running(pool, thread_id).await? {
        return Ok(false);
    }

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::StaleSettle,
        },
        meta: crate::engine::thread_events::EventMeta::with_actor(actor),
    })
    .await?;

    Ok(true)
}

/// Two liveness bits for a CC session, computed in one lock acquisition.
/// `actively_working` implies `running`; the converse is not true (idle
/// sessions are running but not actively working).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentLiveness {
    /// Session exists and its CC subprocess has not exited.
    pub running: bool,
    /// Session is running AND has an in-flight response (`is_waiting=false`).
    pub actively_working: bool,
}

impl LucidosEngine {
    /// Check if any Claude Code session is running for a specific thread.
    pub async fn is_agent_running_for(&self, thread_id: Uuid) -> bool {
        self.agent_liveness(thread_id).await.running
    }

    /// True iff a CC session for `thread_id` exists, has not exited, and is
    /// not at a turn boundary (`is_waiting=false`). Use this when you need to
    /// know whether an in-flight terminal event will fire after `stop_agent`
    /// — an idle session has no in-flight response, so the stop arm emits
    /// nothing and any wait for a fallout terminal would always time out.
    pub async fn is_agent_actively_working(&self, thread_id: Uuid) -> bool {
        self.agent_liveness(thread_id).await.actively_working
    }

    /// Snapshot both liveness bits in one lock acquisition. Use this when you
    /// need both — `archive_thread` calls it to decide whether to fire
    /// `stop_agent` (running) AND whether to wait for a fallout terminal
    /// (actively_working).
    pub async fn agent_liveness(&self, thread_id: Uuid) -> AgentLiveness {
        let guard = self.agent_sessions.lock().await;
        match guard.get(&thread_id) {
            Some(s) => AgentLiveness {
                running: !s.process_exited,
                actively_working: !s.process_exited && !s.is_waiting,
            },
            None => AgentLiveness::default(),
        }
    }

    /// Stop a running Claude Code session via the generic stop signal.
    /// `reason` is recorded on the session as `pending_stop` and read by the
    /// run_session loop's stop arm + post-loop cleanup — see `StopReason` for
    /// the per-variant terminator semantics.
    ///
    /// `actor` identifies the user who clicked the button. Flows into any
    /// resulting `ChangeApplied` / `ChangeApplyFailed` events stamped via
    /// the stale-session fallback. Engine-internal shutdowns pass `None`.
    ///
    /// `thread_id = None` stops every session (engine shutdown timeout path).
    ///
    /// No-live-session fallback mirrors `interrupt_agent`: cancel the
    /// in-process token, try `end_stale_waiting_session`, then settle any
    /// stuck `running` projection so the Discard button doesn't 404 on
    /// threads whose `spawn_agent_thread` errored before SessionStarted.
    pub async fn stop_agent(
        self: &Arc<Self>,
        reason: StopReason,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get_mut(&tid) {
                session.pending_stop = Some(reason);
                session.stop.notify_one();
                Ok(())
            } else {
                drop(guard);
                if self.cancel_thread(tid) {
                    return Ok(());
                }
                let discard = matches!(reason, StopReason::Discard);
                let stale_result = self
                    .end_stale_waiting_session(tid, discard, actor.clone())
                    .await;
                let settled =
                    settle_stuck_running_thread(&self.pool, &self.event_bus, tid, actor).await?;
                if settled {
                    Ok(())
                } else {
                    stale_result
                }
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            for session in guard.values_mut() {
                session.pending_stop = Some(reason);
                session.stop.notify_one();
            }
            Ok(())
        }
    }

    /// Snapshot a session's `pending_stop` reason. Encapsulates the lock
    /// acquisition so the run_session loop and its post-loop cleanup don't
    /// have to know which field carries the stop reason — flipping `bool`
    /// fields back and forth here used to be the source of the phantom
    /// `ResponseCanceled` regressions.
    pub(crate) async fn pending_stop_reason(&self, thread_id: Uuid) -> Option<StopReason> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|s| s.pending_stop)
    }

    /// Interrupt a running Claude Code session — sends control_request:interrupt to stop
    /// current work without killing the session (like pressing Esc in the CC terminal).
    /// The CC process stays alive and enters waiting state.
    ///
    /// `actor` flows onto the `ResponseCanceled` emitted by the no-session
    /// settle fallback so the panel reads "You" instead of "⚙ System".
    ///
    /// Fallbacks when no live CC subprocess exists for `thread_id`:
    ///   1. Cancel the in-process agentic loop via the `active_threads` token
    ///      (handles run_thread-spawned chat threads and CC threads that are
    ///      mid-startup, before SessionStarted has registered an agent_session).
    ///      When the cancel lands, `run_session`'s `chat_cancel` arm emits the
    ///      terminal `ResponseCanceled` — skip the settle to avoid double-emit.
    ///   2. Only if the cancel had no entry to land on (truly stuck — spawn
    ///      task errored and was lost), settle by emitting `ResponseCanceled`
    ///      ourselves so the UI unsticks.
    ///
    /// Unlike `stop_agent`, interrupt is always a real Cancel/Stop click — the
    /// frontend offers no "interrupt for Apply/Discard/Archive" path.
    pub async fn interrupt_agent(
        &self,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get(&tid) {
                if session.is_waiting {
                    return Err("Session is already waiting".into());
                }
                session.interrupt.notify_one();
                Ok(())
            } else {
                drop(guard);
                if self.cancel_thread(tid) {
                    return Ok(());
                }
                settle_stuck_running_thread(&self.pool, &self.event_bus, tid, actor).await?;
                Ok(())
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            for session in guard.values() {
                if !session.is_waiting {
                    session.interrupt.notify_one();
                }
            }
            Ok(())
        }
    }

    /// Send a control request to a running Claude Code session via its control channel.
    pub async fn send_agent_control_request(
        &self,
        thread_id: Uuid,
        request: crate::runtime::ControlRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = crate::engine::thread_events::ThreadEvent::from_control_request(
            &request,
            crate::runtime::AgentKind::ClaudeCode,
        );
        {
            let mut guard = self.agent_sessions.lock().await;
            let session = guard
                .get_mut(&thread_id)
                .ok_or("No Claude Code session found for this thread")?;
            if let crate::runtime::ControlRequest::SetModel { ref model } = request {
                session.current_model = Some(model.clone());
            }
            if let crate::runtime::ControlRequest::SetReasoningEffort { ref effort } = request {
                session.current_reasoning_effort = Some(effort.clone());
            }
            session
                .control_tx
                .send(request)
                .map_err(|_| "Control channel closed")?;
        }
        if let Some(ev) = event {
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: ev,
                    meta: crate::engine::thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                })
                .await
            {
                log!(
                    "[ClaudeCode] Failed to persist CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }
        Ok(())
    }

    /// Query the latest CC settings (model, reasoning_effort) for a thread from events.
    /// Returns (model, effort) — each is None if never changed.
    pub(crate) async fn cc_thread_settings(
        &self,
        thread_id: Uuid,
    ) -> (Option<String>, Option<String>) {
        let tid = thread_id.to_string();
        let rows: Vec<(serde_json::Value,)> = match sqlx::query_as(
            "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
             ORDER BY created DESC LIMIT 10",
        )
        .bind(&tid)
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log!(
                    "[ClaudeCode] cc_thread_settings({}) DB error: {} — falling back to no per-thread overrides",
                    thread_id,
                    e
                );
                Vec::new()
            }
        };
        // Fold from newest to oldest — first non-null value wins for each field
        let mut model: Option<String> = None;
        let mut effort: Option<String> = None;
        for (payload,) in &rows {
            if model.is_none() {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
                }
            }
            if effort.is_none() {
                if let Some(e) = payload.get("reasoning_effort").and_then(|v| v.as_str()) {
                    effort = Some(e.to_string());
                }
            }
            if model.is_some() && effort.is_some() {
                break;
            }
        }
        (model, effort)
    }

    pub async fn cc_categorized_commands(&self, thread_id: Uuid) -> CcCommandsResult {
        let guard = self.agent_sessions.lock().await;
        if let Some(s) = guard.get(&thread_id) {
            let has_active = !s.process_exited;
            let mut info = s.to_commands_info();
            let model = s.current_model.clone();
            let effort = s.current_reasoning_effort.clone();
            // Session exists but Init hasn't arrived yet — use repo cache for commands
            if info.builtin_commands.is_empty() && info.skill_commands.is_empty() {
                if let Some(ref repo) = s.repo_root {
                    let repo_key = repo.to_string_lossy().to_string();
                    drop(guard);
                    let cache = self.cc_commands_cache.read().await;
                    if let Some(cached) = cache.get(&repo_key) {
                        info.builtin_commands = cached.builtin_commands.clone();
                        info.skill_commands = cached.skill_commands.clone();
                    }
                }
            }
            return CcCommandsResult {
                info,
                has_active_session: has_active,
                current_model: model,
                current_reasoning_effort: effort,
            };
        }
        drop(guard);
        // No live session — get commands from repo cache, settings from thread events
        let repo_root: Option<String> = sqlx::query_scalar(
            "SELECT repo_root FROM changes WHERE thread_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| {
            log!(
                "[ClaudeCode] Failed to look up repo_root for {}: {}",
                thread_id,
                e
            );
            e
        })
        .ok()
        .flatten();
        let info = if let Some(repo_key) = repo_root {
            let cache = self.cc_commands_cache.read().await;
            lookup_repo_commands_in_cache(&cache, &repo_key)
        } else {
            // Thread has no recorded repo (no `changes` row yet) — return
            // empty rather than leaking another repo's skills.
            CcCommandsInfo::default()
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: model,
            current_reasoning_effort: effort,
        }
    }

    /// Return cached commands for a specific repo path (for compose-view menu).
    /// Empty result if the repo has never had a CC session — never falls back
    /// to another repo's cache.
    pub async fn cc_commands_for_repo(&self, repo_path: &Path) -> CcCommandsResult {
        let cache = self.cc_commands_cache.read().await;
        let info = lookup_repo_commands_in_cache(&cache, &repo_path.to_string_lossy());
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: None,
            current_reasoning_effort: None,
        }
    }

    /// Spawn a CC subprocess to run `/harden` on the given branch's worktree.
    /// When `auto_apply_change_id` is `Some`, the apply is re-entered after
    /// hardening completes (or `ChangeApplyFailed` is emitted on failure).
    /// `actor` carries the user who initiated the original apply so the
    /// resulting `ChangeApplied` stamps that user instead of the engine
    /// fallback.
    pub(crate) fn spawn_hardening_session(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone_arc();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let prompt = "Your changes have not been hardened. Run /harden now.";

            // Engine-retriggered harden: stamp with the dedicated reason so the
            // route popover surfaces "Engine · Harden auto-retrigger" instead of
            // "Unknown".
            let origin = Some(MessageOrigin::engine(EngineReason::HardenRetrigger));
            let origin_id = match engine.emit_automated_prompt(thread_id, prompt, origin).await {
                Ok(id) => id,
                Err(e) => {
                    log!(
                        "[ClaudeCode] Failed to emit hardening prompt for thread {}: {}",
                        thread_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Failed to emit prompt: {}", e),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();

            let hardening_system_prompt = format!(
                "HARDENING SESSION: You are hardening changes on branch `{}`. \
                 Your ONLY task is to run the /harden skill. Do NOT continue any implementation work, \
                 do NOT look at implementation plans, do NOT add features. \
                 Just harden the existing changes for quality and correctness.\n\n\
                 CRITICAL: Never run `exit` as a bash command.",
                branch_name
            );
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    None,
                    Some((worktree_path, branch_name)), // recovery_worktree — reuse existing worktree
                    None,
                    Some(hardening_system_prompt),
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            let Some(change_id) = auto_apply_change_id else {
                match result {
                    Ok(_) => log!(
                        "[ClaudeCode] Hardening session completed for thread {}",
                        thread_id
                    ),
                    Err(e) => log!(
                        "[ClaudeCode] Hardening session failed for thread {}: {}",
                        thread_id,
                        e
                    ),
                }
                return;
            };

            match result {
                Ok(_) => {
                    let was_aborted = cancel_token.is_cancelled();
                    let hardened = match engine.changes().get_by_id(change_id).await {
                        Some(c) => {
                            crate::engine::change_ops::branch_is_hardened(
                                &engine.pool,
                                engine.changes(),
                                std::path::Path::new(&c.repo_root),
                                &c.branch_name,
                            )
                            .await
                        }
                        None => false,
                    };

                    if !hardening_succeeded(was_aborted, hardened) {
                        log!(
                            "[ClaudeCode] Hardening for change {} did not succeed (aborted={}, hardened={}) — skipping apply",
                            change_id,
                            was_aborted,
                            hardened
                        );
                        engine
                            .emit_apply_failed_unhardened(
                                thread_id,
                                &change_id.to_string(),
                                actor.clone(),
                                "[ClaudeCode] ChangeApplyFailed (incomplete hardening)",
                            )
                            .await;
                        engine.broadcast_changes_updated().await;
                        return;
                    }

                    log!(
                        "[ClaudeCode] Hardening completed for change {}, auto-applying",
                        change_id
                    );
                    // Emit ChangeHardened so apply_change doesn't re-enter the
                    // unhardened path and respawn hardening.
                    engine
                        .emit_change_hardened(thread_id, change_id, "[ClaudeCode] ChangeHardened")
                        .await;
                    match engine.apply_change(change_id, actor.clone()).await {
                        Ok(r) => {
                            log!(
                                "[ClaudeCode] Auto-applied change {} after hardening: {}",
                                change_id,
                                r.message
                            );
                            engine.broadcast_changes_updated().await;
                        }
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Auto-apply failed after hardening for change {}: {}",
                                change_id,
                                e
                            );
                            engine
                                .event_bus
                                .emit_or_log(
                                    crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event:
                                            crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                                change_id: change_id.to_string(),
                                                error: e.to_string(),
                                                actor: actor.clone(),
                                            },
                                        meta: crate::engine::thread_events::EventMeta::NONE,
                                    },
                                    "[ClaudeCode] ChangeApplyFailed",
                                )
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log!(
                        "[ClaudeCode] Hardening session failed for change {}: {}",
                        change_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: change_id.to_string(),
                                        error: format!("Hardening failed: {}", e),
                                        actor: actor.clone(),
                                    },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ChangeApplyFailed (hardening)",
                        )
                        .await;
                }
            }
        });
    }

    /// Discard pending CC changes without ending the session.
    /// Resets the worktree to main and re-enters idle state.
    ///
    /// `actor` is the user who clicked Discard — propagated to any
    /// `ChangeApplyFailed` emitted by the stale-session fallback so the
    /// resulting event carries the real actor.
    pub async fn discard_cc_changes(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let wt = {
            let guard = self.agent_sessions.lock().await;
            if let Some(session) = guard.get(&thread_id) {
                session
                    .worktree_path
                    .clone()
                    .ok_or("No worktree for this session")?
            } else {
                // No live session — fall back to stale session handling.
                // discard=true because this is the user-clicked Discard
                // button: explicit user intent.
                return self
                    .end_stale_waiting_session(thread_id, true, actor)
                    .await;
            }
        };

        self.discard_pending_for_thread(thread_id, actor).await;

        self.reset_worktree_and_idle(thread_id, &wt).await;

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// Spawn a Claude Code session in a new independent thread.
    ///
    /// Creates a new thread_id, persists a MessageReceived event, sets a title
    /// (caller-provided or generated), and spawns CC in a background tokio task.
    /// Returns the new thread_id immediately.
    pub(crate) async fn spawn_agent_thread(
        &self,
        params: SpawnAgentThreadParams,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let SpawnAgentThreadParams {
            prompt,
            user_images,
            device_id,
            parent_thread_id,
            spawning_event_id,
            repo_id,
            caller_title,
        } = params;

        let cc_thread_id = Uuid::new_v4();
        let explicit_title = caller_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let has_explicit_title = explicit_title.is_some();
        let title = explicit_title.unwrap_or_else(|| prompt.chars().take(60).collect::<String>());

        let device_name = if let Some(ref did) = device_id {
            crate::core::DeviceStore::display_name(&self.pool, did).await
        } else {
            None
        };

        // Persist + broadcast MessageReceived for the CC thread.
        // Route through the canonical synthesizer so `mode + parent_thread_id`
        // produces a `ParentThread { mode: Agent }` origin — without it, the
        // frontend's `originMode(undefined)` defaults to `engine` and the chip
        // mislabels the spawn as engine-initiated work.
        let emit_result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: cc_thread_id,
                event: crate::engine::chat::make_message_received(
                    &self.workspace_path,
                    &prompt,
                    user_images.as_deref(),
                    device_id.as_deref(),
                    device_name,
                    parent_thread_id,
                    spawning_event_id,
                    ActorMode::Agent,
                    None,
                    None,
                    None,
                ),
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?
            .expect("persisted event must return EmitResult");
        let origin_id = emit_result.event_id;

        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: cc_thread_id,
                event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                    title: title.clone(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
        {
            log!("[ClaudeCode] Failed to emit title: {}", e);
        }

        if !has_explicit_title {
            if let Some(ref extractor) = self.extractor {
                let title_model =
                    crate::core::PreferenceStore::get(&self.pool, crate::core::PREF_MODEL_TITLE)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                let provider = extractor.provider_for_model(&title_model);
                let msg = prompt.clone();
                let bus = self.event_bus.clone();
                tokio::spawn(async move {
                    // CC prompts arrive as text; image attachments don't flow this path.
                    super::chat::emit_generated_title(
                        &bus,
                        &provider,
                        cc_thread_id,
                        &msg,
                        None,
                        None,
                        0,
                    )
                    .await;
                });
            }
        }

        // Notify parent thread about the new CC thread via EventBus
        // CodingAgentThreadSpawned is transient — just tells the frontend a new thread was spawned
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id: cc_thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentThreadSpawned {
                        cc_thread_id: cc_thread_id.to_string(),
                        title: title.clone(),
                        agent: crate::runtime::AgentKind::ClaudeCode,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[ClaudeCode] CodingAgentThreadSpawned",
            )
            .await;

        // Register the CC thread as active BEFORE spawning the background task.
        // This ensures list_threads API includes it in the active set immediately,
        // preventing a race where loadAllThreads sees the thread (from persisted
        // events) but not in the active set, causing it to show in History.
        let (cancel_token, _injection_rx, guard) = self.register_thread(cc_thread_id);

        // Spawn CC in a background task
        let engine = self.clone_arc();
        let images = user_images;

        let handle = tokio::spawn(async move {
            // Guard moved here — auto-unregisters on drop when task ends
            let _guard = guard;

            // All events flow through EventBus — no forwarder needed
            let result = engine
                .run_direct_agent(
                    cc_thread_id,
                    cc_thread_id,
                    &prompt,
                    images.as_deref(),
                    origin_id,
                    spawning_event_id,
                    &cancel_token,
                    None,
                    None,
                    repo_id,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            match result {
                Ok(ref res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            let pending = engine.changes().list_pending().await;
                            if let Some(change) =
                                pending.iter().find(|c| c.request_id == res.request_id)
                            {
                                match engine.apply_change(change.id, None).await {
                                    Ok(r) => {
                                        log!("[ClaudeCode] Auto-applied change: {}", r.message)
                                    }
                                    Err(e) => {
                                        log!("[ClaudeCode] Failed to auto-apply: {}", e);
                                        engine
                                            .event_bus
                                            .emit_or_log(
                                                crate::engine::event_bus::BusEvent::Thread {
                                                    thread_id: cc_thread_id,
                                                    event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                                        change_id: change.id.to_string(),
                                                        error: e.to_string(),
                                                        actor: None,
                                                    },
                                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                                },
                                                "[ClaudeCode] ChangeApplyFailed",
                                            )
                                            .await;
                                    }
                                }
                            }
                        }

                        engine.broadcast_changes_updated().await;
                    }
                }
                Err(e) => {
                    log!("[ClaudeCode] Background CC session failed: {}", e);
                    emit_background_task_failure(
                        &engine,
                        cc_thread_id,
                        &e,
                        "[ClaudeCode] spawn_agent_thread failure",
                    )
                    .await;
                }
            }
        });

        Self::monitor_cc_task(self.clone_arc(), cc_thread_id, handle);

        Ok(cc_thread_id)
    }
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod tests;
