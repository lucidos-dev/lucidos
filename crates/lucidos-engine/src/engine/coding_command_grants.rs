use crate::engine::command_guard::TrustedHeads;
use crate::engine::thread_events::AllowScope;

pub(crate) const BROAD_ALLOW_INEFFECTIVE: &[&str] =
    &["Edit", "ExitPlanMode", "NotebookEdit", "Write"];

/// INVARIANT: Directories no head grant may write under, only the bare `Bash` line:
/// Claude Code's own settings and hooks, and git metadata, whose hooks and
/// config run on later git calls. Read through [`input_touches_protected_path`].
pub(crate) const CC_PROTECTED_PATH_MARKERS: &[&str] = &[".claude/", ".git/"];

pub(crate) const SESSION_PATH_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit"];

pub(crate) const CODEX_BACKEND_TOOLS: &[&str] = &[CODEX_COMMAND_TOOL, "file_change"];

pub(crate) const CODEX_COMMAND_TOOL: &str = "command_execution";

pub(crate) const CLAUDE_CODE_COMMAND_TOOL: &str = "Bash";

pub fn derive_allow_pattern(
    tool_name: &str,
    input: &serde_json::Value,
    scope: AllowScope,
    trusted: &TrustedHeads,
) -> Option<String> {
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
            // INVARIANT: Codex raises `file_change` approval for a patch outside its
            // sandbox. Each approval gets its own card. A patch can span
            // several files, and a bare-name fallback would approve every later
            // out-of-sandbox patch.
            if tool_name == "file_change" {
                return None;
            }
            // INVARIANT: A command whose head does not resolve gets NO session pattern.
            // The bare-name fallback below would read one click on an
            // unreadable command as a grant for every command in the thread.
            // That is the shape `file_change` above is denied.
            if matches!(tool_name, "Bash" | "command_execution") {
                return narrow_subscope(tool_name, input, trusted);
            }
            if let Some(narrow) = narrow_subscope(tool_name, input, trusted) {
                return Some(narrow);
            }
            Some(tool_name.to_string())
        }
    }
}

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
        // INVARIANT: Codex's command approvals carry a `command` field too, and get the
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

/// INVARIANT: The head a command grant stores, through [`command_guard::grant_head`]. A
/// trusted repo script in `trusted` is named by its worktree-relative path.
/// Read as text, as the session and persisted coverage checks read it.
///
/// [`command_guard::grant_head`]: crate::engine::command_guard::grant_head
pub fn command_grant_head(input: &serde_json::Value, trusted: &TrustedHeads) -> Option<String> {
    use crate::engine::command_guard::{grant_head, CommandSite};
    let command = input.get("command").and_then(|v| v.as_str())?;
    grant_head(command, CommandSite::TextOnly { root: None }, trusted)
}
