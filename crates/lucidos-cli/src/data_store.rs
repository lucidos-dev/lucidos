//! `lucidos data-store add <name> <source-dir>`
//!
//! Moves an existing directory to `~/.lucidos/data/<name>/` and prints the
//! resolved absolute path on stdout. Used to promote bulk reference corpora
//! out of a workspace's `data/artifacts/` into a persistent, cross-workspace
//! location (see system-knowhow/best-practices rule 8).
//!
//! This is a pure filesystem helper — does not talk to the engine.

use std::path::{Path, PathBuf};

use crate::workspace::BoxError;

pub(crate) fn cmd_add(name: &str, source_dir: &str) -> Result<(), BoxError> {
    let target_root = data_store_root()?;
    let source_path = expand_tilde(source_dir);
    let target = move_into_root(name, &source_path, &target_root)?;
    println!("{}", target.display());
    Ok(())
}

/// Pure helper — takes the target root explicitly so tests don't need to mutate
/// the process-global `HOME` env var. Returns the absolute path of the moved
/// directory on success.
fn move_into_root(name: &str, source: &Path, target_root: &Path) -> Result<PathBuf, BoxError> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is empty".into());
    }
    if name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err(format!(
            "name must be a single path segment with no slashes or leading '.', got {:?}",
            name
        )
        .into());
    }

    if !source.exists() {
        return Err(format!("source directory does not exist: {}", source.display()).into());
    }
    if !source.is_dir() {
        return Err(format!("source is not a directory: {}", source.display()).into());
    }

    std::fs::create_dir_all(target_root).map_err(|e| {
        format!(
            "failed to create data-store root {}: {}",
            target_root.display(),
            e
        )
    })?;

    let target = target_root.join(name);
    if target.exists() {
        return Err(format!(
            "destination already exists: {} (delete it first or pick a different name)",
            target.display()
        )
        .into());
    }

    std::fs::rename(source, &target).map_err(|e| {
        format!(
            "failed to move {} → {}: {}",
            source.display(),
            target.display(),
            e
        )
    })?;
    Ok(target)
}

fn data_store_root() -> Result<PathBuf, BoxError> {
    let home = std::env::var("HOME").map_err(|_| "HOME env var not set")?;
    Ok(Path::new(&home).join(".lucidos").join("data"))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn moves_directory_into_target_root() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();

        let root = tmp.path().join(".lucidos/data");
        let target = move_into_root("wikifonia", &src, &root).unwrap();

        assert_eq!(target, root.join("wikifonia"));
        assert!(target.exists());
        assert_eq!(std::fs::read(target.join("a.txt")).unwrap(), b"hello");
        assert!(!src.exists());
    }

    #[test]
    fn refuses_when_target_already_exists() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");

        let src1 = tmp.path().join("src1");
        std::fs::create_dir_all(&src1).unwrap();
        move_into_root("ds", &src1, &root).unwrap();

        let src2 = tmp.path().join("src2");
        std::fs::create_dir_all(&src2).unwrap();
        let err = move_into_root("ds", &src2, &root).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert!(src2.exists(), "source must be left untouched on refusal");
    }

    #[test]
    fn refuses_missing_source() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let err = move_into_root("foo", Path::new("/nonexistent/path/here"), &root).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn refuses_source_that_is_a_file() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let f = tmp.path().join("not-a-dir.txt");
        std::fs::write(&f, b"x").unwrap();
        let err = move_into_root("foo", &f, &root).unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn rejects_name_with_slash() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(move_into_root("foo/bar", &src, &root).is_err());
    }

    #[test]
    fn rejects_name_with_backslash() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(move_into_root("foo\\bar", &src, &root).is_err());
    }

    #[test]
    fn rejects_empty_name() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(move_into_root("   ", &src, &root).is_err());
    }

    #[test]
    fn rejects_name_starting_with_dot() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(".lucidos/data");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(move_into_root(".hidden", &src, &root).is_err());
    }

    #[test]
    fn creates_target_root_if_missing() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("deeply/nested/.lucidos/data");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        move_into_root("ds", &src, &root).unwrap();
        assert!(root.join("ds").exists());
    }
}
