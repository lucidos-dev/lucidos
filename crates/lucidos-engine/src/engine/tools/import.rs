use super::super::LucidosEngine;
use super::bulk_limits::{bulk_threshold_error, BulkContext, MAX_BULK_BYTES, MAX_BULK_FILE_COUNT};
use crate::core::WriteAnnouncement;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::memory::MemorySource;
use chrono::Utc;
use std::path::Path;
use uuid::Uuid;

/// Standard refusal text when `git_clone` is called without a destination, or
/// with one that lacks a recognized prefix. Per best-practices rule 8, the
/// agent must pick a route explicitly — there is no default and no fallback.
/// Lives next to the route enum so the wire message can't drift from the
/// tool description in `llm/tools/misc.rs`.
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

/// Where a `git_clone` invocation should land. Decided by parsing the
/// LLM-provided `destination` parameter. Variants carry the resolved
/// subdirectory name so the dispatch site doesn't need a second match.
enum GitCloneRoute {
    Artifacts(String),
    Tmp(String),
}

// Inner recursive helper: every parameter is per-recursion-frame state
// (filters + four mutable counters); flattening into a struct would just
// wrap the same set without simplifying the call sites below.
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
    // Bail mid-walk once the bulk threshold trips so a 1 GB clone
    // doesn't read 1 GB of UTF-8 into memory before the outer site
    // refuses. The outer site re-checks the same predicate.
    if *imported_count > MAX_BULK_FILE_COUNT || *total_bytes > MAX_BULK_BYTES {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read dir: {}", e))?;

    for entry in entries.filter_map(|e| e.ok()) {
        if *imported_count > MAX_BULK_FILE_COUNT || *total_bytes > MAX_BULK_BYTES {
            return Ok(());
        }
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let relative_str = relative.to_string_lossy();

        let is_default_excluded = default_excludes.iter().any(|pattern| {
            if pattern.contains("**") {
                let prefix = pattern.trim_end_matches("/**");
                relative_str.starts_with(prefix)
            } else if pattern.starts_with("*.") {
                let ext = pattern.trim_start_matches("*.");
                path.extension().is_some_and(|e| e.to_string_lossy() == ext)
            } else {
                relative_str == *pattern || relative_str.starts_with(&format!("{}/", pattern))
            }
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

        // FILES only. `include_patterns` selects files to import,
        // and a directory essentially never matches a file glob
        // (`glob::Pattern::new("src/**/*.rs").matches("src")` is
        // false, and so is `"*.py"` against `src`). Pruning a
        // directory here would skip the recursion below, so any
        // non-empty `include_patterns` imported only top-level
        // matches: the tool schema's own example
        // `['*.py', 'src/**/*.rs']` imported nothing at all.
        // Subtree pruning is `exclude_patterns`' job, above.
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
                // Binary files aren't imported here: write_batch_and_commit
                // is text-only and committing arbitrary binaries by default
                // would bloat the repo.
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
                        // Not valid UTF-8, same reason as binary above.
                        *skipped_count += 1;
                    }
                }
            }
        }
    }
    Ok(())
}

impl LucidosEngine {
    pub(crate) async fn execute_import_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        extraction_ctx: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "import_file" => {
                let source_path = args["source_path"].as_str().unwrap_or("");

                if source_path.is_empty() {
                    return Ok("Error: source_path is required".to_string());
                }

                let source = std::path::Path::new(source_path);

                // Validate source exists and is a file
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

                // Determine destination path
                let filename = source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("imported_file");

                let dest_arg = args.get("destination").and_then(|v| v.as_str());
                if let Some(d) = dest_arg {
                    if crate::api::is_path_traversal(d) {
                        return Ok(
                            "Error: destination must be relative with no '..' components"
                                .to_string(),
                        );
                    }
                }
                let dest_relative = dest_arg
                    .map(|s| format!("imported/{}", s.trim_start_matches("imported/")))
                    .unwrap_or_else(|| format!("imported/{}", filename));

                // Finder-style auto-suffix on collision: never overwrite an
                // existing artifact. Resolve before the binary/text branch so
                // every write/commit/event below uses the same final path.
                let dest_relative = match self
                    .artifact_manager
                    .resolve_collision_free_path(&dest_relative)
                {
                    Ok(d) => d,
                    Err(e) => return Ok(format!("Error: {}", e)),
                };

                // Check if file is binary by extension
                let extension = source
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_binary = crate::core::is_binary_extension(&extension);

                let (summary, _file_size, commit_sha) = if is_binary {
                    // Other binary file: copy as bytes
                    let bytes = match std::fs::read(source) {
                        Ok(b) => b,
                        Err(e) => return Ok(format!("Error: Failed to read file: {}", e)),
                    };
                    let size = bytes.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &self.event_bus,
                            &dest_relative,
                            &bytes,
                            &format!("Import {}", dest_relative),
                            WriteAnnouncement::SupersededBy("ArtifactImported"),
                        )
                        .await?;

                    // Metadata-only summary for binary files
                    let summary = format!("{} file ({} bytes)", extension.to_uppercase(), size);
                    (Some(summary), size, main_commit)
                } else {
                    // Text file: read as string
                    let content = match std::fs::read_to_string(source) {
                        Ok(c) => c,
                        Err(e) => {
                            // Fallback: try as binary if text read fails
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
                            &self.event_bus,
                            &dest_relative,
                            &content,
                            &format!("Import {}", dest_relative),
                            WriteAnnouncement::SupersededBy("ArtifactImported"),
                        )
                        .await?;

                    // Generate summary for text files
                    let summary = self.summarize_artifact(&dest_relative, &content).await;
                    (summary, size, main_commit)
                };

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ArtifactImported {
                        artifact_path: dest_relative.clone(),
                        source_type: "local_file".into(),
                        source_detail: source_path.into(),
                        commit_hash: commit_sha.clone(),
                        summary: summary.clone(),
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

                // Parse repo name from URL
                let repo_name = url
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

                if crate::api::is_path_traversal(raw_destination) {
                    return Ok(
                        "Error: destination must be relative with no '..' components".to_string(),
                    );
                }

                // Per best-practices rule 8, the agent must explicitly pick the
                // route — bare names are rejected so we never silently land a
                // clone under `data/artifacts/`.
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
                        log!(@git_clone, "Cloning {} to {:?} (.lucidos/tmp/)", url, target);
                        {
                            let mut builder = git2::build::RepoBuilder::new();
                            let mut fetch_opts = git2::FetchOptions::new();
                            fetch_opts.depth(1);
                            if let Some(ref b) = branch {
                                builder.branch(b);
                            }
                            builder.fetch_options(fetch_opts);
                            if let Err(e) = builder.clone(url, &target) {
                                let _ = std::fs::remove_dir_all(&target);
                                return Ok(format!("Error: Failed to clone repository: {}", e));
                            }
                        }
                        return Ok(format!(
                            "[ACTION COMPLETED] CLONED TO TMP: {} → .lucidos/tmp/{}/ (ephemeral, gitignored, not indexed). \
                             Extract any files the app actually needs into data/artifacts/imported/<name>/ via copy_file or import_file.",
                            url, sub
                        ));
                    }
                    GitCloneRoute::Artifacts(sub) => sub,
                };

                // Clone to temp directory - wrapped in block so git2 objects are dropped before async ops
                let temp_dir =
                    std::env::temp_dir().join(format!("lucidos_clone_{}", Uuid::new_v4()));
                log!(@git_clone, "Cloning {} to {:?}", url, temp_dir);

                {
                    // Build clone options - these types are not Send so must be dropped before await
                    let mut builder = git2::build::RepoBuilder::new();
                    let mut fetch_opts = git2::FetchOptions::new();
                    fetch_opts.depth(1); // Shallow clone for speed

                    if let Some(ref b) = branch {
                        builder.branch(b);
                    }
                    builder.fetch_options(fetch_opts);

                    if let Err(e) = builder.clone(url, &temp_dir) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        return Ok(format!("Error: Failed to clone repository: {}", e));
                    }
                    // builder and fetch_opts dropped here
                }

                // Default exclusions
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

                // Walk the cloned repo and import files
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
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Ok(format!("Error: {}", e));
                }

                let _ = std::fs::remove_dir_all(&temp_dir);

                if imported_count == 0 {
                    return Ok(format!(
                        "No files imported from {}. {} files skipped due to filters.",
                        url, skipped_count
                    ));
                }

                if imported_count > MAX_BULK_FILE_COUNT || total_bytes > MAX_BULK_BYTES {
                    return Ok(bulk_threshold_error(
                        BulkContext::Clone { url },
                        Some(imported_count),
                        total_bytes,
                    ));
                }

                // Write all files and commit in one operation
                let commit_msg = format!("Import {} files from {}", imported_count, url);
                let commit_sha = self
                    .artifact_manager
                    .write_batch_and_commit(&collected_files, &commit_msg)
                    .await?;

                // Emit event for the batch import
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::RepositoryImported {
                        url: url.to_string(),
                        branch: branch.unwrap_or_default(),
                        destination: dest_base.clone(),
                        file_count: imported_count,
                        skipped_count,
                        commit: commit_sha.clone(),
                        files: imported_files.iter().take(100).cloned().collect(),
                    }))
                    .await?;

                // Index in memory
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

                log!(@git_clone, "Imported {} files, skipped {}", imported_count, skipped_count);
                Ok(format!(
                    "[ACTION COMPLETED] CLONED REPOSITORY: {} files imported to artifacts/{} (skipped {} binary/excluded files, commit: {})",
                    imported_count, dest_base, skipped_count, &commit_sha[..commit_sha.floor_char_boundary(7)]
                ))
            }
            _ => Ok(format!("Unknown import tool: {}", name)),
        }
    }

    /// Import a file from a given path (used by upload API)
    /// Import a file into the artifact store (write + git commit + event).
    /// Returns `(result_message, commit_sha, resolved_dest_relative)` — the
    /// resolved path can differ from the requested one when a Finder-style
    /// auto-suffix was applied to avoid overwriting an existing artifact.
    pub async fn import_file_from_path(
        &self,
        source: &std::path::Path,
        dest_relative: &str,
    ) -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
        // Check file exists
        if !source.exists() {
            return Err(format!("File not found: {:?}", source).into());
        }
        if !source.is_file() {
            return Err(format!("Not a file: {:?}", source).into());
        }

        // Finder-style auto-suffix on collision: never overwrite an existing
        // artifact. Resolve once up front so the write/commit, the commit
        // message, the event, and the returned message all use the final path.
        let dest_relative = self
            .artifact_manager
            .resolve_collision_free_path(dest_relative)?;

        // Determine if binary by extension
        let extension = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_binary = crate::core::is_binary_extension(&extension);

        // Phase 1: Write file to artifact store + git commit (synchronous)
        let commit_sha = if is_binary {
            let bytes = std::fs::read(source)?;
            self.artifact_manager
                .write_and_commit(
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
                    &self.event_bus,
                    &dest_relative,
                    &content,
                    &format!("Import {}", dest_relative),
                    WriteAnnouncement::SupersededBy("ArtifactImported"),
                )
                .await?
        };
        log!(@import, "Committed: {}", &commit_sha[..commit_sha.floor_char_boundary(7)]);

        self.event_bus
            .emit(BusEvent::System(SystemEvent::ArtifactImported {
                artifact_path: dest_relative.clone(),
                source_type: "local_file".into(),
                source_detail: source.to_string_lossy().to_string(),
                commit_hash: commit_sha.clone(),
                summary: None,
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

    /// `walk_dir` was a nested `fn` inside `git_clone`'s match arm, which is why
    /// the bug below shipped untested. It is lifted to module level (a pure move,
    /// it captured nothing) so the recursion can be exercised directly.
    #[allow(clippy::too_many_arguments)]
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

    /// Regression: `include_patterns` was matched against DIRECTORY entries and a
    /// non-match `continue`d BEFORE the `path.is_dir()` recursion, so a non-empty
    /// include list pruned whole subtrees. `Pattern::new("src/**/*.rs")` does not
    /// match the bare directory `src`, so the tool schema's own example
    /// `['*.py', 'src/**/*.rs']` reached only TOP-LEVEL matches: on the tree
    /// below it returned `["top.py"]` and lost both nested `.rs` files, and on a
    /// repo whose every match is nested (the ordinary shape for that example) it
    /// imported nothing at all.
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

    /// The other half of the contract: pruning a subtree is `exclude_patterns`'
    /// job, and lifting the directory check out of the include gate must not have
    /// disturbed it.
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

    /// An empty include list imports everything, the pre-existing behaviour that
    /// masked the bug (the buggy gate was a no-op when `is_included` was always true).
    #[test]
    fn an_empty_include_list_imports_the_whole_tree() {
        let d = tree();
        let (files, _) = walk(d.path(), &[], &[]);
        assert_eq!(files.len(), 4, "got {files:?}");
    }
}
