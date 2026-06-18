use std::error::Error;
use std::path::{Path, PathBuf};

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Debug)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) api_port: u16,
    pub(crate) proto: String,
    /// Explicit engine base URL handed down by the engine (`LUCIDOS_API_BASE_URL`),
    /// used in preference to `proto`/`api_port` when present. Under the workspace
    /// gateway (ADR 0014) the `.lucidos/ports` file holds the user-facing gateway
    /// port — but the gateway routes the workspace under `/<slug>/`, so a bare
    /// `https://localhost:<port>/api/v1/...` request never reaches this engine
    /// (the gateway resolves the first path segment as a workspace slug). The
    /// engine therefore exports the loopback HTTP URL it is actually reachable at;
    /// we honour it so the CLI talks to the engine directly. `None` in legacy /
    /// Tauri / terminal use → fall back to the ports file (which resolves the
    /// engine correctly there).
    pub(crate) api_base_override: Option<String>,
}

impl Workspace {
    pub(crate) fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub(crate) fn base_url(&self) -> String {
        if let Some(base) = &self.api_base_override {
            return base.clone();
        }
        format!("{}://localhost:{}", self.proto, self.api_port)
    }
}

pub(crate) fn resolve_from_env() -> Result<Workspace, BoxError> {
    let pwd = std::env::current_dir()
        .map_err(|e| format!("Failed to read current dir: {}", e))?;
    let env_ws = std::env::var("LUCIDOS_WORKSPACE").ok().map(PathBuf::from);
    let mut ws = resolve(&pwd, env_ws.as_deref())?;
    // Prefer the engine-provided base URL when set (workspace gateway, ADR 0014
    // — see `Workspace::api_base_override`). Read here, not in `resolve`, to keep
    // `resolve` free of env reads so tests can drive it deterministically.
    if let Some(base) = std::env::var("LUCIDOS_API_BASE_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        ws.api_base_override = Some(base);
    }
    Ok(ws)
}

/// Separated from env reads so tests can drive it without racing the global
/// env table.
///
/// Precedence: `LUCIDOS_WORKSPACE` env var beats walking up from `start_dir`.
/// The env var is set explicitly by the engine that spawned the subprocess
/// (CC, scheduler, etc.) and is the authoritative source. Walk-up is the
/// fallback path for terminal users who haven't set it. Doing it the other way
/// — walk-up first — was the cause of a production bug: a rogue lucidos-engine
/// started inside a CC worktree dropped its own `.lucidos/ports` file, so every
/// `lucidos hardened mark` from CC POSTed to the wrong DB. The parent engine
/// then saw Missing at apply time and triggered an unnecessary auto-/harden.
pub(crate) fn resolve(start_dir: &Path, env_workspace: Option<&Path>) -> Result<Workspace, BoxError> {
    if let Some(root) = env_workspace {
        let ports_path = root.join(".lucidos/ports");
        let (api_port, proto) = read_ports(&ports_path).map_err(|e| {
            format!(
                "LUCIDOS_WORKSPACE={}: {}",
                root.display(),
                e
            )
        })?;
        return Ok(Workspace {
            root: root.to_path_buf(),
            api_port,
            proto,
            api_base_override: None,
        });
    }

    if let Some(root) = walk_up_for_ports(start_dir) {
        let (api_port, proto) = read_ports(&root.join(".lucidos/ports"))?;
        return Ok(Workspace {
            root,
            api_port,
            proto,
            api_base_override: None,
        });
    }

    Err(format!(
        "Could not locate a Lucidos workspace. Walked up from {} looking for .lucidos/ports, \
         and $LUCIDOS_WORKSPACE is not set.",
        start_dir.display()
    )
    .into())
}

fn walk_up_for_ports(start_dir: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start_dir);
    while let Some(dir) = cur {
        if dir.join(".lucidos/ports").is_file() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Read API_PORT and PROTO from the ports file. PROTO defaults to "https"
/// for backward compatibility with ports files written before it was added.
pub(crate) fn read_ports(path: &Path) -> Result<(u16, String), BoxError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let mut port: Option<u16> = None;
    let mut proto: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("API_PORT=") {
            port = Some(
                rest.trim()
                    .parse()
                    .map_err(|e| format!("Invalid API_PORT in {}: {}", path.display(), e))?,
            );
        } else if let Some(rest) = line.strip_prefix("PROTO=") {
            proto = Some(rest.trim().to_string());
        }
    }
    let port = port.ok_or_else(|| format!("API_PORT line not found in {}", path.display()))?;
    Ok((port, proto.unwrap_or_else(|| "https".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_ports(dir: &Path, port: u16) {
        let lucidos = dir.join(".lucidos");
        fs::create_dir_all(&lucidos).unwrap();
        fs::write(
            lucidos.join("ports"),
            format!("API_PORT={}\nVITE_PORT={}\n", port, port),
        )
        .unwrap();
    }

    #[test]
    fn resolves_from_workspace_root() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 1234);
        let ws = resolve(tmp.path(), None).unwrap();
        assert_eq!(ws.root, tmp.path());
        assert_eq!(ws.api_port, 1234);
        assert_eq!(ws.proto, "https");
    }

    #[test]
    fn resolves_from_worktree_subdirectory() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 5555);
        let worktree = tmp.path().join(".lucidos/worktrees/abc/sub/dir");
        fs::create_dir_all(&worktree).unwrap();
        let ws = resolve(&worktree, None).unwrap();
        assert_eq!(ws.root, tmp.path());
        assert_eq!(ws.api_port, 5555);
    }

    #[test]
    fn resolves_does_not_pick_intermediate_lucidos_dir() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 8080);
        let mid = tmp.path().join(".lucidos");
        let ws = resolve(&mid, None).unwrap();
        assert_eq!(ws.root, tmp.path());
    }

    #[test]
    fn falls_back_to_env_var() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 7777);
        let unrelated = tempdir().unwrap();
        let ws = resolve(unrelated.path(), Some(tmp.path())).unwrap();
        assert_eq!(ws.root, tmp.path());
        assert_eq!(ws.api_port, 7777);
    }

    #[test]
    fn env_var_takes_precedence_over_walk_up() {
        let walked = tempdir().unwrap();
        write_ports(walked.path(), 100);
        let env = tempdir().unwrap();
        write_ports(env.path(), 200);
        let ws = resolve(walked.path(), Some(env.path())).unwrap();
        assert_eq!(ws.api_port, 200);
        assert_eq!(ws.root, env.path());
    }

    #[test]
    fn env_var_wins_when_nested_workspace_lurks_in_cwd_ancestry() {
        let parent_ws = tempdir().unwrap();
        write_ports(parent_ws.path(), 5173);

        let worktree = parent_ws.path().join(".lucidos/worktrees/thread-abc");
        fs::create_dir_all(&worktree).unwrap();
        write_ports(&worktree, 5177);

        let ws = resolve(&worktree, Some(parent_ws.path())).unwrap();
        assert_eq!(ws.api_port, 5173);
        assert_eq!(ws.root, parent_ws.path());
    }

    #[test]
    fn errors_when_no_workspace_anywhere() {
        let tmp = tempdir().unwrap();
        let err = resolve(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("Could not locate"));
    }

    #[test]
    fn errors_on_env_var_pointing_at_missing_ports() {
        let tmp = tempdir().unwrap();
        let env = tempdir().unwrap();
        let err = resolve(tmp.path(), Some(env.path())).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("LUCIDOS_WORKSPACE"), "msg: {msg}");
        assert!(msg.contains("Failed to read"), "msg: {msg}");
    }

    #[test]
    fn errors_on_missing_api_port_line() {
        let tmp = tempdir().unwrap();
        let lucidos = tmp.path().join(".lucidos");
        fs::create_dir_all(&lucidos).unwrap();
        fs::write(lucidos.join("ports"), "VITE_PORT=9000\n").unwrap();
        let err = resolve(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("API_PORT"));
    }

    #[test]
    fn reads_proto_from_ports_file() {
        let tmp = tempdir().unwrap();
        let lucidos = tmp.path().join(".lucidos");
        fs::create_dir_all(&lucidos).unwrap();
        fs::write(
            lucidos.join("ports"),
            "API_PORT=5177\nVITE_PORT=5177\nPROTO=http\n",
        )
        .unwrap();
        let ws = resolve(tmp.path(), None).unwrap();
        assert_eq!(ws.proto, "http");
        assert_eq!(ws.base_url(), "http://localhost:5177");
    }

    #[test]
    fn defaults_to_https_when_proto_missing() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 5177);
        let ws = resolve(tmp.path(), None).unwrap();
        assert_eq!(ws.proto, "https");
        assert_eq!(ws.base_url(), "https://localhost:5177");
    }

    #[test]
    fn base_url_prefers_explicit_override() {
        // Under the workspace gateway (ADR 0013) the engine hands the CLI an
        // explicit loopback base URL via LUCIDOS_API_BASE_URL. It must win over
        // the ports-file port, which under the gateway is the user-facing
        // gateway port (a bare `/api/v1` request there returns the picker's
        // SPA HTML, not JSON — the trigger-notification bug this fixes).
        let ws = Workspace {
            root: PathBuf::from("/tmp/test-ws"),
            api_port: 5173,
            proto: "https".to_string(),
            api_base_override: Some("http://127.0.0.1:62072".to_string()),
        };
        assert_eq!(ws.base_url(), "http://127.0.0.1:62072");
    }

    #[test]
    fn base_url_falls_back_to_ports_file_without_override() {
        // Legacy / Tauri / terminal: no override → ports-file proto + port.
        let ws = Workspace {
            root: PathBuf::from("/tmp/test-ws"),
            api_port: 5177,
            proto: "https".to_string(),
            api_base_override: None,
        };
        assert_eq!(ws.base_url(), "https://localhost:5177");
    }
}
