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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AllowScope {
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

/// Emit a terminal `ResponseCanceled` event for a thread the projection still
/// considers `running` but for which no live agent session or in-process loop
/// remains. Both callers (`cancel_agent`, `interrupt_agent`) are user stop
/// clicks, so the actor flows onto the event.
///
/// Returns `Ok(true)` if an event was emitted, `Ok(false)` if the thread was
/// already settled (or doesn't exist).
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
        event: crate::engine::thread_events::ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: crate::engine::thread_events::EventMeta::with_actor(actor),
    })
    .await?;

    Ok(true)
}

impl LucidosEngine {
    /// Check if any Claude Code session is running for a specific thread.
    pub async fn is_agent_running_for(&self, thread_id: Uuid) -> bool {
        let guard = self.agent_sessions.lock().await;
        guard
            .get(&thread_id)
            .map(|s| !s.process_exited)
            .unwrap_or(false)
    }

    /// Cancel a running Claude Code process via notify signal.
    /// If `auto_apply` is true, the resulting proposed change will be applied immediately.
    /// If `thread_id` is provided, cancel that specific session; otherwise cancel all.
    ///
    /// `actor` identifies the user who clicked Stop / Apply / Archive — flows into
    /// any resulting `ChangeApplied` / `ChangeApplyFailed` events stamped via
    /// the stale-session fallback. HTTP callers build it; engine-internal
    /// shutdowns pass `None`.
    ///
    /// No-live-session fallback mirrors `interrupt_agent`: cancel the
    /// in-process token, try `end_stale_waiting_session`, then settle any
    /// stuck `running` projection so the Discard button doesn't 404 on
    /// threads whose `spawn_agent_thread` errored before SessionStarted.
    pub async fn cancel_agent(
        self: &Arc<Self>,
        auto_apply: bool,
        discard: bool,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get_mut(&tid) {
                session.auto_apply = auto_apply;
                session.discard = discard;
                session.cancel.notify_one();
                Ok(())
            } else {
                drop(guard);
                if self.cancel_thread(tid) {
                    return Ok(());
                }
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
                session.auto_apply = auto_apply;
                session.discard = discard;
                session.cancel.notify_one();
            }
            Ok(())
        }
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
                    log!(
                        "[ClaudeCode] Hardening completed for change {}, auto-applying",
                        change_id
                    );
                    // Emit ChangeHardened — projection updates from the event so
                    // apply_change won't re-enter the unhardened path and spawn
                    // another hardening → infinite loop.
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
mod tests {
    use super::{
        append_allowed_tool_pattern, cc_allowed_tools, derive_allow_pattern,
        lookup_repo_commands_in_cache, read_allowed_tools_file, settle_stuck_running_thread,
        write_allowed_tools_file, AllowScope, CC_ALLOWED_TOOLS_HEADER, DEFAULT_CC_ALLOWED_TOOLS,
    };
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::engine::types::CcCommandsInfo;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn cache_with(entries: &[(&str, &[&str])]) -> HashMap<String, CcCommandsInfo> {
        entries
            .iter()
            .map(|(path, skills)| {
                (
                    (*path).to_string(),
                    CcCommandsInfo {
                        builtin_commands: vec![],
                        skill_commands: skills.iter().map(|s| (*s).to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn lookup_returns_cached_entry_for_matching_repo() {
        let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
        let info = lookup_repo_commands_in_cache(&cache, "/repo/a");
        assert_eq!(info.skill_commands, vec!["skill-a".to_string()]);
    }

    /// Regression test for the bug where the compose-view menu returned
    /// `cache.values().next()` — i.e., an arbitrary other repo's skills —
    /// when the requested repo had no cache entry. Skills from a non-selected
    /// repo must NEVER surface.
    #[test]
    fn lookup_returns_empty_for_unknown_repo_never_falls_back_to_other_repos() {
        let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
        let info = lookup_repo_commands_in_cache(&cache, "/repo/never-cached");
        assert!(info.skill_commands.is_empty(), "must not leak other repos' skills");
        assert!(info.builtin_commands.is_empty());
    }

    #[test]
    fn lookup_returns_empty_for_empty_cache() {
        let cache: HashMap<String, CcCommandsInfo> = HashMap::new();
        let info = lookup_repo_commands_in_cache(&cache, "/any/path");
        assert!(info.skill_commands.is_empty());
        assert!(info.builtin_commands.is_empty());
    }

    /// Emit MessageReceived for a CC-channel thread → status='running'.
    /// Mirrors what `spawn_agent_thread` does before kicking off the bg task.
    async fn seed_running_cc_thread(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "do the thing".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn read_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    fn user_device_actor() -> crate::engine::thread_events::MessageOrigin {
        crate::engine::thread_events::MessageOrigin::Device {
            device_id: "test-device".into(),
            label: "Test Device".into(),
        }
    }

    /// User clicks Stop on a CC thread that's stuck at status='running' (the
    /// background spawn task errored before any terminal event could fire, or
    /// the CC subprocess hadn't yet registered in agent_sessions when the
    /// user pressed cancel). The settle helper emits `ResponseCanceled` with
    /// the user actor — NOT `ResponseAborted` with no actor (which the prior
    /// implementation produced and rendered as "⚙ System — Response
    /// interrupted", with a confusing Continue panel).
    #[tokio::test]
    async fn settle_stuck_running_thread_emits_canceled_with_user_actor() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_running_cc_thread(&bus, thread_id).await;
        assert_eq!(read_status(&pool, thread_id).await.as_deref(), Some("running"));

        let did_emit = settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap();
        assert!(did_emit, "stuck running thread should be settled");

        // Status: a user-driven cancel from a CC thread that never produced
        // a SessionStarted lands as `failed` in the projection (mirrors the
        // chat-side cancel-without-response path). The point of the change
        // is the *event type* and *actor*, not the projection bucket.
        let canceled_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(canceled_count, 1, "exactly one ResponseCanceled must be persisted");

        let aborted_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            aborted_count, 0,
            "settle on a user-driven cancel must NOT emit ResponseAborted — that renders as 'System' \
             and creates a misleading Continue panel"
        );

        // Actor must be persisted so the AbortPanel/exchange status reads "You" not "System".
        let actor: serde_json::Value = sqlx::query_scalar(
            "SELECT payload->'actor' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actor["kind"], "device", "actor.kind must be 'device' (user from a known device)");
        assert_eq!(actor["device_id"], "test-device");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Idempotency: settling a thread that's already non-running is a no-op
    /// (so that double-clicks on the stop button don't pile up events).
    #[tokio::test]
    async fn settle_stuck_running_thread_no_op_when_already_settled() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_running_cc_thread(&bus, thread_id).await;
        // First settle transitions running → failed.
        assert!(settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap());
        // Second settle should be a no-op.
        assert!(!settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap());

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "second settle must not emit a duplicate event");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A thread that the projection never knew about (no thread_summaries row)
    /// is also a no-op — interrupt of an unknown id should not emit phantom
    /// events for non-existent threads.
    #[tokio::test]
    async fn settle_stuck_running_thread_no_op_for_unknown_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let did_emit = settle_stuck_running_thread(&pool, &bus, Uuid::new_v4(), Some(user_device_actor()))
            .await
            .unwrap();
        assert!(!did_emit);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn cc_allowed_tools_returns_default_when_user_dir_missing() {
        assert_eq!(cc_allowed_tools(None), DEFAULT_CC_ALLOWED_TOOLS.join(","));
    }

    #[test]
    fn cc_allowed_tools_seeds_empty_default_file_on_first_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cc-allowed-tools");
        assert!(!path.exists());

        let result = cc_allowed_tools(Some(dir.path()));

        assert_eq!(result, "", "default allowlist must be empty");
        assert!(path.exists(), "seed file should have been written");
        let seeded = std::fs::read_to_string(&path).unwrap();
        assert_eq!(seeded, CC_ALLOWED_TOOLS_HEADER);
    }

    #[test]
    fn derive_allow_pattern_skill_narrow_uses_plugin_glob() {
        let input = serde_json::json!({ "skill": "code-review:code-review" });
        assert_eq!(
            derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
            Some("Skill(code-review:*)"),
        );
    }

    #[test]
    fn derive_allow_pattern_skill_narrow_with_no_colon_uses_full_name() {
        let input = serde_json::json!({ "skill": "loop" });
        assert_eq!(
            derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
            Some("Skill(loop:*)"),
        );
    }

    #[test]
    fn derive_allow_pattern_skill_broad_returns_bare_tool_name() {
        let input = serde_json::json!({ "skill": "code-review:code-review" });
        assert_eq!(
            derive_allow_pattern("Skill", &input, AllowScope::Broad).as_deref(),
            Some("Skill"),
        );
    }

    #[test]
    fn derive_allow_pattern_bash_narrow_uses_first_token() {
        let input = serde_json::json!({ "command": "git status --short" });
        assert_eq!(
            derive_allow_pattern("Bash", &input, AllowScope::Narrow).as_deref(),
            Some("Bash(git:*)"),
        );
    }

    #[test]
    fn derive_allow_pattern_bash_broad_returns_bare_tool_name() {
        let input = serde_json::json!({ "command": "ls" });
        assert_eq!(
            derive_allow_pattern("Bash", &input, AllowScope::Broad).as_deref(),
            Some("Bash"),
        );
    }

    #[test]
    fn derive_allow_pattern_other_tool_narrow_returns_none() {
        let input = serde_json::json!({ "file_path": "/tmp/x" });
        assert_eq!(derive_allow_pattern("Read", &input, AllowScope::Narrow), None);
    }

    #[test]
    fn derive_allow_pattern_other_tool_broad_returns_bare_name() {
        let input = serde_json::json!({});
        assert_eq!(
            derive_allow_pattern("Read", &input, AllowScope::Broad).as_deref(),
            Some("Read"),
        );
    }

    #[test]
    fn derive_allow_pattern_skill_missing_input_returns_none() {
        let input = serde_json::json!({});
        assert_eq!(derive_allow_pattern("Skill", &input, AllowScope::Narrow), None);
    }

    /// CC's `--permission-mode acceptEdits` routes parametric file-path tools
    /// (Edit/Write/NotebookEdit) through `--permission-prompt-tool` for any
    /// out-of-cwd path **regardless** of bare entries in `--allowedTools`.
    /// Persisting bare `Edit` (etc.) silently does nothing for the very paths
    /// that surfaced the prompt — so the engine must refuse to write them.
    /// In-cwd paths never reach this card (acceptEdits auto-approves), so no
    /// legitimate caller is denied by this guard.
    #[test]
    fn derive_allow_pattern_broad_returns_none_for_acceptedits_routed_tools() {
        let input = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
        assert_eq!(
            derive_allow_pattern("Edit", &input, AllowScope::Broad),
            None,
            "broad Edit must be suppressed — bare entry doesn't bypass acceptEdits routing"
        );
        assert_eq!(
            derive_allow_pattern("Write", &input, AllowScope::Broad),
            None,
            "broad Write must be suppressed for the same reason"
        );
        assert_eq!(
            derive_allow_pattern("NotebookEdit", &input, AllowScope::Broad),
            None,
            "broad NotebookEdit must be suppressed for the same reason"
        );
    }

    /// CC always routes `ExitPlanMode` through `--permission-prompt-tool`
    /// regardless of `--allowedTools` — plan-mode exit is the user's plan
    /// approval step, not a regular tool call. Persisting bare `ExitPlanMode`
    /// to the allowlist would mislead the user into thinking the prompt would
    /// stop coming back; suppress so the UI hides the broad button.
    #[test]
    fn derive_allow_pattern_broad_returns_none_for_exit_plan_mode() {
        let input = serde_json::json!({ "plan": "Step 1: do thing" });
        assert_eq!(
            derive_allow_pattern("ExitPlanMode", &input, AllowScope::Broad),
            None,
            "broad ExitPlanMode must be suppressed — CC always prompts for plan approval"
        );
    }

    /// Narrow scope for the same tools is still None (no narrow pattern is
    /// generated for path-tools today). Documented here so a future addition
    /// of `Edit(<glob>)` patterns is an intentional change, not a side-effect.
    #[test]
    fn derive_allow_pattern_narrow_remains_none_for_path_tools() {
        let input = serde_json::json!({"file_path": "/x"});
        assert_eq!(derive_allow_pattern("Edit", &input, AllowScope::Narrow), None);
        assert_eq!(derive_allow_pattern("Write", &input, AllowScope::Narrow), None);
        assert_eq!(derive_allow_pattern("NotebookEdit", &input, AllowScope::Narrow), None);
    }

    #[test]
    fn derive_allow_pattern_session_edit_uses_per_file_scope() {
        let input = serde_json::json!({
            "file_path": "/Users/me/repo/.claude/commands/harden.md",
            "old_string": "x",
            "new_string": "y",
        });
        assert_eq!(
            derive_allow_pattern("Edit", &input, AllowScope::Session).as_deref(),
            Some("Edit(/Users/me/repo/.claude/commands/harden.md)"),
        );
    }

    #[test]
    fn derive_allow_pattern_session_write_uses_per_file_scope() {
        let input = serde_json::json!({
            "file_path": "/tmp/new.txt",
            "content": "hello",
        });
        assert_eq!(
            derive_allow_pattern("Write", &input, AllowScope::Session).as_deref(),
            Some("Write(/tmp/new.txt)"),
        );
    }

    #[test]
    fn derive_allow_pattern_session_notebookedit_uses_notebook_path() {
        let input = serde_json::json!({
            "notebook_path": "/tmp/nb.ipynb",
            "new_source": "print(1)",
        });
        assert_eq!(
            derive_allow_pattern("NotebookEdit", &input, AllowScope::Session).as_deref(),
            Some("NotebookEdit(/tmp/nb.ipynb)"),
        );
    }

    #[test]
    fn derive_allow_pattern_session_two_edits_to_same_file_match() {
        // Different `old_string`/`new_string` payloads must derive the same
        // session pattern so the second prompt auto-resolves against the first.
        let a = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
        let b = serde_json::json!({"file_path": "/x", "old_string": "c", "new_string": "d"});
        assert_eq!(
            derive_allow_pattern("Edit", &a, AllowScope::Session),
            derive_allow_pattern("Edit", &b, AllowScope::Session),
        );
    }

    #[test]
    fn derive_allow_pattern_session_edit_missing_path_returns_none() {
        let input = serde_json::json!({"old_string": "x"});
        assert_eq!(derive_allow_pattern("Edit", &input, AllowScope::Session), None);
    }

    #[test]
    fn derive_allow_pattern_session_bash_reuses_narrow_pattern() {
        let input = serde_json::json!({"command": "git push origin main"});
        assert_eq!(
            derive_allow_pattern("Bash", &input, AllowScope::Session).as_deref(),
            Some("Bash(git:*)"),
        );
    }

    #[test]
    fn derive_allow_pattern_session_skill_reuses_narrow_pattern() {
        let input = serde_json::json!({"skill": "superpowers:test-driven-development"});
        assert_eq!(
            derive_allow_pattern("Skill", &input, AllowScope::Session).as_deref(),
            Some("Skill(superpowers:*)"),
        );
    }

    /// Bare-tool fallback for tools without a narrow sub-scope. Session scope
    /// is engine-side, so `BROAD_ALLOW_INEFFECTIVE` does not apply — the user
    /// gets to remember any prompt for the rest of the thread.
    #[test]
    fn derive_allow_pattern_session_other_tool_falls_back_to_bare_name() {
        let input = serde_json::json!({"pattern": "foo"});
        assert_eq!(
            derive_allow_pattern("Read", &input, AllowScope::Session).as_deref(),
            Some("Read"),
        );
    }

    #[test]
    fn append_allowed_tool_pattern_creates_file_with_header() {
        let dir = tempfile::tempdir().unwrap();
        append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
        let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
        assert!(body.starts_with(CC_ALLOWED_TOOLS_HEADER));
        assert!(body.trim_end().ends_with("Skill(code-review:*)"));
    }

    #[test]
    fn append_allowed_tool_pattern_skips_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
        append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
        let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
        assert_eq!(body.matches("Skill\n").count(), 1);
    }

    #[test]
    fn append_allowed_tool_pattern_appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cc-allowed-tools"),
            "# header\nBash\nRead\n",
        )
        .unwrap();
        append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
        let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
        assert_eq!(body, "# header\nBash\nRead\nSkill(code-review:*)\n");
    }

    #[test]
    fn append_allowed_tool_pattern_treats_existing_pattern_as_present_even_with_indent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cc-allowed-tools"),
            "# header\n  Skill(code-review:*)  \n",
        )
        .unwrap();
        append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
        let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
        assert_eq!(body, "# header\n  Skill(code-review:*)  \n");
    }

    #[test]
    fn append_allowed_tool_pattern_no_op_when_user_dir_none() {
        // Should not panic and should not error.
        append_allowed_tool_pattern(None, "Bash").unwrap();
    }

    #[test]
    fn read_allowed_tools_file_returns_header_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let body = read_allowed_tools_file(dir.path()).unwrap();
        assert_eq!(body, CC_ALLOWED_TOOLS_HEADER);
    }

    #[test]
    fn write_then_read_allowed_tools_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let payload = "# notes\nBash\nSkill(meta:*)\n";
        write_allowed_tools_file(dir.path(), payload).unwrap();
        assert_eq!(read_allowed_tools_file(dir.path()).unwrap(), payload);
    }

    #[test]
    fn default_cc_allowed_tools_is_empty() {
        assert_eq!(DEFAULT_CC_ALLOWED_TOOLS, &[] as &[&str]);
    }

    #[test]
    fn cc_allowed_tools_parses_user_file_strips_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cc-allowed-tools"),
            "# header comment\n\nBash\nRead\n  Edit  \n# inline comment\nSkill(superpowers:*)\n\n",
        )
        .unwrap();

        assert_eq!(
            cc_allowed_tools(Some(dir.path())),
            "Bash,Read,Edit,Skill(superpowers:*)",
        );
    }

    /// Validates that idle_notify (using notify_waiters) wakes a registered waiter.
    /// This is the contract that send_and_wait and apply_now_inner depend on:
    /// the Result handler must call idle_notify.notify_waiters() so that
    /// any task waiting on idle_notify.notified() wakes up.
    #[tokio::test]
    async fn idle_notify_wakes_registered_waiter() {
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify2 = notify.clone();

        // Simulate send_and_wait: register a waiter BEFORE the notification
        let waiter = tokio::spawn(async move {
            // Use the same 5s timeout pattern as send_and_wait
            match tokio::time::timeout(std::time::Duration::from_secs(2), notify2.notified()).await
            {
                Ok(()) => true,  // Woken by notify_waiters
                Err(_) => false, // Timed out — notify_waiters was never called
            }
        });

        // Give the waiter time to register
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Simulate the Result handler firing idle_notify
        notify.notify_waiters();

        let woken = waiter.await.unwrap();
        assert!(
            woken,
            "idle_notify.notify_waiters() must wake registered waiters — \
                         this is the contract that send_and_wait depends on"
        );
    }

    /// Validates that notify_waiters does NOT store a permit — calling it
    /// BEFORE a waiter registers means the waiter misses the notification.
    /// This is why bare `idle_notify.notified().await` (without a poll loop)
    /// is dangerous: if the notification fires before the await starts, it's lost.
    #[tokio::test]
    async fn notify_waiters_does_not_store_permit() {
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());

        // Fire notification BEFORE any waiter is registered
        notify.notify_waiters();

        // Now try to wait — should NOT wake up (permit not stored)
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;

        assert!(result.is_err(), "notify_waiters() must NOT store a permit — \
                                   bare .notified().await after a missed notification hangs forever");
    }

    /// Validates the resume branch reuse logic: when a branch exists,
    /// `git worktree add` should use the existing branch (no -b flag)
    /// instead of creating a new one.
    #[tokio::test]
    async fn resume_branch_reuses_existing_branch() {
        use crate::engine::git_ops::git_cmd;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo with an initial commit
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Create a branch with a commit (simulating a previous CC session)
        let branch_name = "claude-code/20260326-test";
        let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
        let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
        let _ = git_cmd(&["checkout", "main"], repo).await;

        // Verify the branch exists
        let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(exists, "Test branch should exist");

        // Create a worktree from the existing branch (no -b flag)
        let wt_path = tmp.path().join("worktree-resume");
        let result = git_cmd(
            &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Should create worktree from existing branch"
        );

        // The worktree should have the CC changes
        let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
            .await
            .unwrap();
        assert_eq!(
            content, "cc changes",
            "Resumed worktree should contain previous CC changes"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// When a worktree already exists for the resume branch (e.g. left over from
    /// a previous engine session), `parse_worktree_list` should detect it so the
    /// caller can reuse the existing worktree instead of failing with
    /// "branch is already used by worktree at ...".
    #[tokio::test]
    async fn resume_reuses_existing_worktree_for_branch() {
        use crate::engine::git_ops::{git_cmd, parse_worktree_list};

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo with an initial commit
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Create a branch and a worktree for it (simulating a previous CC session)
        let branch_name = "claude-code/20260326-leftover";
        let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
        let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
        let _ = git_cmd(&["checkout", "main"], repo).await;

        let wt_path = tmp.path().join("old-worktree");
        let result = git_cmd(
            &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Setup: should create initial worktree"
        );

        // Now verify that parse_worktree_list detects the existing worktree
        let list_output = git_cmd(&["worktree", "list", "--porcelain"], repo)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&list_output.stdout);
        let map = parse_worktree_list(&stdout);
        let found = map.get(branch_name);
        assert!(
            found.is_some(),
            "parse_worktree_list should find the existing worktree for branch {}",
            branch_name
        );

        // Verify it points to the correct path
        let found_path = found.unwrap();
        let canonical_expected = wt_path.canonicalize().unwrap();
        let canonical_found = found_path.canonicalize().unwrap();
        assert_eq!(
            canonical_found,
            canonical_expected,
            "Existing worktree path should match: expected {}, got {}",
            canonical_expected.display(),
            canonical_found.display()
        );

        // Trying to create a SECOND worktree for the same branch should fail
        let wt_path_new = tmp.path().join("new-worktree");
        let fail_result = git_cmd(
            &[
                "worktree",
                "add",
                wt_path_new.to_str().unwrap(),
                branch_name,
            ],
            repo,
        )
        .await;
        assert!(
            !fail_result.unwrap().status.success(),
            "git worktree add should fail when branch already checked out in another worktree"
        );

        // Verify the original worktree content is still intact
        let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
            .await
            .unwrap();
        assert_eq!(
            content, "cc changes",
            "Existing worktree should preserve CC changes"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// When the resume branch no longer exists, a fresh branch should be created.
    #[tokio::test]
    async fn resume_falls_back_when_branch_deleted() {
        use crate::engine::git_ops::git_cmd;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Verify a non-existent branch
        let branch_name = "claude-code/20260326-deleted";
        let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(!exists, "Deleted branch should not exist");

        // The code should fall back to creating a new branch
        let new_branch = crate::engine::agent_session::generate_cc_branch_name();
        let wt_path = tmp.path().join("worktree-fresh");
        let result = git_cmd(
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                &new_branch,
            ],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Should create worktree with fresh branch"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// Test helper: create a `AgentSession` with sensible defaults.
    /// Only `msg_tx` and `is_waiting` vary across tests.
    fn make_test_session(
        msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
        is_waiting: bool,
    ) -> crate::engine::AgentSession {
        use std::sync::Arc;
        crate::engine::AgentSession {
            msg_tx,
            is_waiting,
            has_changes: false,
            requires_restart: false,
            auto_apply: false,
            discard: false,
            cancel: Arc::new(tokio::sync::Notify::new()),
            interrupt: Arc::new(tokio::sync::Notify::new()),
            idle_notify: Arc::new(tokio::sync::Notify::new()),
            apply_now_in_progress: false,
            process_exited: false,
            worktree_path: None,
            branch_name: None,
            repo_root: None,
            cc_session_id: None,
            shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            external_terminal_emitted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            control_tx: tokio::sync::mpsc::unbounded_channel().0,
            builtin_commands: vec![],
            skill_commands: vec![],
            current_model: None,
            current_reasoning_effort: None,
            last_event_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            pending_followups: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    /// Validates that when a CC session is idle (is_waiting=true), a follow-up
    /// message is routed via msg_tx instead of being rejected with
    /// "Claude Code is already running".
    #[tokio::test]
    async fn idle_session_routes_followup_via_msg_tx() {
        use crate::engine::AgentUserInput;

        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, true);

        // Simulate single-lock routing logic from run_direct_agent
        assert!(!session.process_exited);
        assert!(session.is_waiting);

        // Route via msg_tx (same as production code)
        assert!(
            session
                .msg_tx
                .send(AgentUserInput {
                    text: "Follow-up message".to_string(),
                    images: None,
                    origin_event_id: None,
                })
                .is_ok(),
            "send should succeed when receiver is alive"
        );

        let received = msg_rx
            .try_recv()
            .expect("msg_rx should have received the follow-up");
        assert_eq!(received.text, "Follow-up message");
    }

    /// Validates that when a CC session is actively working (is_waiting=false),
    /// the follow-up is rejected (not routed via msg_tx).
    #[tokio::test]
    async fn busy_session_rejects_followup() {
        use crate::engine::AgentUserInput;

        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, false);

        assert!(!session.process_exited);
        assert!(
            !session.is_waiting,
            "Busy session should reject follow-up with 'already running' error"
        );
    }

    /// Validates that when the msg_tx channel is closed (receiver dropped),
    /// send returns Err — production code should propagate the error.
    #[tokio::test]
    async fn closed_channel_returns_error() {
        use crate::engine::AgentUserInput;

        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, true);

        // Drop the receiver to simulate CC process exit
        drop(msg_rx);

        let result = session.msg_tx.send(AgentUserInput {
            text: "too late".to_string(),
            images: None,
            origin_event_id: None,
        });
        assert!(result.is_err(), "send should fail when receiver is dropped");
    }

    /// Idle CC session (waiting for next prompt, process alive) must NOT be
    /// reported as in-flight. Regression: `abort_in_flight_for_restart` used
    /// to filter only on `!process_exited`, so every idle CC session got a
    /// `ResponseAborted` on `/api/restart` — rendering as "Response Interrupted"
    /// with a Continue button on every restart.
    #[tokio::test]
    async fn is_in_flight_false_for_idle_session() {
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let session = make_test_session(msg_tx, true);
        assert!(!session.is_in_flight());
    }

    #[tokio::test]
    async fn is_in_flight_true_for_busy_session() {
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let session = make_test_session(msg_tx, false);
        assert!(session.is_in_flight());
    }

    #[tokio::test]
    async fn is_in_flight_false_for_exited_session() {
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = make_test_session(msg_tx, false);
        session.process_exited = true;
        assert!(!session.is_in_flight());
    }

    #[test]
    fn is_engine_injected_path_matches_excluded_paths_only() {
        assert!(super::is_engine_injected_path(".lucidos-workspace"));
        assert!(super::is_engine_injected_path(".lucidos/bin/lucidos"));
        assert!(super::is_engine_injected_path(".lucidos/"));
        assert!(super::is_engine_injected_path(".lucidos"));
        assert!(super::is_engine_injected_path(
            ".claude/skills/lucidos-cli/SKILL.md"
        ));
        assert!(super::is_engine_injected_path(".claude/skills/lucidos-cli/"));
        assert!(super::is_engine_injected_path(".claude/skills/lucidos-cli"));

        // Sibling paths must NOT match — false positives would hide unrelated
        // user files (e.g. a user-named `.lucidos-workspace-archive` or a
        // `.claude/skills/lucidos-cli-helper/` skill).
        assert!(!super::is_engine_injected_path(".lucidos-workspace-archive"));
        assert!(!super::is_engine_injected_path(".lucidosX/bin"));
        assert!(!super::is_engine_injected_path(
            ".claude/skills/lucidos-cli-helper/SKILL.md"
        ));
        assert!(!super::is_engine_injected_path(".claude/skills/bugfix/SKILL.md"));
        assert!(!super::is_engine_injected_path(".claude/CLAUDE.md"));
        assert!(!super::is_engine_injected_path("src/main.rs"));
    }
}
