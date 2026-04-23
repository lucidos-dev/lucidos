use std::error::Error;
use std::path::{Path, PathBuf};

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Debug)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) api_port: u16,
}

impl Workspace {
    pub(crate) fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub(crate) fn base_url(&self) -> String {
        format!("https://localhost:{}", self.api_port)
    }
}

pub(crate) fn resolve_from_env() -> Result<Workspace, BoxError> {
    let pwd = std::env::current_dir()
        .map_err(|e| format!("Failed to read current dir: {}", e))?;
    let env_ws = std::env::var("COGNOS_WORKSPACE").ok().map(PathBuf::from);
    resolve(&pwd, env_ws.as_deref())
}

/// Separated from env reads so tests can drive it without racing the global
/// env table.
pub(crate) fn resolve(start_dir: &Path, env_workspace: Option<&Path>) -> Result<Workspace, BoxError> {
    if let Some(root) = walk_up_for_ports(start_dir) {
        let api_port = read_api_port(&root.join(".cognos/ports"))?;
        return Ok(Workspace { root, api_port });
    }

    if let Some(root) = env_workspace {
        let api_port = read_api_port(&root.join(".cognos/ports")).map_err(|e| {
            format!(
                "COGNOS_WORKSPACE={}: {}",
                root.display(),
                e
            )
        })?;
        return Ok(Workspace {
            root: root.to_path_buf(),
            api_port,
        });
    }

    Err(format!(
        "Could not locate a CognOS workspace. Walked up from {} looking for .cognos/ports, \
         and $COGNOS_WORKSPACE is not set.",
        start_dir.display()
    )
    .into())
}

fn walk_up_for_ports(start_dir: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start_dir);
    while let Some(dir) = cur {
        if dir.join(".cognos/ports").is_file() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

fn read_api_port(path: &Path) -> Result<u16, BoxError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("API_PORT=") {
            let port: u16 = rest
                .trim()
                .parse()
                .map_err(|e| format!("Invalid API_PORT in {}: {}", path.display(), e))?;
            return Ok(port);
        }
    }
    Err(format!("API_PORT line not found in {}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_ports(dir: &Path, port: u16) {
        let cognos = dir.join(".cognos");
        fs::create_dir_all(&cognos).unwrap();
        fs::write(
            cognos.join("ports"),
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
    }

    #[test]
    fn resolves_from_worktree_subdirectory() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 5555);
        let worktree = tmp.path().join(".cognos/worktrees/abc/sub/dir");
        fs::create_dir_all(&worktree).unwrap();
        let ws = resolve(&worktree, None).unwrap();
        assert_eq!(ws.root, tmp.path());
        assert_eq!(ws.api_port, 5555);
    }

    #[test]
    fn resolves_does_not_pick_intermediate_cognos_dir() {
        let tmp = tempdir().unwrap();
        write_ports(tmp.path(), 8080);
        let mid = tmp.path().join(".cognos");
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
    fn walk_up_takes_precedence_over_env_var() {
        let walked = tempdir().unwrap();
        write_ports(walked.path(), 100);
        let env = tempdir().unwrap();
        write_ports(env.path(), 200);
        let ws = resolve(walked.path(), Some(env.path())).unwrap();
        assert_eq!(ws.api_port, 100);
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
        assert!(msg.contains("COGNOS_WORKSPACE"), "msg: {msg}");
        assert!(msg.contains("Failed to read"), "msg: {msg}");
    }

    #[test]
    fn errors_on_missing_api_port_line() {
        let tmp = tempdir().unwrap();
        let cognos = tmp.path().join(".cognos");
        fs::create_dir_all(&cognos).unwrap();
        fs::write(cognos.join("ports"), "VITE_PORT=9000\n").unwrap();
        let err = resolve(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("API_PORT"));
    }
}
