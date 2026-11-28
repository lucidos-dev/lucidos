//! Coding-agent sign-in as a terminal suggestion (ADR 0820): the CLI's own
//! login command, for the user to run in the thread terminal.

use super::CodingAgent;
use crate::engine::thread_events::AgentSignInTarget;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why a sign-in suggestion is posted. The label tells the user (ADR 0823).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInReason {
    /// The agent called a tool on the server.
    ToolCall,
    /// The status report marks the server signed out. Codex only: it hides
    /// such a server's tools, so no tool call can happen.
    Reported,
    /// The user asked, with `/mcp` or `/login`, or a turn failed on login.
    Requested,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SignInSuggestion {
    pub command: String,
    pub label: String,
}

/// The suggestion for `target`, or `None` when the agent has no such sign-in
/// or the server name could not be typed as one argument. A `gateway` model
/// login opens Claude Code itself, where `/login` reviews the gateway.
pub fn suggestion(
    agent: CodingAgent,
    target: &AgentSignInTarget,
    reason: SignInReason,
    gateway: bool,
    binary_override: Option<&str>,
) -> Option<SignInSuggestion> {
    let binary = agent_binary(agent, binary_override).into_string().ok()?;
    let binary = shell_quote(&binary);
    match target {
        AgentSignInTarget::McpServer { name } if valid_server_name(name) => {
            Some(SignInSuggestion {
                command: format!("{binary} mcp login {} --no-browser", shell_quote(name)),
                label: mcp_label(name, reason),
            })
        }
        AgentSignInTarget::ModelLogin if agent == CodingAgent::ClaudeCode && gateway => {
            Some(SignInSuggestion {
                command: binary,
                label: "Open Claude Code, then run /login to review and trust the gateway"
                    .to_string(),
            })
        }
        AgentSignInTarget::ModelLogin if agent == CodingAgent::ClaudeCode => {
            Some(SignInSuggestion {
                command: format!("env BROWSER=false {binary} auth login"),
                label: "Sign in to Claude Code".to_string(),
            })
        }
        _ => None,
    }
}

/// A name too long for a suggestion label leaves it out: the command still
/// names the server.
fn mcp_label(name: &str, reason: SignInReason) -> String {
    let (named, unnamed) = match reason {
        SignInReason::ToolCall => (
            format!("A tool call needs the {name} MCP server. Sign in to it"),
            "A tool call needs this MCP server. Sign in to it",
        ),
        SignInReason::Reported | SignInReason::Requested => (
            format!("Sign in to the {name} MCP server"),
            "Sign in to this MCP server",
        ),
    };
    if named.chars().count() <= crate::engine::terminal::suggestion::MAX_LABEL_CHARS {
        named
    } else {
        unnamed.to_string()
    }
}

/// Whether `tool` is a call on the MCP server `server`. Both CLIs name an MCP
/// tool `mcp__<server>__<tool>`; Claude Code first turns every character
/// outside `[A-Za-z0-9_-]` into `_`, so both names go through that.
pub fn calls_mcp_server(tool: &str, server: &str) -> bool {
    let normalise = |name: &str| -> String {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let Some(rest) = tool.strip_prefix("mcp__") else {
        return false;
    };
    let rest = normalise(rest);
    let prefix = format!("{}__", normalise(server));
    rest.len() > prefix.len() && rest.starts_with(&prefix)
}

/// The CLI the agent spawn runs, by absolute path when one is installed.
fn agent_binary(agent: CodingAgent, binary_override: Option<&str>) -> OsString {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let override_path = binary_override.map(Path::new);
    match agent {
        CodingAgent::ClaudeCode => {
            super::claude_code::resolve_claude_binary(home.as_deref(), override_path)
        }
        CodingAgent::Codex => super::codex::resolve_codex_binary(home.as_deref(), override_path),
    }
}

fn valid_server_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.starts_with('-') && !name.chars().any(char::is_control)
}

/// One POSIX shell word. Single quotes also read literally in fish.
fn shell_quote(word: &str) -> String {
    let bare = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-./:=@%+,".contains(&b));
    if bare {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(name: &str) -> AgentSignInTarget {
        AgentSignInTarget::McpServer { name: name.into() }
    }

    #[test]
    fn an_mcp_sign_in_runs_the_resolved_cli_with_no_browser() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            let suggestion = suggestion(
                agent,
                &mcp("example server"),
                SignInReason::Requested,
                false,
                Some("/opt/agent cli/bin"),
            )
            .unwrap();
            assert_eq!(
                suggestion.command,
                "'/opt/agent cli/bin' mcp login 'example server' --no-browser"
            );
            assert_eq!(suggestion.label, "Sign in to the example server MCP server");
        }
    }

    #[test]
    fn a_long_server_name_still_yields_a_valid_label() {
        let name = "s".repeat(200);
        let built = suggestion(
            CodingAgent::ClaudeCode,
            &mcp(&name),
            SignInReason::Requested,
            false,
            None,
        )
        .unwrap();
        assert!(crate::engine::terminal::suggestion::validate_label(&built.label).is_ok());
        assert!(built.command.contains(&name));
    }

    #[test]
    fn a_model_login_keeps_the_browser_closed() {
        let login = suggestion(
            CodingAgent::ClaudeCode,
            &AgentSignInTarget::ModelLogin,
            SignInReason::Requested,
            false,
            Some("/home/user/.local/bin/claude"),
        )
        .unwrap();
        assert_eq!(
            login.command,
            "env BROWSER=false /home/user/.local/bin/claude auth login"
        );
        let gateway = suggestion(
            CodingAgent::ClaudeCode,
            &AgentSignInTarget::ModelLogin,
            SignInReason::Requested,
            true,
            Some("/home/user/.local/bin/claude"),
        )
        .unwrap();
        assert_eq!(gateway.command, "/home/user/.local/bin/claude");
        assert!(suggestion(
            CodingAgent::Codex,
            &AgentSignInTarget::ModelLogin,
            SignInReason::Requested,
            false,
            None
        )
        .is_none());
    }

    #[test]
    fn a_server_name_stays_one_inert_argument() {
        let quoted = suggestion(
            CodingAgent::ClaudeCode,
            &mcp("it's $(x)"),
            SignInReason::Requested,
            false,
            Some("/bin/claude"),
        )
        .unwrap();
        assert_eq!(
            quoted.command,
            r"/bin/claude mcp login 'it'\''s $(x)' --no-browser"
        );
        for refused in ["", "  ", "--help", "a\nb"] {
            assert!(
                suggestion(
                    CodingAgent::ClaudeCode,
                    &mcp(refused),
                    SignInReason::Requested,
                    false,
                    None
                )
                .is_none(),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_tool_call_label_says_why() {
        let built = suggestion(
            CodingAgent::ClaudeCode,
            &mcp("example"),
            SignInReason::ToolCall,
            false,
            Some("/bin/claude"),
        )
        .unwrap();
        assert_eq!(built.command, "/bin/claude mcp login example --no-browser");
        assert_eq!(
            built.label,
            "A tool call needs the example MCP server. Sign in to it"
        );
        let long = suggestion(
            CodingAgent::Codex,
            &mcp(&"s".repeat(200)),
            SignInReason::ToolCall,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            long.label,
            "A tool call needs this MCP server. Sign in to it"
        );
        assert!(crate::engine::terminal::suggestion::validate_label(&long.label).is_ok());
    }

    #[test]
    fn a_tool_matches_its_server_by_the_cli_prefix() {
        assert!(calls_mcp_server("mcp__calendar__list_events", "calendar"));
        assert!(calls_mcp_server(
            "mcp__vend-mcp-gateway__authenticate",
            "vend-mcp-gateway"
        ));
        assert!(calls_mcp_server("mcp__my_server__search", "my server"));
        assert!(calls_mcp_server("mcp__caf___search", "café"));
        for (tool, server) in [
            ("mcp__calendar__list", "cal"),
            ("mcp__calendar_two__list", "calendar"),
            ("mcp__calendar__", "calendar"),
            ("Bash", "calendar"),
            ("calendar__list", "calendar"),
        ] {
            assert!(!calls_mcp_server(tool, server), "{tool} on {server}");
        }
    }

    #[test]
    fn a_suggestion_passes_terminal_validation() {
        let suggestion = suggestion(
            CodingAgent::ClaudeCode,
            &mcp("example"),
            SignInReason::Requested,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            crate::engine::terminal::suggestion::validate(&suggestion.command),
            Ok(())
        );
        assert_eq!(
            crate::engine::terminal::suggestion::validate_label(&suggestion.label),
            Ok(())
        );
    }
}
