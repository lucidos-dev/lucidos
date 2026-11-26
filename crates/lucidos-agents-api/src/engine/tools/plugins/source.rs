use crate::engine::pending_plugins::SourceType;
use sqlx::PgPool;
use std::path::{Path, PathBuf};

use crate::core::git_auth::GitCredentials;
use crate::core::plugins::{
    is_git_url, validate_archive_entry_path, ValidationError, PLUGIN_ARCHIVE_EXT,
};

#[derive(Debug)]
pub(crate) enum Source {
    Git {
        url: String,
        branch: Option<String>,
        subpath: Option<String>,
    },
    Archive(PathBuf),
}

pub(crate) fn detect_source(s: &str) -> Result<Source, String> {
    let trimmed = s.trim();

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

pub(crate) async fn credentials_for_source(pool: &PgPool, source_str: &str) -> GitCredentials {
    match detect_source(source_str) {
        Ok(Source::Git { url, .. }) => GitCredentials::resolve_one(pool, &url).await,
        _ => GitCredentials::none(),
    }
}

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
            crate::core::git_auth::shallow_clone(
                url,
                branch.as_deref(),
                &clone_target,
                credentials,
            )?;
            let _ = std::fs::remove_dir_all(clone_target.join(".git"));

            if let Some(sub) = subpath
                .as_deref()
                .filter(|s| crate::core::is_path_traversal(s))
            {
                return Err(format!(
                    "rejected plugin subpath '{}': must be relative with no '..', \
                     leading '/' or leading '\\'",
                    sub
                ));
            }
            let plugin_root =
                crate::core::plugins::clone_subpath_root(&clone_target, subpath.as_deref())?;
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
            .by_index_with_options(
                i,
                zip::read::ZipReadOptions::new().enforce_header_consistency(true),
            )
            .map_err(|e| format!("zip entry {}: {}", i, e))?;
        let raw_name = entry
            .name()
            .map_err(|e| format!("zip entry {} name: {}", i, e))?
            .into_owned();
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

pub(crate) fn copy_atomic(src: &Path, dst: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::copy(src, tmp)
            .map(|_| ())
            .map_err(|e| format!("copy {:?} to {:?}: {}", src, tmp, e))
    })
}

pub(crate) fn write_atomic(bytes: &[u8], dst: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::write(tmp, bytes).map_err(|e| format!("write {:?}: {}", tmp, e))
    })
}

pub(crate) fn write_atomic_like(bytes: &[u8], dst: &Path, mode_of: &Path) -> Result<(), String> {
    write_via_tmp(dst, |tmp| {
        std::fs::write(tmp, bytes).map_err(|e| format!("write {:?}: {}", tmp, e))?;
        let Ok(meta) = std::fs::metadata(mode_of) else {
            return Ok(());
        };
        std::fs::set_permissions(tmp, meta.permissions())
            .map_err(|e| format!("set mode on {:?}: {}", tmp, e))
    })
}

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
