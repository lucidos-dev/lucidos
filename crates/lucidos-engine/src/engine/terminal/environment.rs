//! The environment a thread terminal's shell starts with (ADR 0356 decision
//! 5, amended by ADR 0820 decision 3).
//!
//! INVARIANT: The engine's own variables never cross. The shell gets a normal
//! login, Lucidos's tools, the user's own Settings variables and, in a
//! coding-agent thread, the agent's profile location. Nothing else.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use uuid::Uuid;

pub const TERMINAL_ID_VAR: &str = "LUCIDOS_TERMINAL_ID";

/// Variables a normal login keeps, passed through when the engine has them.
const PASSED: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// The profile variables a coding-agent thread's shell may carry. They name
/// folders, never credentials.
pub const PROFILE_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

pub struct Inputs<'a> {
    pub engine_env: &'a [(OsString, OsString)],
    pub user_env: &'a [(String, String)],
    pub profile: &'a [(OsString, OsString)],
    pub workspace: &'a Path,
    pub thread_id: Uuid,
    pub path_prefixes: &'a [&'a Path],
}

pub fn build(inputs: Inputs<'_>) -> Vec<(OsString, OsString)> {
    let engine: BTreeMap<&OsStr, &OsStr> = inputs
        .engine_env
        .iter()
        .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
        .collect();
    let mut env: BTreeMap<OsString, OsString> = BTreeMap::new();
    for name in PASSED {
        if let Some(value) = engine.get(OsStr::new(name)) {
            env.insert((*name).into(), (*value).to_owned());
        }
    }
    for (key, value) in inputs.engine_env {
        if key
            .to_str()
            .is_some_and(|key| key == "LANG" || key.starts_with("LC_"))
        {
            env.insert(key.clone(), value.clone());
        }
    }
    env.entry("LANG".into())
        .or_insert_with(|| default_locale().into());
    for (key, value) in inputs.user_env {
        if !reserved(key) {
            env.insert(key.into(), value.into());
        }
    }
    let mut path: Vec<std::path::PathBuf> = inputs
        .path_prefixes
        .iter()
        .map(|p| p.to_path_buf())
        .collect();
    if let Some(engine_path) = engine.get(OsStr::new("PATH")) {
        path.extend(std::env::split_paths(engine_path));
    }
    if let Ok(joined) = std::env::join_paths(path) {
        env.insert("PATH".into(), joined);
    }
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "Lucidos".into());
    env.insert(
        "LUCIDOS_WORKSPACE".into(),
        inputs.workspace.as_os_str().to_owned(),
    );
    env.insert(
        "LUCIDOS_THREAD_ID".into(),
        inputs.thread_id.to_string().into(),
    );
    for (key, value) in inputs.profile {
        if key.to_str().is_some_and(|key| PROFILE_VARS.contains(&key)) {
            env.insert(key.clone(), value.clone());
        }
    }
    env.into_iter().collect()
}

/// The environment for one thread's shell, from this engine's own process.
/// `profile` is the coding agent's folders, empty for a chat thread.
pub fn for_thread(
    workspace: &Path,
    thread_id: Uuid,
    user_env: &[(String, String)],
    profile: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let engine_env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let prefixes = crate::runtime::spawn_env::agent_path_prefixes(
        crate::runtime::lucidos_cli::lucidos_cli_dir(),
        None,
        std::env::var_os("LUCIDOS_PG_BIN_DIR").map(std::path::PathBuf::from),
    );
    let prefixes: Vec<&Path> = prefixes.iter().map(std::path::PathBuf::as_path).collect();
    build(Inputs {
        engine_env: &engine_env,
        user_env,
        profile,
        workspace,
        thread_id,
        path_prefixes: &prefixes,
    })
}

/// Names the engine owns. A Settings variable with one of them never wins.
fn reserved(name: &str) -> bool {
    name.starts_with("LUCIDOS_")
        || name.starts_with("PG")
        || name.starts_with("TERM")
        || name == "COLORTERM"
        || name == "PATH"
        || PROFILE_VARS.contains(&name)
}

fn default_locale() -> &'static str {
    if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        items
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    fn lookup<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a str> {
        env.iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.to_str())
    }

    #[test]
    fn the_engine_environment_stays_out_and_a_login_passes() {
        let engine = pairs(&[
            ("HOME", "/home/user"),
            ("USER", "user"),
            ("PATH", "/usr/bin:/bin"),
            ("DATABASE_URL", "postgres://secret"),
            ("LUCIDOS_DATABASE_URL", "postgres://agent"),
            ("LUCIDOS_AGENT_ORIGIN_TOKEN", "token"),
            ("ANTHROPIC_API_KEY", "key"),
            ("CRED_GITHUB", "cred"),
            ("CLAUDE_CONFIG_DIR", "/home/user/.engine-claude"),
            ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
        ]);
        let cli = Path::new("/opt/lucidos/bin");
        let env = build(Inputs {
            engine_env: &engine,
            user_env: &[],
            profile: &[],
            workspace: Path::new("/home/user/workspaces/dev"),
            thread_id: Uuid::nil(),
            path_prefixes: &[cli],
        });
        for withheld in [
            "DATABASE_URL",
            "LUCIDOS_DATABASE_URL",
            "LUCIDOS_AGENT_ORIGIN_TOKEN",
            "ANTHROPIC_API_KEY",
            "CRED_GITHUB",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert_eq!(lookup(&env, withheld), None, "{withheld} crossed");
        }
        assert_eq!(lookup(&env, "HOME"), Some("/home/user"));
        assert_eq!(lookup(&env, "SSH_AUTH_SOCK"), Some("/tmp/agent.sock"));
        assert_eq!(lookup(&env, "PATH"), Some("/opt/lucidos/bin:/usr/bin:/bin"));
        assert_eq!(lookup(&env, "TERM"), Some("xterm-256color"));
        assert_eq!(
            lookup(&env, "LUCIDOS_WORKSPACE"),
            Some("/home/user/workspaces/dev")
        );
        assert!(lookup(&env, "LANG").is_some_and(|lang| lang.ends_with("UTF-8")));
    }

    #[test]
    fn a_coding_agent_profile_passes_and_only_its_folders() {
        let profile = pairs(&[
            ("CLAUDE_CONFIG_DIR", "/home/user/.claude-work"),
            ("CODEX_HOME", "/home/user/.codex"),
            ("ANTHROPIC_API_KEY", "key"),
        ]);
        let env = build(Inputs {
            engine_env: &[],
            user_env: &[],
            profile: &profile,
            workspace: Path::new("/w"),
            thread_id: Uuid::nil(),
            path_prefixes: &[],
        });
        assert_eq!(
            lookup(&env, "CLAUDE_CONFIG_DIR"),
            Some("/home/user/.claude-work")
        );
        assert_eq!(lookup(&env, "CODEX_HOME"), Some("/home/user/.codex"));
        assert_eq!(lookup(&env, "ANTHROPIC_API_KEY"), None);
    }

    #[test]
    fn settings_variables_pass_unless_the_engine_reserves_the_name() {
        let user = vec![
            ("MY_TOOL_HOME".to_owned(), "/x".to_owned()),
            ("LUCIDOS_THREAD_ID".to_owned(), "forged".to_owned()),
            ("PGPASSWORD".to_owned(), "pw".to_owned()),
            ("TERM".to_owned(), "vt100".to_owned()),
        ];
        let env = build(Inputs {
            engine_env: &[],
            user_env: &user,
            profile: &[],
            workspace: Path::new("/w"),
            thread_id: Uuid::nil(),
            path_prefixes: &[],
        });
        assert_eq!(lookup(&env, "MY_TOOL_HOME"), Some("/x"));
        assert_eq!(
            lookup(&env, "LUCIDOS_THREAD_ID"),
            Some(Uuid::nil().to_string().as_str())
        );
        assert_eq!(lookup(&env, "PGPASSWORD"), None);
        assert_eq!(lookup(&env, "TERM"), Some("xterm-256color"));
    }
}
