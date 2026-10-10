//! Plugin install-source detection and fetching: classify a source string
//! (git URL / GitHub tree URL / local archive), clone or unzip it into a
//! scratch dir, and the per-file atomic copy used by the writer. Pure
//! filesystem/network helpers — no engine or event-bus coupling.

use sqlx::PgPool;
use std::path::{Path, PathBuf};

use crate::core::git_auth::GitCredentials;
use crate::core::plugins::{
    is_git_url, validate_archive_entry_path, ValidationError, PLUGIN_ARCHIVE_EXT,
};

/// Where the source string points and how to fetch it.
#[derive(Debug)]
pub(crate) enum Source {
    /// Full git URL with optional branch and subpath inside the repo.
    Git {
        url: String,
        branch: Option<String>,
        subpath: Option<String>,
    },
    /// Local path to a `.lucidos-plugin` archive.
    Archive(PathBuf),
}

/// How a successfully-fetched plugin was obtained. Serialized into
/// `PluginInstalled.source_type` via `as_str()` -- the wire format is
/// `"git"` or `"archive"` and downstream consumers (events table,
/// future projections) match on those literals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceType {
    Git,
    Archive,
}

impl SourceType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Archive => "archive",
        }
    }
}

/// Detect the install source by shape of the input string.
pub(crate) fn detect_source(s: &str) -> Result<Source, String> {
    let trimmed = s.trim();

    // GitHub tree URL → repo + branch + subpath.
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        if let Some(parsed) = parse_github_tree(rest) {
            return Ok(parsed);
        }
    }

    if is_git_url(trimmed) {
        return Ok(Source::Git {
            url: trimmed.to_string(),
            branch: None,
            subpath: None,
        });
    }

    if trimmed.ends_with(PLUGIN_ARCHIVE_EXT) {
        let path = PathBuf::from(trimmed);
        if path.is_file() {
            return Ok(Source::Archive(path));
        }
        return Err(format!(
            "archive path not found or not a file: {}",
            path.display()
        ));
    }

    Err(format!(
        "could not infer source type from '{}' — expected a git URL, a GitHub tree URL, or a path to a .lucidos-plugin file",
        trimmed
    ))
}

fn parse_github_tree(rest: &str) -> Option<Source> {
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    if parts.len() < 4 || parts[2] != "tree" {
        return None;
    }
    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    let branch = parts[3].to_string();
    let subpath = if parts.len() == 5 && !parts[4].is_empty() {
        Some(parts[4].trim_end_matches('/').to_string())
    } else {
        None
    };
    Some(Source::Git {
        url: format!("https://github.com/{}/{}.git", owner, repo),
        branch: Some(branch),
        subpath,
    })
}

/// The stored credentials a [`fetch_source`] of `source_str` may present.
///
/// The fetch is synchronous, so its credentials are resolved out here. A
/// local archive and an unparseable string need none: the fetch itself
/// reports whatever is wrong with them.
pub(crate) async fn credentials_for_source(pool: &PgPool, source_str: &str) -> GitCredentials {
    match detect_source(source_str) {
        Ok(Source::Git { url, .. }) => GitCredentials::resolve_one(pool, &url).await,
        _ => GitCredentials::none(),
    }
}

/// Fetch the source into a fresh `tempfile::TempDir` under
/// `.lucidos/tmp/plugins/`. Returns the TempDir guard (auto-cleans on drop)
/// and the plugin root inside it (where `manifest.toml` lives — for git-tree
/// URLs this is the subpath, not the repo root).
pub(crate) fn fetch_source(
    workspace: &Path,
    source: &Source,
    credentials: &GitCredentials,
) -> Result<(tempfile::TempDir, PathBuf, SourceType), String> {
    let parent = workspace.join(crate::core::TMP_DIR).join("plugins");
    std::fs::create_dir_all(&parent).map_err(|e| format!("create scratch dir: {}", e))?;
    let scratch =
        tempfile::TempDir::new_in(&parent).map_err(|e| format!("create scratch dir: {}", e))?;

    match source {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            let clone_target = scratch.path().join("repo");
            // The returned repo drops at the end of this statement: git2 types
            // are not Send, and the caller awaits. `shallow_clone` handles
            // credentials and the `file://` depth exception.
            crate::core::git_auth::shallow_clone(
                url,
                branch.as_deref(),
                &clone_target,
                credentials,
            )?;
            let _ = std::fs::remove_dir_all(clone_target.join(".git"));

            let plugin_root = match subpath {
                Some(sub) => {
                    // The subpath is parsed out of an LLM-supplied source string,
                    // so it must be validated before the join. A `..` run (or a
                    // leading `/`, which `Path::join` substitutes wholesale)
                    // escapes the scratch dir, and the escaped directory's
                    // contents get copied into the workspace `data/` on Confirm.
                    if crate::core::is_path_traversal(sub) {
                        return Err(format!(
                            "rejected plugin subpath '{}': must be relative with no '..', \
                             leading '/' or leading '\\'",
                            sub
                        ));
                    }
                    clone_target.join(sub)
                }
                None => clone_target.clone(),
            };
            if !plugin_root.is_dir() {
                return Err(format!(
                    "subpath not found inside repo: {}",
                    subpath.as_deref().unwrap_or("")
                ));
            }
            Ok((scratch, plugin_root, SourceType::Git))
        }
        Source::Archive(path) => {
            let extract_target = scratch.path().join("plugin");
            std::fs::create_dir_all(&extract_target)
                .map_err(|e| format!("create extract dir: {}", e))?;
            extract_zip(path, &extract_target)?;
            Ok((scratch, extract_target, SourceType::Archive))
        }
    }
}

pub(crate) fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("open archive: {}", e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read archive: {}", e))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("zip entry {}: {}", i, e))?;
        let raw_name = entry.name().to_string();
        validate_archive_entry_path(&raw_name)
            .map_err(|e: ValidationError| format!("rejected archive entry: {}", e))?;

        let outpath = dest.join(&raw_name);
        if entry.is_dir() {
            std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir {:?}: {}", outpath, e))?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
        }
        let mut out = std::fs::File::create(&outpath)
            .map_err(|e| format!("create file {:?}: {}", outpath, e))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("write file {:?}: {}", outpath, e))?;
    }
    Ok(())
}

/// Atomic-per-file copy: write to `<dest>.tmp` then rename. Caller is
/// responsible for ensuring the parent dir exists.
pub(crate) fn copy_atomic(src: &Path, dst: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::copy(src, tmp)
            .map(|_| ())
            .map_err(|e| format!("copy {:?} to {:?}: {}", src, tmp, e))
    })
}

/// [`copy_atomic`] for bytes the caller already holds. The merge path writes
/// the merged content this way, since there is no source file to copy from.
pub(crate) fn write_atomic(bytes: &[u8], dst: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::write(tmp, bytes).map_err(|e| format!("write {:?}: {}", tmp, e))
    })
}

/// [`write_atomic`], then take the file mode from `mode_of`.
///
/// A merged file replaces one the plugin shipped, so it must end up with the
/// mode a plain install would have given it. `copy_atomic` gets that free
/// (`std::fs::copy` carries the source's permission bits), but `std::fs::write`
/// creates a fresh file at the default mode. Without this, updating an
/// executable plugin script silently drops its executable bit, and only for the
/// files that merged cleanly.
pub(crate) fn write_atomic_like(bytes: &[u8], dst: &Path, mode_of: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::write(tmp, bytes).map_err(|e| format!("write {:?}: {}", tmp, e))?;
        let Ok(meta) = std::fs::metadata(mode_of) else {
            // Shipped file unreadable: the copy path would have failed already,
            // so the default mode is the least surprising outcome here.
            return Ok(());
        };
        // Set on the temp file, so the rename publishes content and mode
        // together and no reader sees one without the other.
        std::fs::set_permissions(tmp, meta.permissions())
            .map_err(|e| format!("set mode on {:?}: {}", tmp, e))
    })
}

/// The git file mode for `path`: `0o100755` when its owner-execute bit is set,
/// `0o100644` otherwise. Git records only those two for a regular file.
pub(crate) fn git_file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o100 != 0)
            .unwrap_or(false);
        if executable {
            return 0o100755;
        }
    }
    0o100644
}

/// Fill a uniquely-named sibling of `dst`, then rename it over `dst`. A rename
/// within one directory is atomic, so a reader sees the old file or the new
/// one, never a half-written one.
fn write_via_tmp(dst: &Path, fill: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    let tmp = dst.with_extension(format!(
        "{}.{}.tmp",
        dst.extension().and_then(|s| s.to_str()).unwrap_or("plugin"),
        uuid::Uuid::new_v4().simple()
    ));
    fill(&tmp)?;
    std::fs::rename(&tmp, dst).map_err(|e| format!("rename {:?} to {:?}: {}", tmp, dst, e))?;
    Ok(())
}
