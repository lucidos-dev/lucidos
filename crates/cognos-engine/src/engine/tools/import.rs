use super::super::document::{extract_text_with_ocr, safe_extract_pdf_text};
use super::super::CognosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::memory::MemorySource;
use chrono::Utc;
use std::path::Path;
use uuid::Uuid;

impl CognosEngine {
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

                // Determine destination path
                let filename = source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("imported_file");

                let dest_relative = args
                    .get("destination")
                    .and_then(|v| v.as_str())
                    .map(|s| format!("imported/{}", s.trim_start_matches("imported/")))
                    .unwrap_or_else(|| format!("imported/{}", filename));

                // Check if file is binary by extension
                let extension = source
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_binary = crate::core::is_binary_extension(&extension);

                let (summary, _file_size, commit_sha) = if extension == "pdf" {
                    // PDF file: copy as bytes and extract text for indexing
                    let bytes = match std::fs::read(source) {
                        Ok(b) => b,
                        Err(e) => return Ok(format!("Error: Failed to read file: {}", e)),
                    };
                    let size = bytes.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &dest_relative,
                            &bytes,
                            &format!("Import {}", dest_relative),
                        )
                        .await?;

                    // Extract text from PDF for summarization
                    // Try pdf_extract first (for text-based PDFs), fall back to OCR (for scanned)
                    let extracted_text = match safe_extract_pdf_text(source) {
                        Ok(text) if !text.trim().is_empty() => {
                            log!("PDF text extracted successfully for {:?}", source);
                            Some(text)
                        }
                        Ok(_) | Err(_) => {
                            log!("PDF text extraction failed, trying OCR for {:?}", source);
                            match extract_text_with_ocr(source) {
                                Ok(text) => {
                                    log!("OCR successful for {:?}", source);
                                    Some(text)
                                }
                                Err(e) => {
                                    log!("OCR failed for {:?}: {}", source, e);
                                    None
                                }
                            }
                        }
                    };

                    let summary = if let Some(text) = &extracted_text {
                        // Save extracted text as sidecar file for fast future access
                        let text_path = format!("{}.txt", dest_relative);
                        if let Err(e) = self
                            .artifact_manager
                            .write_and_commit(
                                &text_path,
                                text,
                                &format!("Extract text from {}", dest_relative),
                            )
                            .await
                        {
                            log!("Warning: Failed to write/commit extracted text: {}", e);
                        }

                        // Generate summary from extracted text
                        self.summarize_artifact(&dest_relative, text)
                            .await
                            .unwrap_or_else(|| format!("PDF document ({} bytes)", size))
                    } else {
                        format!("PDF document ({} bytes) - could not extract text", size)
                    };
                    (Some(summary), size, main_commit)
                } else if is_binary {
                    // Other binary file: copy as bytes
                    let bytes = match std::fs::read(source) {
                        Ok(b) => b,
                        Err(e) => return Ok(format!("Error: Failed to read file: {}", e)),
                    };
                    let size = bytes.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &dest_relative,
                            &bytes,
                            &format!("Import {}", dest_relative),
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
                                    &dest_relative,
                                    &bytes,
                                    &format!("Import {}", dest_relative),
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
                            self.index_memory(
                                MemorySource::Artifact {
                                    path: dest_relative.clone(),
                                    commit: commit_sha.clone(),
                                },
                                &format!("Imported file: {}\n{}", dest_relative, summary),
                                Utc::now(),
                                Some(extraction_ctx),
                            )
                            .await;

                            let short_sha = &commit_sha[..commit_sha.floor_char_boundary(7)];
                            return Ok(format!("[ACTION COMPLETED] IMPORTED: artifacts/{} (binary, {} bytes, commit: {})", dest_relative, size, short_sha));
                        }
                    };
                    let size = content.len();
                    let main_commit = self
                        .artifact_manager
                        .write_and_commit(
                            &dest_relative,
                            &content,
                            &format!("Import {}", dest_relative),
                        )
                        .await?;

                    // Generate summary for text files
                    let summary = self.summarize_artifact(&dest_relative, &content).await;
                    (summary, size, main_commit)
                };

                // Emit ArtifactImported event
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ArtifactImported {
                        artifact_path: dest_relative.clone(),
                        source_type: "local_file".into(),
                        source_detail: source_path.into(),
                        commit_hash: commit_sha.clone(),
                        summary: summary.clone(),
                    }))
                    .await?;

                // Index in memory - send raw content to Flash for fact extraction
                if let Ok(content) = self.artifact_manager.read_artifact(&dest_relative) {
                    self.index_artifact_memory(
                        &dest_relative,
                        &content,
                        &commit_sha,
                        Utc::now(),
                        Some(extraction_ctx),
                    )
                    .await;
                } else {
                    // Fallback: index summary if content isn't readable
                    let fallback = if let Some(ref s) = summary {
                        format!("Imported file: {}\n{}", dest_relative, s)
                    } else {
                        format!("Imported file: {}", dest_relative)
                    };
                    self.index_memory(
                        MemorySource::Artifact {
                            path: dest_relative.clone(),
                            commit: commit_sha.clone(),
                        },
                        &fallback,
                        Utc::now(),
                        Some(extraction_ctx),
                    )
                    .await;
                }

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

                let dest_subdir = args
                    .get("destination")
                    .and_then(|v| v.as_str())
                    .unwrap_or(repo_name);

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

                // Clone to temp directory - wrapped in block so git2 objects are dropped before async ops
                let temp_dir =
                    std::env::temp_dir().join(format!("cognos_clone_{}", Uuid::new_v4()));
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
                ) -> Result<(), String> {
                    let entries =
                        std::fs::read_dir(dir).map_err(|e| format!("Failed to read dir: {}", e))?;

                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        let relative = path.strip_prefix(base).unwrap_or(&path);
                        let relative_str = relative.to_string_lossy();

                        // Check default exclusions first
                        let is_default_excluded = default_excludes.iter().any(|pattern| {
                            if pattern.contains("**") {
                                let prefix = pattern.trim_end_matches("/**");
                                relative_str.starts_with(prefix)
                            } else if pattern.starts_with("*.") {
                                let ext = pattern.trim_start_matches("*.");
                                path.extension().is_some_and(|e| e.to_string_lossy() == ext)
                            } else {
                                relative_str == *pattern
                                    || relative_str.starts_with(&format!("{}/", pattern))
                            }
                        });

                        if is_default_excluded {
                            *skipped_count += 1;
                            continue;
                        }

                        // Check user exclude patterns
                        let is_excluded = !exclude_patterns.is_empty()
                            && exclude_patterns.iter().any(|pattern| {
                                glob::Pattern::new(pattern).is_ok_and(|p| p.matches(&relative_str))
                            });

                        if is_excluded {
                            *skipped_count += 1;
                            continue;
                        }

                        // Check user include patterns (if specified, file must match at least one)
                        let is_included = include_patterns.is_empty()
                            || include_patterns.iter().any(|pattern| {
                                glob::Pattern::new(pattern).is_ok_and(|p| p.matches(&relative_str))
                            });

                        if !is_included {
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
                            )?;
                        } else if path.is_file() {
                            // Check if it's a binary file by extension
                            let extension = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_lowercase();
                            let is_binary = crate::core::is_binary_extension(&extension);

                            let dest_path = format!("{}/{}", dest_base, relative_str);

                            if is_binary {
                                // Skip binary files (can be added later if needed)
                                *skipped_count += 1;
                            } else {
                                // Read text file and collect for batch write
                                match std::fs::read_to_string(&path) {
                                    Ok(content) => {
                                        collected_files.push((dest_path, content));
                                        imported_files.push(relative_str.to_string());
                                        *imported_count += 1;
                                    }
                                    Err(_) => {
                                        // Not valid UTF-8, skip
                                        *skipped_count += 1;
                                    }
                                }
                            }
                        }
                    }
                    Ok(())
                }

                let mut collected_files: Vec<(String, String)> = Vec::new();
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
                ) {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Ok(format!("Error: {}", e));
                }

                // Clean up temp directory
                let _ = std::fs::remove_dir_all(&temp_dir);

                if imported_count == 0 {
                    return Ok(format!(
                        "No files imported from {}. {} files skipped due to filters.",
                        url, skipped_count
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
    /// Returns `(result_message, commit_sha)`.
    pub async fn import_file_from_path(
        &self,
        source: &std::path::Path,
        dest_relative: &str,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        // Check file exists
        if !source.exists() {
            return Err(format!("File not found: {:?}", source).into());
        }
        if !source.is_file() {
            return Err(format!("Not a file: {:?}", source).into());
        }

        // Determine if binary by extension
        let extension = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_binary = crate::core::is_binary_extension(&extension);

        // Phase 1: Write file to artifact store + git commit (synchronous)
        let commit_sha = if is_binary || extension == "pdf" {
            let bytes = std::fs::read(source)?;
            self.artifact_manager
                .write_and_commit(dest_relative, &bytes, &format!("Import {}", dest_relative))
                .await?
        } else {
            let content = std::fs::read_to_string(source)?;
            self.artifact_manager
                .write_and_commit(
                    dest_relative,
                    &content,
                    &format!("Import {}", dest_relative),
                )
                .await?
        };
        log!(@import, "Committed: {}", &commit_sha[..commit_sha.floor_char_boundary(7)]);

        self.event_bus
            .emit(BusEvent::System(SystemEvent::ArtifactImported {
                artifact_path: dest_relative.to_string(),
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
        Ok((msg, commit_sha))
    }
}
