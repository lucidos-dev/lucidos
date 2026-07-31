//! Plugin install-source detection and fetching: classify a source string
//! (git URL / GitHub tree URL / local archive), clone or unzip it into a
//! scratch dir, and the per-file atomic copy used by the writer. Pure
//! filesystem/network helpers — no engine or event-bus coupling.

use std::path::{Path, PathBuf};

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

/// Fetch the source into a fresh `tempfile::TempDir` under
/// `.lucidos/tmp/plugins/`. Returns the TempDir guard (auto-cleans on drop)
/// and the plugin root inside it (where `manifest.toml` lives — for git-tree
/// URLs this is the subpath, not the repo root).
pub(crate) fn fetch_source(
    workspace: &Path,
    source: &Source,
) -> Result<(tempfile::TempDir, PathBuf, SourceType), String> {
    let parent = workspace.join(".lucidos").join("tmp").join("plugins");
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
            // git2 types are not Send — drop them before any await in the caller.
            {
                let mut builder = git2::build::RepoBuilder::new();
                let mut fetch_opts = git2::FetchOptions::new();
                // Shallow clones cut bandwidth for HTTPS/SSH but libgit2's
                // local transport rejects them ("shallow fetch is not
                // supported by the local transport"). Fall back to a full
                // clone for `file://` URLs — they're already local so the
                // bandwidth saving doesn't matter.
                if !url.starts_with("file://") {
                    fetch_opts.depth(1);
                }
                if let Some(b) = branch {
                    builder.branch(b);
                }
                builder.fetch_options(fetch_opts);
                builder
                    .clone(url, &clone_target)
                    .map_err(|e| format!("git clone failed: {}", e))?;
            }
            let _ = std::fs::remove_dir_all(clone_target.join(".git"));

            let plugin_root = match subpath {
                Some(sub) => clone_target.join(sub),
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
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    let tmp = dst.with_extension(format!(
        "{}.{}.tmp",
        dst.extension().and_then(|s| s.to_str()).unwrap_or("plugin"),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::copy(src, &tmp).map_err(|e| format!("copy {:?} → {:?}: {}", src, tmp, e))?;
    std::fs::rename(&tmp, dst).map_err(|e| format!("rename {:?} → {:?}: {}", tmp, dst, e))?;
    Ok(())
}
