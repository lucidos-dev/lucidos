use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::workspace::{BoxError, Workspace};

/// Sub-directories of the workspace's `data/` that the CLI accepts as
/// already-prefixed when resolving paths. The frontend has a similar list at
/// `crates/lucidos-app/src/store/actions/artifacts.ts`, but it includes
/// `system-knowhow/` (engine-shipped, served from the engine repo); that prefix
/// does *not* belong here because it isn't a workspace data sub-directory.
const DATA_PREFIXES: &[&str] = &["artifacts/", "knowhow/", "apps/", "triggers/"];

/// Validate `relative` and return its normalized, `data/`-rooted store path
/// (e.g. `loose.txt` → `artifacts/loose.txt`). This store-relative form — with
/// NO scheme — is the canonical clickable link target in Lucidos chat; see
/// `chat_link` / `cmd_write`.
pub(crate) fn normalize_data_path(relative: &str) -> Result<String, BoxError> {
    let trimmed = relative.trim();
    if trimmed.is_empty() {
        return Err("path is empty".into());
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(format!("path must be relative, got {:?}", relative).into());
    }
    if trimmed.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("path may not contain '..' segments, got {:?}", relative).into());
    }
    Ok(normalize(trimmed))
}

pub(crate) fn resolve_data_path(ws: &Workspace, relative: &str) -> Result<PathBuf, BoxError> {
    Ok(ws.data_dir().join(normalize_data_path(relative)?))
}

/// Build the ready-to-paste clickable Lucidos chat link for a freshly written
/// data file. The target is the bare, `data/`-rooted store path (NO scheme):
/// the frontend's path linkifier rewrites it into a file-preview link. An
/// invented `artifact:` / `file:` scheme dead-ends — no handler claims it — so
/// the link MUST stay scheme-less. Label defaults to the file's basename.
fn chat_link(normalized: &str) -> String {
    let label = normalized.rsplit('/').next().unwrap_or(normalized);
    format!("[{}]({})", label, normalized)
}

fn normalize(path: &str) -> String {
    if DATA_PREFIXES.iter().any(|p| path.starts_with(p)) {
        path.to_string()
    } else {
        format!("artifacts/{}", path)
    }
}

pub(crate) fn cmd_path(ws: &Workspace, relative: &str, mkdir: bool) -> Result<(), BoxError> {
    let abs = resolve_data_path(ws, relative)?;
    if mkdir {
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("Failed to create parent dir {}: {}", parent.display(), e)
            })?;
        }
    }
    println!("{}", abs.display());
    Ok(())
}

pub(crate) enum WriteSource {
    Stdin,
    File(PathBuf),
}

pub(crate) fn cmd_write(ws: &Workspace, relative: &str, source: WriteSource) -> Result<(), BoxError> {
    let normalized = normalize_data_path(relative)?;
    let abs = ws.data_dir().join(&normalized);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent dir {}: {}", parent.display(), e))?;
    }

    let bytes = match source {
        WriteSource::Stdin => {
            let mut buf = Vec::new();
            io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            buf
        }
        WriteSource::File(path) => std::fs::read(&path)
            .map_err(|e| format!("Failed to read source file {}: {}", path.display(), e))?,
    };

    std::fs::write(&abs, &bytes)
        .map_err(|e| format!("Failed to write {}: {}", abs.display(), e))?;

    // Echo the resolved absolute path on stderr so callers see exactly what was
    // written, keeping stdout clean for the clickable link below.
    writeln!(io::stderr(), "{}", abs.display())
        .map_err(|e| format!("Failed to write status to stderr: {}", e))?;

    // Print a ready-to-paste clickable Lucidos chat link on stdout, mirroring
    // `lucidos spawn-thread`. This gives the agent a canonical, working link to
    // hand the user instead of inventing an `artifact:`/`file:` scheme that the
    // frontend has no handler for and that dead-ends on click.
    println!("{}", chat_link(&normalized));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ws_at(root: PathBuf) -> Workspace {
        Workspace { root, api_port: 0, proto: "https".to_string() }
    }

    #[test]
    fn keeps_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "artifacts/ua/report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/ua/report.html"));
    }

    #[test]
    fn keeps_each_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        for prefix in DATA_PREFIXES {
            let path = format!("{}{}", prefix, "child/file.txt");
            let abs = resolve_data_path(&ws, &path).unwrap();
            assert_eq!(abs, PathBuf::from(format!("/ws/data/{}", path)));
        }
    }

    #[test]
    fn prepends_artifacts_when_missing_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/report.html"));
    }

    #[test]
    fn rejects_absolute_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_dot_dot_segment() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "artifacts/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "   ").is_err());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let tmp = tempdir().unwrap();
        let ws = ws_at(tmp.path().to_path_buf());
        let src = tmp.path().join("input.txt");
        std::fs::write(&src, b"hello").unwrap();
        cmd_write(&ws, "artifacts/deeply/nested/x.txt", WriteSource::File(src)).unwrap();
        let written = tmp.path().join("data/artifacts/deeply/nested/x.txt");
        assert_eq!(std::fs::read(&written).unwrap(), b"hello");
    }

    #[test]
    fn normalize_data_path_prepends_artifacts_when_missing_prefix() {
        assert_eq!(normalize_data_path("report.html").unwrap(), "artifacts/report.html");
    }

    #[test]
    fn normalize_data_path_keeps_known_prefix() {
        assert_eq!(normalize_data_path("knowhow/x.md").unwrap(), "knowhow/x.md");
    }

    #[test]
    fn chat_link_uses_basename_label_and_bare_path_target() {
        // No scheme on the target — a bare store path is what the frontend
        // linkifier rewrites to a file preview; `artifact:`/`file:` dead-ends.
        assert_eq!(
            chat_link("artifacts/ost-jira-workflow/node-types-and-attributes.md"),
            "[node-types-and-attributes.md](artifacts/ost-jira-workflow/node-types-and-attributes.md)"
        );
    }

    #[test]
    fn chat_link_handles_top_level_file() {
        assert_eq!(chat_link("artifacts/report.html"), "[report.html](artifacts/report.html)");
    }

    #[test]
    fn write_normalizes_loose_filenames() {
        let tmp = tempdir().unwrap();
        let ws = ws_at(tmp.path().to_path_buf());
        let src = tmp.path().join("input.txt");
        std::fs::write(&src, b"data").unwrap();
        cmd_write(&ws, "loose-name.txt", WriteSource::File(src)).unwrap();
        let written = tmp.path().join("data/artifacts/loose-name.txt");
        assert!(written.exists());
    }
}
