use super::LucidosEngine;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::types::CcCommandsInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

mod control;
mod merge_session;
mod spawn;


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
// intercepts before CC's gate. See `input_touches_protected_path` for the
// related per-input filter (Bash commands targeting `.claude/` / `.git/`
// paths, which empirically still surface a card even with bare `Bash` in
// `--allowedTools`).
const BROAD_ALLOW_INEFFECTIVE: &[&str] = &["Edit", "ExitPlanMode", "NotebookEdit", "Write"];

/// Substrings that mark a path CC treats specially for destructive Bash
/// commands. Empirically (probed 2026-05-16): `Bash rm -rf .../.claude/...`
/// surfaces a permission card even when bare `Bash` is in `--allowedTools`,
/// so persisting a Broad (`Bash`) or Narrow (`Bash(rm:*)`) grant from that
/// card lies about future suppression — the next `Bash rm -rf` on the same
/// path will prompt again. We probed `.git/` only by analogy with the
/// long-standing comment chain; CC's full rule set isn't publicly
/// documented. Read/Edit/cat on the same paths auto-approved silently
/// under bare allowlist entries — the trigger is the tool+path combination,
/// not the path alone. Session scope is unaffected because the engine
/// intercepts the MCP permission server call before CC's gate fires.
const CC_PROTECTED_PATH_MARKERS: &[&str] = &[".claude/", ".git/"];

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
///     reaches CC as `--allowedTools` on the next spawn. Returns `None`
///     (suppressing persistence and hiding the button on the UI) when
///     either:
///       1. The tool is in `BROAD_ALLOW_INEFFECTIVE` (Edit / Write /
///          NotebookEdit / ExitPlanMode) — CC always routes those through
///          the permission prompt regardless of `--allowedTools`, so the
///          bare entry never suppresses anything. Applies to `Broad` only
///          (the Narrow scope already returns `None` for these tools).
///       2. The tool is `Bash` and the command references `.claude/` or
///          `.git/` (per `input_touches_protected_path`) — CC keeps
///          surfacing the card for those Bash commands even with bare
///          `Bash` in `--allowedTools`, so persisting `Bash` or
///          `Bash(<token>:*)` would lie about future suppression for that
///          exact path. Applies to both `Broad` and `Narrow`.
///
///     Otherwise, `Broad` returns `Some(tool_name)` and `Narrow` returns
///     `Some(sub_scope)` where the sub-scope exists:
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
            if input_touches_protected_path(tool_name, input) {
                return None;
            }
            Some(tool_name.to_string())
        }
        AllowScope::Narrow => {
            if input_touches_protected_path(tool_name, input) {
                return None;
            }
            narrow_subscope(tool_name, input)
        }
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

/// True when a `Bash` command references a path CC keeps under special
/// permission routing (`.claude/` or `.git/`). The empirically verified
/// trigger is `Bash rm -rf` targeting those paths — that card fires even
/// when bare `Bash` is in `--allowedTools`, so persisting a Broad
/// (`Bash`) or Narrow (`Bash(rm:*)`) grant from it lies about future
/// suppression. We scan the entire command string rather than restricting
/// to known destructive verbs because (a) CC's full rule set isn't
/// publicly documented, and (b) the false-positive cost is only "hide a
/// button on a card that wouldn't have appeared anyway." Session scope is
/// unaffected: the engine intercepts before CC's gate.
///
/// Restricted to `Bash` because that's the only tool we've empirically
/// observed surfacing the card on these paths under the user's current
/// bare-allowlist setup — see `CC_PROTECTED_PATH_MARKERS` for the probing
/// summary. If CC tightens later, widen this match arm; do not pre-add
/// branches that the UI would never reach in practice.
pub(crate) fn input_touches_protected_path(tool_name: &str, input: &serde_json::Value) -> bool {
    if tool_name != "Bash" {
        return false;
    }
    input
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| CC_PROTECTED_PATH_MARKERS.iter().any(|m| s.contains(m)))
        .unwrap_or(false)
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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(CC_ALLOWED_TOOLS_HEADER.to_string())
        }
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

/// A canceled Claude Code subprocess returns `Ok(_)`, so the inner future succeeding
/// is not enough to prove `/harden` ran. The marker is the only signal that
/// survives a kill — it's only written by the skill's final phase.
pub(crate) fn hardening_succeeded(cancel_token_cancelled: bool, marker_present: bool) -> bool {
    !cancel_token_cancelled && marker_present
}

/// True when `spawn_hardening_session`'s post-CC code should bail without
/// emitting any `ChangeApplyFailed` — because the change was applied via a
/// concurrent path (e.g. another device clicked Apply) while CC was running
/// `/harden`. In that case the marker file has been consumed by the apply and
/// the change row is no longer `"pending"`, so `branch_is_hardened` returns
/// false and we'd otherwise emit a "Hardening did not complete" failure ~15s
/// after the user already saw `ChangeApplied` — a "hindsight failure" in the
/// UI. `change_status` is the `status` field from the change row, or `None`
/// when the row could not be fetched (treat as not-yet-applied so the
/// existing failure path still runs).
pub(crate) fn change_applied_concurrently(change_status: Option<&str>) -> bool {
    matches!(change_status, Some("applied"))
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
///
/// A `**/` prefix means "match this path at any depth in the worktree" — see
/// the lucidos-cli skill entry, which lands at `<wt>/.claude/skills/...` for
/// repo-rooted spawns but at `<wt>/data/apps/<id>/.claude/skills/...` for app
/// coding-agent threads (CC's cwd is the deep app folder, and
/// `install_lucidos_cli_skill` writes relative to cwd). One pattern covers
/// both — no separate code path per spawn kind.
pub(crate) const WORKTREE_EXCLUDE_PATHS: &[&str] = &[
    WORKTREE_WORKSPACE_MARKER,
    "**/.claude/skills/lucidos-cli/",
    RUNTIME_PATH_PREFIX,
];

/// True for paths the engine injects into every CC worktree (see
/// `WORKTREE_EXCLUDE_PATHS`). Trailing-`/` entries match by directory prefix;
/// other entries match exactly so `.lucidos-workspace-archive` doesn't
/// false-positive against `.lucidos-workspace`. A leading `**/` makes the
/// entry match at any depth in the worktree, mirroring gitignore semantics
/// — needed because app coding-agent threads write engine-injected files
/// under `data/apps/<id>/`, not at the worktree root.
pub(crate) fn is_engine_injected_path(path: &str) -> bool {
    WORKTREE_EXCLUDE_PATHS
        .iter()
        .any(|entry| entry_matches_path(entry, path))
}

fn entry_matches_path(entry: &str, path: &str) -> bool {
    let (anywhere, body) = match entry.strip_prefix("**/") {
        Some(rest) => (true, rest),
        None => (false, entry),
    };
    let dir_form = body.strip_suffix('/');
    let trimmed = dir_form.unwrap_or(body);

    // Root match — same semantics as before for non-`**/` entries.
    let root_match = if dir_form.is_some() {
        path == trimmed || path.starts_with(body)
    } else {
        path == trimmed
    };
    if root_match {
        return true;
    }

    // Anywhere-deeper match — only for `**/`-prefixed entries.
    if anywhere {
        let nested_dir = format!("/{}", trimmed);
        let nested_prefix = format!("/{}/", trimmed);
        return path.ends_with(&nested_dir) || path.contains(&nested_prefix);
    }

    false
}

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
    /// When `Some`, spawn an **app coding-agent thread** instead of a
    /// Lucidos / external-repo one: sparse-checkout worktree of the
    /// workspace git narrowed to `data/apps/<id>/`. `repo_id` MUST be
    /// `None` when this is set — apps are not in the repo registry.
    pub app_id: Option<String>,
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
    Ok(status.as_deref() == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
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

/// Returned by [`LucidosEngine::interrupt_agent`] when the session is already
/// idle/waiting — i.e. a Cancel (Stop) click that races a turn which already
/// finished. Callers (the `/claude-code/stop` handler) treat it as a no-op
/// (HTTP 200), not an error: there is nothing to interrupt. Lives here (not in
/// the private `control` submodule) so the API handler can reference it.
pub(crate) const SESSION_ALREADY_WAITING: &str = "Session is already waiting";

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

#[cfg(test)]
#[path = "../claude_code_tests/cache_status.rs"]
mod cache_status_tests;

#[cfg(test)]
#[path = "../claude_code_tests/settle.rs"]
mod settle_tests;

#[cfg(test)]
#[path = "../claude_code_tests/allowed_tools.rs"]
mod allowed_tools_tests;

#[cfg(test)]
#[path = "../claude_code_tests/session.rs"]
mod session_tests;
