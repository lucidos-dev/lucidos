//! INVARIANT: Resolve another workspace's filesystem location and API port from its name.
//!
//! Mirrors `lucidos-cli/src/workspace.rs` + `spawn_thread::resolve_target` so
//! the engine can do cross-workspace POSTs without shelling out to the CLI.
//! Both must agree on:
//!   1. Workspaces live under [`workspaces_root`]: `$LUCIDOS_WORKSPACES_ROOT`,
//!      else beside `$LUCIDOS_WORKSPACE`, else `~/workspaces`.
//!   2. Each workspace publishes its API port in `<root>/<name>/.lucidos/ports`
//!      as a `KEY=VALUE` line `API_PORT=<u16>`.
//!   3. The ports file may contain `PROTO=http|https`. An absent line means the
//!      file predates the key, so `https` is assumed and the guess is recorded
//!      (`CrossWorkspaceTarget::proto_assumed`) for a failure to name.
//!
//! If the CLI ever changes either contract, this file MUST follow — otherwise
//! `lucidos spawn-thread --to <ws>` and `run_coding_agent(workspace=<ws>)` would
//! resolve to different places.
//!
//! See `workspace_client.rs` for the actual outbound POST that uses these.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossWorkspaceTarget {
    pub workspace_path: PathBuf,
    pub api_port: u16,
    pub proto: String,
    pub proto_assumed: bool,
}

impl CrossWorkspaceTarget {
    pub fn assumed_proto_note(&self) -> String {
        if !self.proto_assumed {
            return String::new();
        }
        format!(
            " ({} has no PROTO= line, so {} was assumed; relaunch that workspace \
             to have it record the protocol)",
            ports_file_path(&self.workspace_path).display(),
            self.proto,
        )
    }
}

pub fn ports_file_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(".lucidos/ports")
}

pub fn resolve_workspace(
    workspace_name: &str,
    workspaces_root: Option<&Path>,
) -> Result<CrossWorkspaceTarget, String> {
    if workspace_name.is_empty() {
        return Err("workspace name is empty".into());
    }
    if workspace_name.contains('/')
        || workspace_name.contains('\\')
        || workspace_name == ".."
        || workspace_name == "."
    {
        return Err(format!(
            "workspace name '{}' must be a single path segment (no '/', '\\\\', '.', or '..')",
            workspace_name
        ));
    }
    let root = match workspaces_root {
        Some(r) => r.to_path_buf(),
        None => workspaces_root_from_env(
            std::env::var_os("LUCIDOS_WORKSPACES_ROOT"),
            std::env::var_os("LUCIDOS_WORKSPACE"),
            std::env::var_os("HOME"),
        )?,
    };
    let workspace_path = root.join(workspace_name);
    let ports_file = ports_file_path(&workspace_path);
    let (api_port, recorded) = read_ports(workspace_name, &workspace_path, &ports_file)?;
    Ok(CrossWorkspaceTarget {
        workspace_path,
        api_port,
        proto: recorded.clone().unwrap_or_else(default_proto),
        proto_assumed: recorded.is_none(),
    })
}

fn default_proto() -> String {
    "https".to_string()
}

fn workspaces_root_from_env(
    explicit: Option<std::ffi::OsString>,
    workspace: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if let Some(root) = explicit {
        return Ok(PathBuf::from(root));
    }
    if let Some(parent) = workspace
        .map(PathBuf::from)
        .as_deref()
        .and_then(Path::parent)
    {
        return Ok(parent.to_path_buf());
    }
    home.map(|h| PathBuf::from(h).join("workspaces"))
        .ok_or_else(|| {
            "cannot resolve workspaces root: none of LUCIDOS_WORKSPACES_ROOT, \
             LUCIDOS_WORKSPACE or HOME is set"
                .to_string()
        })
}

fn read_ports(
    workspace_name: &str,
    workspace_path: &Path,
    ports_file: &Path,
) -> Result<(u16, Option<String>), String> {
    let text = std::fs::read_to_string(ports_file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "workspace '{}' not found at {} (no .lucidos/ports — is the engine running?)",
                workspace_name,
                workspace_path.display()
            )
        } else {
            format!("cannot read {}: {}", ports_file.display(), e)
        }
    })?;
    let mut port: Option<u16> = None;
    let mut proto: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("API_PORT=") {
            port = Some(
                rest.trim()
                    .parse::<u16>()
                    .map_err(|e| format!("bad API_PORT in {}: {}", ports_file.display(), e))?,
            );
        } else if let Some(rest) = line.strip_prefix("PROTO=") {
            proto = Some(rest.trim().to_string());
        }
    }
    let port =
        port.ok_or_else(|| format!("API_PORT= line not found in {}", ports_file.display()))?;
    Ok((port, proto.filter(|p| !p.is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_ports(root: &Path, name: &str, contents: &str) {
        let dir = root.join(name).join(".lucidos");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ports"), contents).unwrap();
    }

    #[test]
    fn resolves_workspace_with_well_formed_ports_file() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "dev", "API_PORT=5174\nVITE_PORT=5174\n");
        let target = resolve_workspace("dev", Some(tmp.path())).unwrap();
        assert_eq!(target.api_port, 5174);
        assert_eq!(target.workspace_path, tmp.path().join("dev"));
        assert_eq!(target.proto, "https");
        assert!(target.proto_assumed, "no PROTO= line means a guess");
    }

    #[test]
    fn reads_proto_from_ports_file() {
        let tmp = TempDir::new().unwrap();
        write_ports(
            tmp.path(),
            "dev",
            "API_PORT=5177\nVITE_PORT=5177\nPROTO=http\n",
        );
        let target = resolve_workspace("dev", Some(tmp.path())).unwrap();
        assert_eq!(target.proto, "http");
        assert!(!target.proto_assumed);
        assert_eq!(target.assumed_proto_note(), "");
    }

    #[test]
    fn a_ports_file_with_no_proto_line_resolves_and_says_it_guessed() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "old", "API_PORT=5180\nVITE_PORT=5180\n");
        let target = resolve_workspace("old", Some(tmp.path())).unwrap();
        assert_eq!(target.api_port, 5180);
        assert_eq!(target.proto, "https");
        assert!(target.proto_assumed);

        let note = target.assumed_proto_note();
        assert!(note.contains("PROTO="), "{note}");
        assert!(note.contains("https"), "{note}");
        assert!(note.contains(".lucidos/ports"), "{note}");
    }

    #[test]
    fn an_empty_proto_value_is_no_answer_at_all() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "ws", "API_PORT=5181\nPROTO=\n");
        let target = resolve_workspace("ws", Some(tmp.path())).unwrap();
        assert_eq!(target.proto, "https");
        assert!(target.proto_assumed);
    }

    #[test]
    fn errors_when_workspace_dir_missing() {
        let tmp = TempDir::new().unwrap();
        let err = resolve_workspace("nope", Some(tmp.path())).unwrap_err();
        assert!(
            err.contains("nope"),
            "error must name the workspace: {}",
            err
        );
        assert!(
            err.contains(".lucidos/ports"),
            "error must mention the ports file we looked for: {}",
            err
        );
    }

    #[test]
    fn errors_when_ports_file_lacks_api_port_line() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "ws", "VITE_PORT=5174\n");
        let err = resolve_workspace("ws", Some(tmp.path())).unwrap_err();
        assert!(err.contains("API_PORT="), "error: {}", err);
    }

    #[test]
    fn errors_when_api_port_is_not_a_number() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "ws", "API_PORT=not-a-number\n");
        let err = resolve_workspace("ws", Some(tmp.path())).unwrap_err();
        assert!(err.contains("bad API_PORT"), "error: {}", err);
    }

    fn os(s: &str) -> std::ffi::OsString {
        std::ffi::OsString::from(s)
    }

    #[test]
    fn a_bare_name_resolves_beside_the_callers_own_workspace() {
        assert_eq!(
            workspaces_root_from_env(None, Some(os("/app-data/workspaces/other")), Some(os("/h")))
                .unwrap(),
            PathBuf::from("/app-data/workspaces")
        );
    }

    #[test]
    fn an_explicit_root_beats_the_callers_workspace() {
        assert_eq!(
            workspaces_root_from_env(
                Some(os("/elsewhere")),
                Some(os("/app-data/workspaces/other")),
                Some(os("/h"))
            )
            .unwrap(),
            PathBuf::from("/elsewhere")
        );
    }

    #[test]
    fn home_workspaces_is_still_the_last_resort() {
        assert_eq!(
            workspaces_root_from_env(None, None, Some(os("/h"))).unwrap(),
            PathBuf::from("/h/workspaces")
        );
    }

    #[test]
    fn a_parentless_workspace_falls_through_to_home() {
        assert_eq!(
            workspaces_root_from_env(None, Some(os("/")), Some(os("/h"))).unwrap(),
            PathBuf::from("/h/workspaces")
        );
    }

    #[test]
    fn rejects_empty_workspace_name() {
        let err = resolve_workspace("", None).unwrap_err();
        assert!(err.contains("empty"), "error: {}", err);
    }

    #[test]
    fn rejects_workspace_name_with_slashes() {
        let err = resolve_workspace("../escape", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }

    #[test]
    fn rejects_bare_dotdot_workspace_name() {
        let err = resolve_workspace("..", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }

    #[test]
    fn rejects_bare_dot_workspace_name() {
        let err = resolve_workspace(".", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }
}
