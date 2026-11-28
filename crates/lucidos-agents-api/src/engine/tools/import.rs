use super::super::LucidosEngine;
use super::bulk_limits::{bulk_threshold_error, BulkContext, MAX_BULK_BYTES, MAX_BULK_FILE_COUNT};
use crate::core::WriteAnnouncement;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::memory::MemorySource;
use chrono::Utc;
use std::path::Path;
use std::time::Duration;

const GIT_CLONE_DESTINATION_REQUIRED: &str =
    "Error: destination is required and must start with one of two prefixes — \
     '.lucidos/tmp/<name>/' (ephemeral, gitignored, the default per best-practices \
     rule 8 for research / inspect / extract-then-discard work) or \
     'data/artifacts/imported/<name>/' (persistent, git-tracked — only when the \
     user has confirmed they want the full repo to live in the workspace). \
     Bare names without a prefix are rejected so a clone never silently lands \
     under data/artifacts/. For bulk reference corpora the user wants to keep \
     but not in the workspace, use `lucidos data-store add <name> <source-dir>` \
     to move into ~/.lucidos/data/<name>/.";

const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(600);

enum GitCloneRoute {
    Artifacts(String),
    Tmp(String),
}

#[allow(clippy::too_many_arguments)]
fn walk_dir(
    dir: &Path,
    base: &Path,
    dest_base: &str,
    include_patterns: &[String],
    exclude_patterns: &[String],
    default_excludes: &[&str],
    collected_files: &mut Vec<(String, String)>,
    imported_files: &mut Vec<String>,
    imported_count: &mut usize,
    skipped_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if *imported_count > MAX_BULK_FILE_COUNT || *total_bytes > MAX_BULK_BYTES {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read dir: {}", e))?;

    for entry in entries.filter_map(|e| e.ok()) {
        if *imported_count > MAX_BULK_FILE_COUNT || *total_bytes > MAX_BULK_BYTES {
            return Ok(());
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            *skipped_count += 1;
            continue;
        };
        if file_type.is_symlink() {
            *skipped_count += 1;
            continue;
        }
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let relative_str = relative.to_string_lossy();

        let is_default_excluded = default_excludes.iter().any(|pattern| {
            if let Some(ext) = pattern.strip_prefix("*.") {
                return path.extension().is_some_and(|e| e.to_string_lossy() == ext);
            }
            let dir = pattern.trim_end_matches("/**");
            relative_str == dir || relative_str.starts_with(&format!("{dir}/"))
        });

        if is_default_excluded {
            *skipped_count += 1;
            continue;
        }

        let is_excluded = !exclude_patterns.is_empty()
            && exclude_patterns
                .iter()
                .any(|pattern| glob::Pattern::new(pattern).is_ok_and(|p| p.matches(&relative_str)));

        if is_excluded {
            *skipped_count += 1;
            continue;
        }

        let is_included = include_patterns.is_empty()
            || include_patterns
                .iter()
                .any(|pattern| glob::Pattern::new(pattern).is_ok_and(|p| p.matches(&relative_str)));

        if !path.is_dir() && !is_included {
            *skipped_count += 1;
            continue;
        }

        if path.is_dir() {
            walk_dir(
                &path,
                base,
                dest_base,
                include_patterns,
                exclude_patterns,
                default_excludes,
                collected_files,
                imported_files,
                imported_count,
                skipped_count,
                total_bytes,
            )?;
        } else if path.is_file() {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let is_binary = crate::core::is_binary_extension(&extension);

            let dest_path = format!("{}/{}", dest_base, relative_str);

            if is_binary {
                *skipped_count += 1;
            } else {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        *total_bytes += content.len() as u64;
                        collected_files.push((dest_path, content));
                        imported_files.push(relative_str.to_string());
                        *imported_count += 1;
                    }
                    Err(_) => {
                        *skipped_count += 1;
                    }
                }
            }
        }
    }
    Ok(())
}

fn import_destination(destination: Option<&str>, filename: &str) -> String {
    let path = destination.map_or(filename, crate::core::artifact_relative_path);
    format!("imported/{}", path.trim_start_matches("imported/"))
}

#[easy_ext::ext(ImportExt)]
pub(crate) impl LucidosEngine {
    async fn execute_import_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        extraction_ctx: &str,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "import_file" => {
                let writer = crate::core::artifact_writer::ArtifactWriterOrigin::Thread(thread_id)
                    .resolve(self.event_bus.pool())
                    .await?;

                let source_path = args["source_path"].as_str().unwrap_or("");

                if source_path.is_empty() {
                    return Ok("Error: source_path is required".to_string());
                }

                let source = std::path::Path::new(source_path);
                let source = &std::path::absolute(source)?;
                crate::runtime::agent_sandbox::private_file_check(self.workspace_path())(source)?;

                if !source.exists() {
                    return Ok(format!("Error: File not found: {}", source_path));
                }
                if !source.is_file() {
                    return Ok(format!("Error: Not a file: {}", source_path));
                }

                if let Ok(meta) = source.metadata() {
                    if meta.len() > MAX_BULK_BYTES {
                        return Ok(bulk_threshold_error(
                            BulkContext::ImportFile { source_path },
                            None,
                            meta.len(),
                        ));
                    }
                }

                let filename = source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("imported_file");

                let dest_arg = args.get("destination").and_then(|v| v.as_str());
                if let Some(d) = dest_arg {
                    if crate::core::is_path_traversal(d) {
                        return Ok(
                            "Error: destination must be relative with no '..' components"
                                .to_string(),
                        );
                    }
                }
                let dest_relative = import_destination(dest_arg, filename);

                let held = self.lock_workspace_repo().await;
                let dest_relative = match self
                    .artifact_manager
                    .resolve_collision_free_path(&dest_relative)
                {
                    Ok(d) => d,
                    Err(e) => return Ok(format!("Error: {}", e)),
                };

                let extension = source
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_binary = crate::core::is_binary_extension(&extension);

                let (summary, _file_size, commit_sha) = if is_binary {
                    let bytes = match std::fs::read(source) {
                        Ok(b) => b,
                        Err(e) => return Ok(format!("Error: Failed to read file: {}", e)),
                    };
                    let size = bytes.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &held,
                            &self.event_bus,
                            &dest_relative,
                            &bytes,
                            &format!("Import {}", dest_relative),
                            WriteAnnouncement::SupersededBy("ArtifactImported"),
                        )
                        .await?;

                    let summary = format!("{} file ({} bytes)", extension.to_uppercase(), size);
                    (Some(summary), size, main_commit)
                } else {
                    let content = match std::fs::read_to_string(source) {
                        Ok(c) => c,
                        Err(e) => {
                            let bytes = match std::fs::read(source) {
                                Ok(b) => b,
                                Err(e2) => {
                                    return Ok(format!("Error: Failed to read file: {}", e2))
                                }
                            };
                            let size = bytes.len();
                            let summary = format!(
                                "Binary file ({} bytes) - could not read as text: {}",
                                size, e
                            );

                            let commit_sha = self
                                .artifact_manager
                                .write_and_commit(
                                    &held,
                                    &self.event_bus,
                                    &dest_relative,
                                    &bytes,
                                    &format!("Import {}", dest_relative),
                                    WriteAnnouncement::SupersededBy("ArtifactImported"),
                                )
                                .await?;
                            self.event_bus
                                .emit(BusEvent::System(SystemEvent::ArtifactImported {
                                    artifact_path: dest_relative.clone(),
                                    source_type: "local_file".into(),
                                    source_detail: source_path.into(),
                                    commit_hash: commit_sha.clone(),
                                    summary: Some(summary.clone()),
                                    writer_thread_id: Some(thread_id),
                                    writer: Some(writer.clone()),
                                }))
                                .await?;
                            let short_sha = &commit_sha[..commit_sha.floor_char_boundary(7)];
                            return Ok(format!("[ACTION COMPLETED] IMPORTED: artifacts/{} (binary, {} bytes, commit: {})", dest_relative, size, short_sha));
                        }
                    };
                    let size = content.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &held,
                            &self.event_bus,
                            &dest_relative,
                            &content,
                            &format!("Import {}", dest_relative),
                            WriteAnnouncement::SupersededBy("ArtifactImported"),
                        )
                        .await?;
                    drop(held);

                    let summary = self
                        .summarize_artifact(&dest_relative, &content, thread_id)
                        .await;
                    (summary, size, main_commit)
                };

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ArtifactImported {
                        artifact_path: dest_relative.clone(),
                        source_type: "local_file".into(),
                        source_detail: source_path.into(),
                        commit_hash: commit_sha.clone(),
                        summary: summary.clone(),
                        writer_thread_id: Some(thread_id),
                        writer: Some(writer.clone()),
                    }))
                    .await?;

                let short_sha = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] IMPORTED: artifacts/{} (from {}, commit: {})",
                    dest_relative, source_path, short_sha
                ))
            }
            "git_clone" => {
                let url = args["url"].as_str().unwrap_or("");
                if url.is_empty() {
                    return Ok("Error: url is required".to_string());
                }

                let local_source = if crate::core::git_auth::is_local_url(url) {
                    let (_, source) = url.split_once("://").expect("a file URL has a scheme");
                    let source = if source.starts_with("localhost/") {
                        &source["localhost".len()..]
                    } else {
                        source
                    };
                    Some(urlencoding::decode(source)?.into_owned())
                } else if url
                    .split(['/', ':'])
                    .next()
                    .is_some_and(|head| !url[head.len()..].starts_with(':'))
                {
                    Some(url.to_owned())
                } else {
                    None
                };
                if let Some(source) = local_source {
                    let root = std::path::absolute(source)?;
                    let effective = lucidos_home::private::effective_paths(self.workspace_path())?;
                    if effective
                        .configured
                        .iter()
                        .any(|path| lucidos_home::private::overlaps(path, &root))
                    {
                        return Ok("Error: Clone source overlaps a private path.".into());
                    }
                    crate::runtime::agent_sandbox::private_file_check(self.workspace_path())(
                        &root,
                    )?;
                }

                let shown_url = crate::core::git_auth::redacted(url);
                let repo_name = shown_url
                    .trim_end_matches(".git")
                    .rsplit('/')
                    .next()
                    .unwrap_or("repo");

                let branch = args
                    .get("branch")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let raw_destination = match args.get("destination").and_then(|v| v.as_str()) {
                    Some(d) if !d.trim().is_empty() => d.trim(),
                    _ => return Ok(GIT_CLONE_DESTINATION_REQUIRED.to_string()),
                };

                if crate::core::is_path_traversal(raw_destination) {
                    return Ok(
                        "Error: destination must be relative with no '..' components".to_string(),
                    );
                }

                let dest_route = if let Some(sub) = raw_destination
                    .strip_prefix(crate::core::TMP_DIR)
                    .and_then(|rest| rest.strip_prefix('/'))
                {
                    GitCloneRoute::Tmp(sub.trim_matches('/').to_string())
                } else if let Some(sub) = raw_destination
                    .strip_prefix("data/artifacts/imported/")
                    .or_else(|| raw_destination.strip_prefix("artifacts/imported/"))
                    .or_else(|| raw_destination.strip_prefix("imported/"))
                {
                    GitCloneRoute::Artifacts(sub.trim_matches('/').to_string())
                } else {
                    return Ok(GIT_CLONE_DESTINATION_REQUIRED.to_string());
                };

                if matches!(&dest_route, GitCloneRoute::Tmp(s) | GitCloneRoute::Artifacts(s) if s.is_empty())
                {
                    return Ok(
                        "Error: destination must include a subdirectory name (got just a prefix)"
                            .to_string(),
                    );
                }

                let include_patterns: Vec<String> = args
                    .get("include_patterns")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let exclude_patterns: Vec<String> = args
                    .get("exclude_patterns")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let credentials =
                    crate::core::git_auth::GitCredentials::resolve_one(&self.pool, url).await;

                let dest_subdir = match dest_route {
                    GitCloneRoute::Tmp(sub) => {
                        let target = self.workspace_path.join(crate::core::TMP_DIR).join(&sub);
                        if let Some(parent) = target.parent() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                return Ok(format!(
                                    "Error: failed to create parent dir {}: {}",
                                    crate::core::home_path::abbreviate(parent),
                                    e
                                ));
                            }
                        }
                        if target.exists() {
                            return Ok(format!(
                                "Error: destination already exists: .lucidos/tmp/{} (delete it first or pick a different name)",
                                sub
                            ));
                        }
                        log_legacy!(@git_clone, "Cloning {} to {:?} (.lucidos/tmp/)", shown_url, target);
                        if let Err(e) = crate::core::git_auth::shallow_clone_within(
                            url,
                            branch.as_deref(),
                            &target,
                            &credentials,
                            GIT_CLONE_TIMEOUT,
                        )
                        .await
                        {
                            return Ok(format!("Error: {}", e));
                        }
                        return Ok(format!(
                            "[ACTION COMPLETED] CLONED TO TMP: {} → .lucidos/tmp/{}/ (ephemeral, gitignored, not indexed). \
                             Extract any files the app actually needs into data/artifacts/imported/<name>/ via copy_file or import_file.",
                            shown_url, sub
                        ));
                    }
                    GitCloneRoute::Artifacts(sub) => sub,
                };

                let scratch = match tempfile::Builder::new().prefix("lucidos-clone-").tempdir() {
                    Ok(dir) => dir,
                    Err(e) => return Ok(format!("Error: cannot create a clone directory: {e}")),
                };
                let temp_dir = scratch.path().join("repo");
                log_legacy!(@git_clone, "Cloning {} to {:?}", shown_url, temp_dir);

                if let Err(e) = crate::core::git_auth::shallow_clone_within(
                    url,
                    branch.as_deref(),
                    &temp_dir,
                    &credentials,
                    GIT_CLONE_TIMEOUT,
                )
                .await
                {
                    return Ok(format!("Error: {}", e));
                }

                let default_excludes = [
                    ".git",
                    ".git/**",
                    "node_modules/**",
                    "__pycache__/**",
                    "*.pyc",
                    ".DS_Store",
                    "target/**",
                    "*.lock",
                    ".env",
                    "dist/**",
                    "build/**",
                    ".next/**",
                    "vendor/**",
                ];

                let mut imported_count = 0;
                let mut skipped_count = 0;
                let mut imported_files: Vec<String> = Vec::new();
                let dest_base = format!("imported/{}", dest_subdir);

                let mut collected_files: Vec<(String, String)> = Vec::new();
                let mut total_bytes: u64 = 0;
                if let Err(e) = walk_dir(
                    &temp_dir,
                    &temp_dir,
                    &dest_base,
                    &include_patterns,
                    &exclude_patterns,
                    &default_excludes,
                    &mut collected_files,
                    &mut imported_files,
                    &mut imported_count,
                    &mut skipped_count,
                    &mut total_bytes,
                ) {
                    return Ok(format!("Error: {}", e));
                }
                drop(scratch);

                if imported_count == 0 {
                    return Ok(format!(
                        "No files imported from {}. {} files skipped due to filters.",
                        shown_url, skipped_count
                    ));
                }

                if imported_count > MAX_BULK_FILE_COUNT || total_bytes > MAX_BULK_BYTES {
                    return Ok(bulk_threshold_error(
                        BulkContext::Clone { url: &shown_url },
                        Some(imported_count),
                        total_bytes,
                    ));
                }

                let commit_msg = format!("Import {} files from {}", imported_count, shown_url);
                let commit_sha = self
                    .artifact_manager
                    .write_batch_and_commit(
                        &self.lock_workspace_repo().await,
                        &collected_files,
                        &commit_msg,
                    )
                    .await?;

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::RepositoryImported {
                        url: shown_url.clone(),
                        branch: branch.unwrap_or_default(),
                        destination: dest_base.clone(),
                        file_count: imported_count,
                        skipped_count,
                        commit: commit_sha.clone(),
                        files: imported_files.iter().take(100).cloned().collect(),
                    }))
                    .await?;

                let summary = format!(
                    "Repository {} cloned: {} files imported to artifacts/{}",
                    repo_name, imported_count, dest_base
                );
                self.index_memory(
                    MemorySource::Artifact {
                        path: dest_base.clone(),
                        commit: commit_sha.clone(),
                    },
                    &summary,
                    Utc::now(),
                    Some(extraction_ctx),
                )
                .await;

                log_legacy!(@git_clone, "Imported {} files, skipped {}", imported_count, skipped_count);
                Ok(format!(
                    "[ACTION COMPLETED] CLONED REPOSITORY: {} files imported to artifacts/{} (skipped {} binary/excluded files, commit: {})",
                    imported_count, dest_base, skipped_count, &commit_sha[..commit_sha.floor_char_boundary(7)]
                ))
            }
            _ => Err(format!("Unknown import tool: {}", name).into()),
        }
    }

    async fn import_file_from_path(
        &self,
        source: &std::path::Path,
        dest_relative: &str,
        writer: crate::engine::event_bus::ArtifactWriter,
    ) -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
        if !source.exists() {
            return Err(format!("File not found: {:?}", source).into());
        }
        if !source.is_file() {
            return Err(format!("Not a file: {:?}", source).into());
        }

        let held = self.lock_workspace_repo().await;
        let dest_relative = self
            .artifact_manager
            .resolve_collision_free_path(dest_relative)?;

        let extension = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_binary = crate::core::is_binary_extension(&extension);

        let commit_sha = if is_binary {
            let bytes = std::fs::read(source)?;
            self.artifact_manager
                .write_and_commit(
                    &held,
                    &self.event_bus,
                    &dest_relative,
                    &bytes,
                    &format!("Import {}", dest_relative),
                    WriteAnnouncement::SupersededBy("ArtifactImported"),
                )
                .await?
        } else {
            let content = std::fs::read_to_string(source)?;
            self.artifact_manager
                .write_and_commit(
                    &held,
                    &self.event_bus,
                    &dest_relative,
                    &content,
                    &format!("Import {}", dest_relative),
                    WriteAnnouncement::SupersededBy("ArtifactImported"),
                )
                .await?
        };
        drop(held);
        log_legacy!(@import, "Committed: {}", &commit_sha[..commit_sha.floor_char_boundary(7)]);

        self.event_bus
            .emit(BusEvent::System(SystemEvent::ArtifactImported {
                artifact_path: dest_relative.clone(),
                source_type: "local_file".into(),
                source_detail: source.to_string_lossy().to_string(),
                commit_hash: commit_sha.clone(),
                summary: None,
                writer_thread_id: None,
                writer: Some(writer),
            }))
            .await?;

        let msg = format!(
            "[ACTION COMPLETED] IMPORTED: artifacts/{} (commit: {})",
            dest_relative,
            &commit_sha[..commit_sha.floor_char_boundary(7)]
        );
        Ok((msg, commit_sha, dest_relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_destination_lands_in_imported_once() {
        for given in [
            "report.pdf",
            "imported/report.pdf",
            "artifacts/imported/report.pdf",
            "artifacts/report.pdf",
        ] {
            assert_eq!(
                import_destination(Some(given), "x.pdf"),
                "imported/report.pdf",
                "{given}"
            );
        }
        assert_eq!(import_destination(None, "x.pdf"), "imported/x.pdf");
    }

    #[test]
    fn git_clone_hands_the_raw_url_only_to_the_clone() {
        let src = include_str!("import.rs");
        let start = src.find("let shown_url =").expect("the git_clone arm");
        let arm = &src[start..src[start..].find("Unknown import tool").unwrap() + start];
        let names_raw_url = |line: &str| {
            line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|token| token == "url")
        };
        let allowed = [
            "GitCredentials::resolve_one(&self.pool, url)",
            "git_auth::redacted(url)",
            "url: shown_url",
            "url: &shown_url",
        ];
        for line in arm.lines().filter(|l| names_raw_url(l)) {
            let trimmed = line.trim();
            assert!(
                trimmed == "url," || allowed.iter().any(|a| trimmed.contains(a)),
                "git_clone passes the raw URL somewhere other than the clone: {trimmed}"
            );
        }
        assert_eq!(
            arm.lines().filter(|l| l.trim() == "url,").count(),
            2,
            "the two shallow_clone_within calls are the only raw-URL arguments"
        );
    }

    fn walk(root: &Path, include: &[String], exclude: &[String]) -> (Vec<String>, usize) {
        let mut collected = Vec::new();
        let mut imported_files = Vec::new();
        let (mut imported, mut skipped, mut bytes) = (0usize, 0usize, 0u64);
        walk_dir(
            root,
            root,
            "imported/x",
            include,
            exclude,
            &[],
            &mut collected,
            &mut imported_files,
            &mut imported,
            &mut skipped,
            &mut bytes,
        )
        .expect("walk");
        imported_files.sort();
        (imported_files, skipped)
    }

    #[test]
    fn a_dir_default_exclude_skips_only_that_directory() {
        let d = tempfile::tempdir().unwrap();
        for dir in [".git", ".github/workflows", "build"] {
            std::fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        for file in [".git/HEAD", ".github/workflows/ci.yml", ".gitignore"] {
            std::fs::write(d.path().join(file), "x").unwrap();
        }
        std::fs::write(d.path().join("build/out.txt"), "x").unwrap();
        std::fs::write(d.path().join("build.rs"), "fn main() {}").unwrap();

        let (mut imported, mut skipped, mut bytes) = (0usize, 0usize, 0u64);
        let (mut collected, mut files) = (Vec::new(), Vec::new());
        walk_dir(
            d.path(),
            d.path(),
            "imported/x",
            &[],
            &[],
            &[".git/**", "build/**"],
            &mut collected,
            &mut files,
            &mut imported,
            &mut skipped,
            &mut bytes,
        )
        .expect("walk");
        files.sort();
        assert_eq!(
            files,
            vec![".github/workflows/ci.yml", ".gitignore", "build.rs"]
        );
        assert_eq!(skipped, 2, "only .git/ and build/ are skipped");
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
        std::fs::create_dir_all(d.path().join("vendor")).unwrap();
        std::fs::write(d.path().join("top.py"), "print(1)").unwrap();
        std::fs::write(d.path().join("src/a.rs"), "fn a() {}").unwrap();
        std::fs::write(d.path().join("src/deep/b.rs"), "fn b() {}").unwrap();
        std::fs::write(d.path().join("vendor/v.rs"), "fn v() {}").unwrap();
        d
    }

    #[test]
    fn include_patterns_select_files_without_pruning_the_directories_holding_them() {
        let d = tree();
        let include = vec!["*.py".to_string(), "src/**/*.rs".to_string()];
        let (files, _) = walk(d.path(), &include, &[]);
        assert_eq!(
            files,
            vec![
                "src/a.rs".to_string(),
                "src/deep/b.rs".to_string(),
                "top.py".to_string()
            ],
            "an include list must reach files nested under directories that match no file glob"
        );
    }

    #[test]
    fn exclude_patterns_still_prune_a_subtree() {
        let d = tree();
        let (files, _) = walk(d.path(), &[], &["vendor/**".to_string()]);
        assert!(
            !files.iter().any(|f| f.starts_with("vendor/")),
            "got {files:?}"
        );
        assert!(files.contains(&"src/a.rs".to_string()), "got {files:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_a_cloned_tree_is_skipped_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "host file").unwrap();
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("real.txt"), "kept").unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            d.path().join("file_link"),
        )
        .unwrap();

        let (files, skipped) = walk(d.path(), &[], &[]);
        assert_eq!(files, vec!["real.txt".to_string()]);
        assert_eq!(skipped, 2, "both links are skipped");
    }

    #[tokio::test]
    async fn import_file_from_path_resolves_its_destination_under_the_repo_lock() {
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };

        if !in_engine_child(
            "engine::tools::import::tests::import_file_from_path_resolves_its_destination_under_the_repo_lock",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("c.txt");
        std::fs::write(&source, "incoming").unwrap();

        let held = engine.lock_workspace_repo().await;
        let importer = engine.clone();
        let task = tokio::spawn(async move {
            importer
                .import_file_from_path(
                    &source,
                    "imported/c.txt",
                    crate::engine::event_bus::ArtifactWriter::Person,
                )
                .await
                .map_err(|e| e.to_string())
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let taken = engine
            .workspace_path()
            .join(crate::core::ARTIFACTS_DIR)
            .join("imported/c.txt");
        std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
        std::fs::write(&taken, "someone else").unwrap();
        drop(held);

        let (_, _, resolved) = task.await.unwrap().expect("the import succeeds");
        assert_ne!(
            resolved, "imported/c.txt",
            "the import took a name already in use"
        );
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "someone else");

        engine.pool().close().await;
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_import_file_tool_resolves_its_destination_under_the_repo_lock() {
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };

        if !in_engine_child(
            "engine::tools::import::tests::the_import_file_tool_resolves_its_destination_under_the_repo_lock",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("d.png");
        std::fs::write(&source, [0x89, b'P', b'N', b'G']).unwrap();

        let held = engine.lock_workspace_repo().await;
        let importer = engine.clone();
        let args = serde_json::json!({
            "source_path": source.to_string_lossy(),
            "destination": "imported/d.png",
        });
        let task = tokio::spawn(async move {
            importer
                .execute_import_tool("import_file", &args, "", uuid::Uuid::new_v4())
                .await
                .map_err(|e| e.to_string())
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let taken = engine
            .workspace_path()
            .join(crate::core::ARTIFACTS_DIR)
            .join("imported/d.png");
        std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
        std::fs::write(&taken, "someone else").unwrap();
        drop(held);

        let reply = task.await.unwrap().expect("the import succeeds");
        assert!(reply.contains("d (1).png"), "{reply}");
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "someone else");

        engine.pool().close().await;
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn an_empty_include_list_imports_the_whole_tree() {
        let d = tree();
        let (files, _) = walk(d.path(), &[], &[]);
        assert_eq!(files.len(), 4, "got {files:?}");
    }
}
