//! Resolve another workspace's filesystem location and API port from its name.
//!
//! Mirrors `lucidos-cli/src/workspace.rs` + `spawn_thread::resolve_target` so
//! the engine can do cross-workspace POSTs without shelling out to the CLI.
//! Both must agree on:
//!   1. Workspaces live under `$LUCIDOS_WORKSPACES_ROOT` (default `~/workspaces`).
//!   2. Each workspace publishes its API port in `<root>/<name>/.lucidos/ports`
//!      as a `KEY=VALUE` line `API_PORT=<u16>`.
//!   3. The ports file may contain `PROTO=http|https` (defaults to `https`).
//!
//! If the CLI ever changes either contract, this file MUST follow — otherwise
//! `lucidos spawn-thread --to <ws>` and `run_coding_agent(workspace=<ws>)` would
//! resolve to different places.
//!
//! See `workspace_client.rs` for the actual outbound POST that uses these.
use std::path::{Path, PathBuf};

/// A resolved cross-workspace target: where the workspace lives, which port
/// its engine listens on, and which protocol it speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossWorkspaceTarget {
    pub workspace_path: PathBuf,
    pub api_port: u16,
    pub proto: String,
}

/// Resolve `workspace_name` to a `CrossWorkspaceTarget`.
///
/// `workspaces_root` overrides discovery for tests. Production callers pass
/// `None`, which falls back to `$LUCIDOS_WORKSPACES_ROOT` and then to
/// `~/workspaces`.
pub fn resolve_workspace(
    workspace_name: &str,
    workspaces_root: Option<&Path>,
) -> Result<CrossWorkspaceTarget, String> {
    if workspace_name.is_empty() {
        return Err("workspace name is empty".into());
    }
    // Per CLAUDE.md `.claude/rules/rust.md`: "Always check for `..`, leading
    // `/`, leading `\\` before accepting user-provided paths." A bare `..`
    // contains no slash but `root.join("..")` still escapes the workspaces
    // root, so it must be rejected explicitly.
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
        None => std::env::var("LUCIDOS_WORKSPACES_ROOT")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join("workspaces")))
            .map_err(|_| {
                "cannot resolve workspaces root: neither LUCIDOS_WORKSPACES_ROOT nor HOME is set"
                    .to_string()
            })?,
    };
    let workspace_path = root.join(workspace_name);
    let ports_file = workspace_path.join(".lucidos/ports");
    let (api_port, proto) = read_ports(workspace_name, &workspace_path, &ports_file)?;
    Ok(CrossWorkspaceTarget {
        workspace_path,
        api_port,
        proto,
    })
}

/// Parse `API_PORT=` and `PROTO=` out of a `.lucidos/ports` KEY=VALUE file.
/// `PROTO` defaults to `"https"` for backward compatibility with ports files
/// written before it was added. Maps `NotFound` to a friendly hint.
fn read_ports(workspace_name: &str, workspace_path: &Path, ports_file: &Path) -> Result<(u16, String), String> {
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
    let port = port.ok_or_else(|| {
        format!("API_PORT= line not found in {}", ports_file.display())
    })?;
    Ok((port, proto.unwrap_or_else(|| "https".to_string())))
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
    }

    #[test]
    fn reads_proto_from_ports_file() {
        let tmp = TempDir::new().unwrap();
        write_ports(tmp.path(), "dev", "API_PORT=5177\nVITE_PORT=5177\nPROTO=http\n");
        let target = resolve_workspace("dev", Some(tmp.path())).unwrap();
        assert_eq!(target.proto, "http");
    }

    #[test]
    fn errors_when_workspace_dir_missing() {
        let tmp = TempDir::new().unwrap();
        let err = resolve_workspace("nope", Some(tmp.path())).unwrap_err();
        assert!(err.contains("nope"), "error must name the workspace: {}", err);
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

    #[test]
    fn rejects_empty_workspace_name() {
        let err = resolve_workspace("", None).unwrap_err();
        assert!(err.contains("empty"), "error: {}", err);
    }

    #[test]
    fn rejects_workspace_name_with_slashes() {
        // Path traversal guard — the resolver joins onto the root, so a
        // segmented name would let "../foo" escape the workspaces dir.
        let err = resolve_workspace("../escape", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }

    #[test]
    fn rejects_bare_dotdot_workspace_name() {
        // `..` is a single path segment (no slash) but `root.join("..")`
        // escapes the workspaces root. The slash guard wouldn't catch it.
        let err = resolve_workspace("..", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }

    #[test]
    fn rejects_bare_dot_workspace_name() {
        // `.` resolves to the workspaces root itself, which has no
        // .lucidos/ports of its own. Reject for symmetry with `..`.
        let err = resolve_workspace(".", None).unwrap_err();
        assert!(err.contains("single path segment"), "error: {}", err);
    }
}
