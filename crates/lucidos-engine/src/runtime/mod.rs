pub mod agent_runtime;
pub mod browser;
mod browser_consent;
pub mod claude_code;
pub mod codex;
mod codex_app_server;
mod codex_app_server_parse;
mod codex_parse;
pub mod lucidos_cli;
pub mod python;
mod spawn_env;

pub use agent_runtime::{
    AgentEvent, AgentInput, AgentPermissionRequest, AgentRuntime, CodingAgent, ControlRequest,
    RunningAgent, SpawnArgs,
};
pub use browser::{BrowserLogins, BrowserRuntime, HeadlessBlocklist};
pub use claude_code::ClaudeCodeRuntime;
pub use codex::{CodexRuntime, CODEX_ASK_USER_QUESTION_TOOL};
pub use python::PythonRuntime;

/// Whether the engine is running inside a packaged build (the macOS `.app` /
/// headless tarball / `install.sh` service all set `LUCIDOS_PACKAGED=1` — see
/// `desktop.rs::spawn_gateway` and `service.sh::service_runtime_env_pairs`).
/// Dev / e2e leave it unset. Used to fail fast on a missing bundled runtime
/// dependency in packaged while keeping dev's tolerant degrade-with-log path.
pub(crate) fn is_packaged() -> bool {
    std::env::var_os("LUCIDOS_PACKAGED").is_some_and(|v| v == "1")
}

/// Effective resolution of a coding agent's CLI binary — the payload of
/// `GET /api/v1/coding-agents/binaries` (Settings → System → Coding agents). Live
/// detection, recomputed per request: only an explicit user override is ever
/// persisted (the `coding_agent_*_path` preference), so a Homebrew upgrade or
/// installer move self-heals instead of leaving a stale stored path.
#[derive(Debug, serde::Serialize)]
pub struct AgentBinaryStatus {
    /// Resolved absolute path when one was found (the override, a probe hit,
    /// or a PATH hit); `None` when nothing resolves.
    pub path: Option<String>,
    /// Where the resolution came from: `override` (user preference),
    /// `detected` (probe list), `path` (bare PATH lookup), `not-found`.
    pub source: &'static str,
    /// `false` when the override is set but doesn't point at an executable —
    /// `error` then carries the message a spawn would fail with.
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Compute the effective CLI binary resolution for `agent`, mirroring exactly
/// what a spawn would do: a set override wins (validated the same way the
/// spawn validates it), else the probe list, else a bare-name PATH walk.
pub fn detect_agent_binary(agent: CodingAgent, override_path: Option<&str>) -> AgentBinaryStatus {
    if let Some(raw) = override_path {
        let (label, pref_key) = match agent {
            CodingAgent::ClaudeCode => {
                ("Claude Code (`claude`)", crate::core::PREF_CODING_AGENT_CLAUDE_PATH)
            }
            CodingAgent::Codex => ("Codex (`codex`)", crate::core::PREF_CODING_AGENT_CODEX_PATH),
        };
        return match spawn_env::resolve_binary_override(raw, label, pref_key) {
            Ok(p) => AgentBinaryStatus {
                path: Some(p.display().to_string()),
                source: "override",
                valid: true,
                error: None,
            },
            Err(e) => AgentBinaryStatus {
                path: Some(raw.to_string()),
                source: "override",
                valid: false,
                error: Some(e.to_string()),
            },
        };
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let resolved = match agent {
        CodingAgent::ClaudeCode => claude_code::resolve_claude_binary(home.as_deref(), None),
        CodingAgent::Codex => codex::resolve_codex_binary(home.as_deref(), None),
    };
    if std::path::Path::new(&resolved).is_absolute() {
        return AgentBinaryStatus {
            path: Some(resolved.to_string_lossy().into_owned()),
            source: "detected",
            valid: true,
            error: None,
        };
    }
    // Bare name — emulate `Command::spawn`'s PATH walk so the UI shows what
    // would actually run (or that nothing would).
    let path_hit = spawn_env::find_on_path(&resolved, std::env::var_os("PATH").as_deref());
    match path_hit {
        Some(found) => AgentBinaryStatus {
            path: Some(found.display().to_string()),
            source: "path",
            valid: true,
            error: None,
        },
        None => AgentBinaryStatus {
            path: None,
            source: "not-found",
            valid: false,
            error: None,
        },
    }
}

