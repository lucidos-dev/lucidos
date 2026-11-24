//! Rules for deriving and matching coding-agent command grants.

use crate::engine::command_guard::TrustedHeads;
use crate::engine::thread_events::AllowScope;

// Tools whose bare `--allowedTools` entry Claude Code cannot honor.
// - Edit, Write and NotebookEdit still prompt for protected paths. Claude Code
//   already auto-approves the other in-worktree writes in acceptEdits mode.
// - ExitPlanMode always prompts for plan review.
//
// Cards offer a session grant for eligible requests that need in-thread
// persistence. See `input_touches_protected_path` for Bash writes to protected
// paths.
pub(crate) const BROAD_ALLOW_INEFFECTIVE: &[&str] =
    &["Edit", "ExitPlanMode", "NotebookEdit", "Write"];

/// Directories no head grant may write under, only the bare `Bash` line:
/// Claude Code's own settings and hooks, and git metadata, whose hooks and
/// config run on later git calls. Read through [`input_touches_protected_path`].
pub(crate) const CC_PROTECTED_PATH_MARKERS: &[&str] = &[".claude/", ".git/"];

// Tools whose `AllowScope::Session` pattern is per-file, derived from the
// input's `file_path` / `notebook_path` field rather than the tool name.
// Overlaps with but is not identical to `BROAD_ALLOW_INEFFECTIVE`: this set
// is "tools where remembering one path doesn't imply remembering all paths,"
// while `BROAD_ALLOW_INEFFECTIVE` is "tools whose bare allowlist entry is a
// lie." Edit/Write/NotebookEdit are in both; ExitPlanMode is only in the
// latter (no per-path identifier — its session pattern is the bare tool
// name).
pub(crate) const SESSION_PATH_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit"];

// Codex backend tool names, raised by the app-server approval bridge (see
// `runtime/codex_app_server_parse.rs::parse_approval_request`). No Codex
// driver reads `cc-allowed-tools`. A `command_execution` card persists Claude
// Code's own `Bash` / `Bash(<head>:*)` line instead, and the engine's gate
// honours it (`cc_permission::persisted_allow_covers`, ADR 0571). Session scope
// uses `command_execution(<head>:*)` (see `narrow_subscope`). `file_change`
// gets no persisted and no session pattern (see the Session arm of
// `derive_allow_pattern`).
pub(crate) const CODEX_BACKEND_TOOLS: &[&str] = &[CODEX_COMMAND_TOOL, "file_change"];

/// The Codex app-server's command-approval tool. A Codex card means the OS
/// sandbox blocked the command, which the user never chose.
pub(crate) const CODEX_COMMAND_TOOL: &str = "command_execution";

/// Claude Code's command tool. A `Bash` card means the tool is missing from a
/// `--allowedTools` list the user curates, so it reaches the attended command
/// gate only under `coding_agent_auto_answer`.
pub(crate) const CLAUDE_CODE_COMMAND_TOOL: &str = "Bash";

/// Derive the grant pattern for the selected scope.
/// - `Broad` and `Narrow` persist to Claude Code's next spawn. `Broad` records
///   the tool name and excludes `BROAD_ALLOW_INEFFECTIVE`. `Narrow` derives a
///   Skill namespace or Bash command pattern.
/// - `Session` matches an exact pattern in the current thread. Edit, Write and
///   NotebookEdit use paths. Bash uses [`command_grant_head`], Skill uses its
///   namespace and the other tools use their names.
///
/// Codex `command_execution` persists as Bash (ADR 0571). A protected-path
/// command cannot take a narrow or session grant. `file_change` takes no
/// derived grant.
///
/// `cc_permission::grant_offer` offers the pattern only if it covers its own
/// request.
pub fn derive_allow_pattern(
    tool_name: &str,
    input: &serde_json::Value,
    scope: AllowScope,
    trusted: &TrustedHeads,
) -> Option<String> {
    // A Codex command persists Claude Code's own line, under its protected-path
    // rule, so one line means the same whichever agent asks (ADR 0571).
    let persisted_tool = if tool_name == CODEX_COMMAND_TOOL {
        CLAUDE_CODE_COMMAND_TOOL
    } else {
        tool_name
    };
    match scope {
        AllowScope::Broad | AllowScope::Narrow if CODEX_BACKEND_TOOLS.contains(&persisted_tool) => {
            None
        }
        AllowScope::Narrow | AllowScope::Session
            if input_touches_protected_path(tool_name, input) =>
        {
            None
        }
        AllowScope::Broad if BROAD_ALLOW_INEFFECTIVE.contains(&persisted_tool) => None,
        AllowScope::Broad => Some(persisted_tool.to_string()),
        AllowScope::Narrow => narrow_subscope(persisted_tool, input, trusted),
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
            // Codex raises `file_change` approval for a patch outside its
            // sandbox. Each approval gets its own card. A patch can span
            // several files, and a bare-name fallback would approve every later
            // out-of-sandbox patch.
            if tool_name == "file_change" {
                return None;
            }
            // A command whose head does not resolve gets NO session pattern.
            // The bare-name fallback below would read one click on an
            // unreadable command as a grant for every command in the thread.
            // That is the shape `file_change` above is denied.
            if matches!(tool_name, "Bash" | "command_execution") {
                return narrow_subscope(tool_name, input, trusted);
            }
            if let Some(narrow) = narrow_subscope(tool_name, input, trusted) {
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

/// True when a `Bash` or Codex command may WRITE under `.claude/` or `.git/`
/// ([`command_guard::command_may_write_under`]). Only the bare `Bash` line
/// covers such a write. A head grant, persisted or session, never does: one
/// click on `cat` would otherwise let a redirect plant a git hook. A command
/// that only mentions or reads those paths is not touched (ADR 0634).
///
/// [`command_guard::command_may_write_under`]: crate::engine::command_guard::command_may_write_under
pub fn input_touches_protected_path(tool_name: &str, input: &serde_json::Value) -> bool {
    if !matches!(tool_name, CLAUDE_CODE_COMMAND_TOOL | CODEX_COMMAND_TOOL) {
        return false;
    }
    input
        .get("command")
        .and_then(|v| v.as_str())
        .is_some_and(|command| {
            crate::engine::command_guard::command_may_write_under(
                command,
                CC_PROTECTED_PATH_MARKERS,
            )
        })
}

/// Narrow `--allowedTools`-style sub-scope for tools whose input carries a
/// meaningful identifier. Returns `None` for tools without one — Narrow
/// callers treat that as "no narrow button"; Session callers fall back to
/// the bare tool name.
fn narrow_subscope(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
) -> Option<String> {
    match tool_name {
        "Skill" => {
            let skill = input.get("skill").and_then(|v| v.as_str())?;
            let plugin = skill.split_once(':').map(|(p, _)| p).unwrap_or(skill);
            if plugin.is_empty() {
                return None;
            }
            Some(format!("Skill({}:*)", plugin))
        }
        // Codex's command approvals carry a `command` field too, and get the
        // same head scoping as Bash: a grant on `git push` covers `git …` but
        // NOT a later `rm -rf …`. Codex's Narrow scope reaches this arm as
        // `Bash`, so it persists Claude Code's line.
        //
        // The head comes from the command guard, not from the first
        // whitespace token. Codex wraps EVERY command as
        // `/bin/zsh -lc '<script>'`, so the raw first token names the same
        // shell every time. One "Allow for this thread" click then blanket
        // approved every later Codex command.
        "Bash" | "command_execution" => {
            let head = command_grant_head(input, trusted)?;
            Some(format!("{}({}:*)", tool_name, head))
        }
        _ => None,
    }
}

/// The head a command grant stores, through [`command_guard::grant_head`]. A
/// trusted repo script in `trusted` is named by its worktree-relative path.
/// Read as text, as the session and persisted coverage checks read it.
///
/// [`command_guard::grant_head`]: crate::engine::command_guard::grant_head
pub fn command_grant_head(input: &serde_json::Value, trusted: &TrustedHeads) -> Option<String> {
    use crate::engine::command_guard::{grant_head, CommandSite};
    let command = input.get("command").and_then(|v| v.as_str())?;
    grant_head(command, CommandSite::TextOnly { root: None }, trusted)
}
