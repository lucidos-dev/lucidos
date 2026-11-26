use std::error::Error;
use std::path::{Path, PathBuf};

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Debug)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) api_port: u16,
    pub(crate) proto: String,
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
    let pwd = std::env::current_dir().map_err(|e| format!("Failed to read current dir: {}", e))?;
    let env_ws = std::env::var("LUCIDOS_WORKSPACE").ok().map(PathBuf::from);
    let api_base_override = std::env::var("LUCIDOS_API_BASE_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    resolve_with_override(&pwd, env_ws.as_deref(), api_base_override)
}

pub(crate) fn resolve_with_override(
    start_dir: &Path,
    env_workspace: Option<&Path>,
    api_base_override: Option<String>,
) -> Result<Workspace, BoxError> {
    let Some(base) = api_base_override else {
        return resolve(start_dir, env_workspace);
    };
    let root = env_workspace
        .map(Path::to_path_buf)
        .or_else(|| walk_up_for_ports(start_dir))
        .unwrap_or_else(|| start_dir.to_path_buf());
    let (api_port, proto) = read_ports(&root.join(".lucidos/ports"))
        .map(|(port, recorded)| (port, recorded.unwrap_or_else(|| DEFAULT_PROTO.to_string())))
        .unwrap_or_else(|_| {
            let proto = if base.starts_with("https") {
                "https"
            } else {
                "http"
            };
            (0, proto.to_string())
        });
    Ok(Workspace {
        root,
        api_port,
        proto,
        api_base_override: Some(base),
    })
}

pub(crate) fn resolve(
    start_dir: &Path,
    env_workspace: Option<&Path>,
) -> Result<Workspace, BoxError> {
    if let Some(root) = env_workspace {
        let ports_path = root.join(".lucidos/ports");
        let (api_port, recorded) = read_ports(&ports_path)
            .map_err(|e| format!("LUCIDOS_WORKSPACE={}: {}", root.display(), e))?;
        let proto = recorded.unwrap_or_else(|| DEFAULT_PROTO.to_string());
        return Ok(Workspace {
            root: root.to_path_buf(),
            api_port,
            proto,
            api_base_override: None,
        });
    }

    if let Some(root) = walk_up_for_ports(start_dir) {
        let (api_port, recorded) = read_ports(&root.join(".lucidos/ports"))?;
        let proto = recorded.unwrap_or_else(|| DEFAULT_PROTO.to_string());
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

pub(crate) const DEFAULT_PROTO: &str = "https";

pub(crate) fn assumed_proto_note(ports_path: &Path) -> String {
    format!(
        " ({} has no PROTO= line, so {DEFAULT_PROTO} was assumed; relaunch that \
         workspace to have it record the protocol, or pass --insecure-http)",
        ports_path.display(),
    )
}

pub(crate) fn read_ports(path: &Path) -> Result<(u16, Option<String>), BoxError> {
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
    Ok((port, proto.filter(|p| !p.is_empty())))
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
    fn a_ports_file_with_no_proto_line_resolves_and_says_it_guessed() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 1234);
        let ports = tmp.path().join(".lucidos/ports");
        let (port, recorded) = read_ports(&ports).unwrap();
        assert_eq!(port, 1234);
        assert_eq!(recorded, None);

        let note = assumed_proto_note(&ports);
        assert!(note.contains("PROTO="), "{note}");
        assert!(note.contains(DEFAULT_PROTO), "{note}");
        assert!(note.contains("--insecure-http"), "{note}");
    }

    #[test]
    fn a_recorded_proto_is_taken_and_an_empty_one_is_not() {
        let tmp = tempdir().unwrap();
        let lucidos = tmp.path().join(".lucidos");
        fs::create_dir_all(&lucidos).unwrap();
        let ports = lucidos.join("ports");

        fs::write(&ports, "API_PORT=1\nPROTO=http\n").unwrap();
        assert_eq!(read_ports(&ports).unwrap().1.as_deref(), Some("http"));

        fs::write(&ports, "API_PORT=1\nPROTO=\n").unwrap();
        assert_eq!(read_ports(&ports).unwrap().1, None);
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
        let ws = Workspace {
            root: PathBuf::from("/tmp/test-ws"),
            api_port: 5173,
            proto: "https".to_string(),
            api_base_override: Some("http://127.0.0.1:62072".to_string()),
        };
        assert_eq!(ws.base_url(), "http://127.0.0.1:62072");
    }

    #[test]
    fn override_makes_ports_file_optional() {
        let ws = tempdir().unwrap();
        let unrelated = tempdir().unwrap();
        let resolved = resolve_with_override(
            unrelated.path(),
            Some(ws.path()),
            Some("http://127.0.0.1:57206".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.root, ws.path());
        assert_eq!(resolved.base_url(), "http://127.0.0.1:57206");
    }

    #[test]
    fn override_wins_even_when_ports_file_present() {
        let ws = tempdir().unwrap();
        write_ports(ws.path(), 5174);
        let resolved = resolve_with_override(
            ws.path(),
            Some(ws.path()),
            Some("http://127.0.0.1:62072".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.base_url(), "http://127.0.0.1:62072");
    }

    #[test]
    fn override_absent_still_requires_ports_file() {
        let tmp = tempdir().unwrap();
        let err = resolve_with_override(tmp.path(), None, None).unwrap_err();
        assert!(err.to_string().contains("Could not locate"), "{err}");
    }

    #[test]
    fn override_root_falls_back_to_start_dir_without_env_workspace() {
        let start = tempdir().unwrap();
        let resolved =
            resolve_with_override(start.path(), None, Some("http://127.0.0.1:1".to_string()))
                .unwrap();
        assert_eq!(resolved.root, start.path());
        assert_eq!(resolved.base_url(), "http://127.0.0.1:1");
    }

    #[test]
    fn base_url_falls_back_to_ports_file_without_override() {
        let ws = Workspace {
            root: PathBuf::from("/tmp/test-ws"),
            api_port: 5177,
            proto: "https".to_string(),
            api_base_override: None,
        };
        assert_eq!(ws.base_url(), "https://localhost:5177");
    }
}
