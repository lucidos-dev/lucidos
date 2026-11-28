//! A thread's coding-agent CLI with its profile, as the agent spawn builds it.
//!
//! INVARIANT: One resolver serves the agent's helpers and the thread terminal
//! (ADR 0820 decision 3). A login run in the terminal must land in the same
//! profile folder the agent's next turn reads.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::agent_sandbox::SandboxedCommand;
use super::{CodingAgent, SpawnArgs};

/// The thread's CLI executable with its working directory, sandbox and
/// account environment, and no arguments yet.
pub fn agent_command(agent: CodingAgent, args: &SpawnArgs<'_>) -> io::Result<SandboxedCommand> {
    match agent {
        CodingAgent::ClaudeCode => super::claude_code::auth_command(args),
        CodingAgent::Codex => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let binary = super::codex::resolve_codex_binary(
                home.as_deref(),
                args.binary_override.map(Path::new),
            );
            let (env, removed) = super::codex::baked_lucidos_env(
                args,
                super::lucidos_cli::lucidos_cli_dir(),
                &args.sandbox,
            );
            let mut cmd = args.sandbox.command(binary)?;
            cmd.envs(env).current_dir(args.worktree_path);
            for key in removed {
                cmd.env_remove(key);
            }
            Ok(cmd)
        }
    }
}

/// The profile folders the agent's CLI resolves for this thread, as
/// environment pairs. Folder paths only, never a credential.
pub fn profile_env(
    agent: CodingAgent,
    args: &SpawnArgs<'_>,
) -> io::Result<Vec<(OsString, OsString)>> {
    let cmd = agent_command(agent, args)?;
    Ok(profile_env_of(agent, &cmd))
}

fn profile_env_of(agent: CodingAgent, cmd: &SandboxedCommand) -> Vec<(OsString, OsString)> {
    match agent {
        CodingAgent::ClaudeCode => final_env(cmd, "CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(|dir| vec![("CLAUDE_CONFIG_DIR".into(), dir)])
            .unwrap_or_default(),
        CodingAgent::Codex => vec![("CODEX_HOME".into(), codex_home(cmd).into_os_string())],
    }
}

/// The program the agent's CLI runs, by absolute path when resolved.
pub fn program(cmd: &SandboxedCommand) -> PathBuf {
    PathBuf::from(cmd.as_std().get_program())
}

/// The final environment is the authority for Codex's profile, including
/// an explicit workspace CODEX_HOME.
pub fn codex_home(cmd: &SandboxedCommand) -> PathBuf {
    final_env(cmd, "CODEX_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(
            || PathBuf::from(final_env(cmd, "HOME").unwrap_or_default()).join(".codex"),
            PathBuf::from,
        )
}

/// A variable as the spawned child would see it: the command's own value,
/// or the engine's when the command leaves it alone.
fn final_env(cmd: &SandboxedCommand, name: &str) -> Option<OsString> {
    match cmd.as_std().get_envs().find(|(key, _)| *key == name) {
        Some((_, value)) => value.map(ToOwned::to_owned),
        None => std::env::var_os(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent_sandbox::Sandbox;

    fn command_with(env: &[(&str, Option<&str>)]) -> SandboxedCommand {
        let mut cmd = Sandbox::inherited().command("/bin/true").unwrap();
        for (key, value) in env {
            match value {
                Some(value) => {
                    cmd.env(key, value);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        cmd
    }

    #[test]
    fn a_pinned_claude_config_dir_is_exported() {
        let cmd = command_with(&[("CLAUDE_CONFIG_DIR", Some("/home/user/.claude-work"))]);
        assert_eq!(
            profile_env_of(CodingAgent::ClaudeCode, &cmd),
            vec![("CLAUDE_CONFIG_DIR".into(), "/home/user/.claude-work".into())]
        );
    }

    #[test]
    fn a_removed_claude_config_dir_exports_nothing() {
        let cmd = command_with(&[("CLAUDE_CONFIG_DIR", None)]);
        assert!(profile_env_of(CodingAgent::ClaudeCode, &cmd).is_empty());
    }

    #[test]
    fn codex_home_follows_the_command_then_home() {
        let explicit = command_with(&[
            ("CODEX_HOME", Some("/srv/codex")),
            ("HOME", Some("/home/user")),
        ]);
        assert_eq!(
            profile_env_of(CodingAgent::Codex, &explicit),
            vec![("CODEX_HOME".into(), "/srv/codex".into())]
        );
        let defaulted = command_with(&[("CODEX_HOME", Some("")), ("HOME", Some("/home/user"))]);
        assert_eq!(
            profile_env_of(CodingAgent::Codex, &defaulted),
            vec![("CODEX_HOME".into(), "/home/user/.codex".into())]
        );
    }
}
