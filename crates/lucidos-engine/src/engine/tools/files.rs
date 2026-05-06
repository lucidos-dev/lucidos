use super::super::context::sanitize_file_content_for_llm;
use super::super::document::{extract_text_with_ocr, safe_extract_pdf_text};
use super::super::LucidosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use base64::Engine as _;
use chrono::Utc;

const IMAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Why a write to `data_path` should be rejected, or `None` if it's allowed.
/// Centralized so all write tools share the same policy and message.
fn read_only_reason(data_path: &str) -> Option<&'static str> {
    crate::core::is_system_knowhow_path(data_path)
        .then_some("system knowhow is read-only (shipped with the engine)")
}

/// Convert dot-bracket notation (e.g. `sections[1].slides[0].title`)
/// to a JSON Pointer string (e.g. `/sections/1/slides/0/title`).
/// If the input already starts with `/`, return it as-is (assume JSON Pointer).
/// Empty string returns empty string (targets root).
pub(crate) fn dot_path_to_pointer(path: &str) -> String {
    if path.is_empty() || path.starts_with('/') {
        return path.to_string();
    }
    let mut pointer = String::new();
    for segment in path.split('.') {
        if let Some(bracket_pos) = segment.find('[') {
            // e.g. "sections[1]" → "/sections/1"
            pointer.push('/');
            pointer.push_str(&segment[..segment.floor_char_boundary(bracket_pos)]);
            // Handle one or more bracket indices: "a[1][2]" → "/a/1/2"
            let rest = &segment[segment.floor_char_boundary(bracket_pos)..];
            for part in rest.split('[') {
                if part.is_empty() {
                    continue;
                }
                let idx = part.trim_end_matches(']');
                pointer.push('/');
                pointer.push_str(idx);
            }
        } else {
            pointer.push('/');
            pointer.push_str(segment);
        }
    }
    pointer
}

/// Set a value at the given JSON Pointer path in a parsed JSON document.
/// Empty pointer replaces the root. Returns Err if the path doesn't exist.
pub(crate) fn json_set_value(
    doc: &mut serde_json::Value,
    pointer: &str,
    new_value: serde_json::Value,
) -> Result<(), String> {
    if pointer.is_empty() {
        *doc = new_value;
        return Ok(());
    }
    match doc.pointer_mut(pointer) {
        Some(target) => {
            *target = new_value;
            Ok(())
        }
        None => Err(format!("JSON path '{}' not found in document", pointer)),
    }
}

impl LucidosEngine {
    /// Commit a file change via the appropriate manager: shared user dir, app, or workspace artifact.
    async fn commit_file_change(
        &self,
        data_path: &str,
        full_path: &std::path::Path,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ud) = self.user_dir() {
            if full_path.starts_with(ud) {
                let rel = full_path.strip_prefix(ud).unwrap();
                crate::core::user_dir::auto_commit(ud, &rel.to_string_lossy(), message);
                return Ok("shared".to_string());
            }
        }
        if let Some(app_path) = data_path.strip_prefix("apps/") {
            Ok(self.app_manager.commit(app_path, message)?)
        } else {
            Ok(self
                .artifact_manager
                .commit_data_path(data_path, message)
                .await?)
        }
    }
}

pub(crate) struct FileEditResult {
    pub path: String,
    pub commit: String,
}

impl LucidosEngine {
    /// Edit a file at the given data-relative path and commit the change.
    /// Supports JSON mode (json_path + new_value) and text mode (old_string + new_string).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn edit_file_at_path(
        &self,
        raw_path: &str,
        json_path: Option<&str>,
        new_value: Option<serde_json::Value>,
        old_string: Option<&str>,
        new_string: Option<&str>,
        replace_all: bool,
        message: Option<&str>,
        extraction_ctx: Option<&str>,
    ) -> Result<FileEditResult, String> {
        let (data_path, full_path) = self.resolve_data_path(raw_path)?;
        let path = data_path.as_str();
        if let Some(reason) = read_only_reason(path) {
            return Err(format!("Cannot edit '{}': {}", path, reason));
        }

        let content = std::fs::read_to_string(&full_path)
            .map_err(|_| format!("File '{}' not found", path))?;

        let new_content = if let Some(jp) = json_path {
            let nv = new_value.ok_or("json_path requires new_value parameter")?;
            let mut doc: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| format!("File '{}' is not valid JSON: {}", path, e))?;
            let pointer = dot_path_to_pointer(jp);
            if let Err(e) = json_set_value(&mut doc, &pointer, nv) {
                let parent = pointer
                    .rfind('/')
                    .map(|i| &pointer[..pointer.floor_char_boundary(i)])
                    .unwrap_or("");
                let available = if parent.is_empty() {
                    doc.as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                } else {
                    doc.pointer(parent)
                        .and_then(|v| v.as_object())
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                };
                let hint = available
                    .map(|k| format!("\nAvailable keys at parent: {}", k))
                    .unwrap_or_default();
                return Err(format!("{}{}", e, hint));
            }
            serde_json::to_string_pretty(&doc).unwrap() + "\n"
        } else if let Some(os) = old_string {
            let ns = new_string.unwrap_or("");
            if os.is_empty() && ns.is_empty() {
                return Err(
                    "Provide either json_path + new_value or old_string + new_string".into(),
                );
            }
            if os == ns {
                return Err("old_string and new_string are identical".into());
            }
            if !content.contains(os) {
                return Err(format!("old_string not found in '{}'", path));
            }
            if replace_all {
                content.replace(os, ns)
            } else {
                content.replacen(os, ns, 1)
            }
        } else {
            return Err("Provide either json_path + new_value or old_string + new_string".into());
        };

        std::fs::write(&full_path, &new_content)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        let commit_msg = message
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| format!("Edit {}", path));
        let commit_sha = self
            .commit_file_change(path, &full_path, &commit_msg)
            .await
            .map_err(|e| format!("Failed to commit: {}", e))?;

        if let Some(artifact_path) = path.strip_prefix("artifacts/") {
            self.event_bus
                .emit(BusEvent::System(SystemEvent::ArtifactUpdated {
                    artifact_path: artifact_path.to_string(),
                    commit: commit_sha.clone(),
                    source: None,
                }))
                .await
                .map_err(|e| format!("Failed to emit event: {}", e))?;
            self.index_artifact_memory(
                artifact_path,
                &new_content,
                &commit_sha,
                Utc::now(),
                extraction_ctx,
            )
            .await;
            if artifact_path == "user_profile.md" {
                *self.user_profile.write().await = new_content;
            }
        }

        let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
        Ok(FileEditResult {
            path: path.to_string(),
            commit: sha_short.to_string(),
        })
    }
}

fn image_media_type(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Parse a `[IMAGE_CONTENT:type]\n<base64>` sentinel produced by `read_file` for image files.
/// Returns `(media_type, base64_data)` if matched, `None` otherwise. Single source of truth
/// for the format; both the agentic loop (which lifts the bytes into an LLM image block) and
/// the persistence path (which strips them) call this.
pub(crate) fn parse_image_content_marker(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("[IMAGE_CONTENT:")?;
    let end_bracket = rest.find("]\n")?;
    Some((&rest[..end_bracket], rest[end_bracket + 2..].trim()))
}

/// If `s` is an `[IMAGE_CONTENT:...]\n<base64>` sentinel, return a small stub that names the
/// media type and approximate decoded size. Returns `None` for non-matching input.
///
/// Why: the LLM-facing path (agentic_loop) parses this sentinel and lifts the bytes into a proper
/// image content block before calling the model, so the base64 in the tool result string is dead
/// weight on disk and on the wire. A 2 MB PNG read by `read_file` becomes a ~50-byte stub.
pub(crate) fn strip_image_content_marker(s: &str) -> Option<String> {
    let (media_type, b64) = parse_image_content_marker(s)?;
    let approx_bytes = (b64.len() * 3) / 4;
    Some(format!(
        "[image {} — {} omitted, not embedded in event]",
        media_type,
        crate::core::format_byte_size(approx_bytes),
    ))
}

impl LucidosEngine {
    pub(crate) async fn execute_file_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        extraction_ctx: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "read_file" => {
                let (data_path, full_path) =
                    match self.resolve_data_path(args["path"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let path = data_path.as_str();

                // Check if it's a binary file by extension
                let extension = std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let is_binary = crate::core::is_binary_extension(&extension);

                if is_binary {
                    // For binary files, check if it exists and return info
                    if full_path.exists() {
                        if extension == "pdf" {
                            // First check for sidecar .txt file (pre-extracted text)
                            let text_sidecar = format!("{}.txt", full_path.display());
                            let text_sidecar_path = std::path::Path::new(&text_sidecar);
                            if text_sidecar_path.exists() {
                                if let Ok(text) = std::fs::read_to_string(text_sidecar_path) {
                                    return Ok(format!("[PDF Text Content]\n\n{}", text));
                                }
                            }

                            // No sidecar, try to extract text from PDF
                            match safe_extract_pdf_text(&full_path) {
                                Ok(text) if !text.trim().is_empty() => {
                                    // Save sidecar and commit (only for artifacts)
                                    if path.starts_with("artifacts/") {
                                        let artifact_path =
                                            path.strip_prefix("artifacts/").unwrap();
                                        let text_path = format!("{}.txt", artifact_path);
                                        let _ = self
                                            .artifact_manager
                                            .write_and_commit(
                                                &text_path,
                                                &text,
                                                &format!("PDF text extracted: {}", artifact_path),
                                            )
                                            .await;
                                    }
                                    Ok(format!("[PDF Text Content]\n\n{}", text))
                                }
                                _ => {
                                    // Try OCR
                                    match extract_text_with_ocr(&full_path) {
                                        Ok(text) => {
                                            // Save sidecar and commit (only for artifacts)
                                            if path.starts_with("artifacts/") {
                                                let artifact_path = path.strip_prefix("artifacts/").unwrap();
                                                let text_path = format!("{}.txt", artifact_path);
                                                let _ = self.artifact_manager.write_and_commit(&text_path, &text, &format!("PDF OCR extracted: {}", artifact_path)).await;
                                            }
                                            Ok(format!("[PDF OCR Content]\n\n{}", text))
                                        }
                                        Err(_) => Ok(format!("[Binary file: {} - PDF text extraction failed. The file exists but cannot be read as text.]", path))
                                    }
                                }
                            }
                        } else if let Some(media_type) = image_media_type(&extension) {
                            let size = match std::fs::metadata(&full_path) {
                                Ok(m) => m.len(),
                                Err(e) => return Ok(format!("Error reading image file: {}", e)),
                            };
                            if size > IMAGE_MAX_BYTES {
                                let mb = size as f64 / (1024.0 * 1024.0);
                                Ok(format!(
                                    "Image too large to read directly ({:.1} MB). Max 5MB.",
                                    mb
                                ))
                            } else {
                                match std::fs::read(&full_path) {
                                    Ok(bytes) => {
                                        let b64 = base64::engine::general_purpose::STANDARD
                                            .encode(&bytes);
                                        Ok(format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64))
                                    }
                                    Err(e) => Ok(format!("Error reading image file: {}", e)),
                                }
                            }
                        } else {
                            Ok(format!("[Binary file: {} - Cannot display binary content. File exists and is {} bytes.]",
                                path,
                                std::fs::metadata(&full_path)
                                    .map(|m| m.len().to_string())
                                    .unwrap_or_else(|_| "unknown".to_string())
                            ))
                        }
                    } else {
                        Ok(format!("[FILE NOT FOUND] '{}' does not exist", path))
                    }
                } else {
                    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    match std::fs::read_to_string(&full_path) {
                        Ok(content) => Ok(sanitize_file_content_for_llm(content, path, offset)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            Ok(format!("Error reading file: file not found: {}", path))
                        }
                        Err(e) => Ok(format!("Error reading file: {}", e)),
                    }
                }
            }
            "write_file" => {
                let content = args["content"].as_str().unwrap_or("");

                let (data_path, full_path) =
                    match self.resolve_data_path(args["path"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let path = data_path.as_str();

                if let Some(reason) = read_only_reason(path) {
                    return Ok(format!("Error: Cannot write '{}': {}", path, reason));
                }

                // SAFEGUARD: Reject empty content (prevents "deleting" via empty write)
                if content.trim().is_empty() {
                    return Ok(
                        "Error: Cannot write empty content. Provide actual file content."
                            .to_string(),
                    );
                }

                let file_exists = full_path.exists();

                // SAFEGUARD: Only block overwrites of binary imports (PDFs, images, etc.)
                // Text files can always be edited since we have git versioning
                let extension = path.rsplit('.').next().unwrap_or("").to_lowercase();
                let is_binary_import = path.starts_with("artifacts/imported/")
                    && crate::core::is_binary_extension(&extension);

                if file_exists && is_binary_import {
                    return Ok(format!(
                        "Error: Cannot overwrite binary import '{}'. Delete it first or create a new file.",
                        path
                    ));
                }

                // Create parent directories and write
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create directories: {}", e))?;
                }
                std::fs::write(&full_path, content)
                    .map_err(|e| format!("Failed to write file: {}", e))?;

                // Commit via appropriate manager
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        let action = if file_exists { "Update" } else { "Add" };
                        format!("{} {}", action, path)
                    });
                let commit_sha = self
                    .commit_file_change(path, &full_path, &commit_msg)
                    .await?;

                // Only emit events and index for artifacts/ paths
                if path.starts_with("artifacts/") {
                    let artifact_path = path.strip_prefix("artifacts/").unwrap();
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::artifact_change(
                            file_exists,
                            artifact_path.to_string(),
                            commit_sha.clone(),
                            None,
                        )))
                        .await?;

                    // Index raw content via Flash extraction
                    self.index_artifact_memory(
                        artifact_path,
                        content,
                        &commit_sha,
                        Utc::now(),
                        Some(extraction_ctx),
                    )
                    .await;

                    // Keep in-memory profile cache in sync
                    if artifact_path == "user_profile.md" {
                        *self.user_profile.write().await = content.to_string();
                    }
                }

                let result_action = if file_exists { "UPDATED" } else { "CREATED" };
                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] {}: {} (commit: {})",
                    result_action, path, sha_short
                ))
            }
            "edit_file" => {
                let raw_path = args["path"].as_str().unwrap_or("");
                match self
                    .edit_file_at_path(
                        raw_path,
                        args.get("json_path").and_then(|v| v.as_str()),
                        args.get("new_value").cloned(),
                        args.get("old_string").and_then(|v| v.as_str()),
                        args.get("new_string").and_then(|v| v.as_str()),
                        args["replace_all"].as_bool().unwrap_or(false),
                        args.get("message").and_then(|v| v.as_str()),
                        Some(extraction_ctx),
                    )
                    .await
                {
                    Ok(r) => Ok(format!(
                        "[ACTION COMPLETED] UPDATED: {} (commit: {})",
                        r.path, r.commit
                    )),
                    Err(e) if e.contains("old_string not found") => {
                        // Show file content so the LLM can retry with the correct old_string
                        let shown = match self
                            .resolve_data_path(raw_path)
                            .map_err(|e| e.to_string())
                            .and_then(|(_, p)| {
                                std::fs::read_to_string(p).map_err(|e| e.to_string())
                            }) {
                            Ok(content) if content.len() > 15000 => {
                                format!("{}...\n[truncated, {} total chars — call read_file to see the rest]",
                                    &content[..content.floor_char_boundary(15000)], content.len())
                            }
                            Ok(content) => content,
                            Err(_) => "Call read_file to see the current content.".to_string(),
                        };
                        Ok(format!(
                            "Error: {}. The file was likely modified by a previous edit. Use the current file content below to construct the correct old_string:\n\n{}",
                            e, shown
                        ))
                    }
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "list_files" => {
                // list_artifacts already walks all browsable data/ directories
                let all_files = match self.artifact_manager.list_artifacts() {
                    Ok(files) => files,
                    Err(e) => return Ok(format!("Error listing files: {}", e)),
                };

                Ok(all_files.join("\n"))
            }
            "copy_file" => {
                let (src_data_path, src_path) =
                    match self.resolve_data_path(args["source"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let source = src_data_path.as_str();
                let (dst_data_path, dst_path) =
                    match self.resolve_data_path(args["destination"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };

                if let Some(reason) = read_only_reason(&dst_data_path) {
                    return Ok(format!(
                        "Error: Cannot copy to '{}': {}",
                        dst_data_path, reason
                    ));
                }

                if !src_path.exists() {
                    return Ok(format!("Error: Source file '{}' not found", source));
                }

                let file_exists = dst_path.exists();

                // Create parent directories
                if let Some(parent) = dst_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create directories: {}", e))?;
                }
                std::fs::copy(&src_path, &dst_path)
                    .map_err(|e| format!("Failed to copy file: {}", e))?;

                // Commit
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Copy {} to {}", source, &dst_data_path));
                let commit_sha = if let Some(app_path) = dst_data_path.strip_prefix("apps/") {
                    self.app_manager.commit(app_path, &commit_msg)?
                } else {
                    self.artifact_manager
                        .commit_data_path(&dst_data_path, &commit_msg)
                        .await?
                };

                // Emit event for artifacts
                if dst_data_path.starts_with("artifacts/") {
                    let artifact_path = dst_data_path.strip_prefix("artifacts/").unwrap();
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::artifact_change(
                            file_exists,
                            artifact_path.to_string(),
                            commit_sha.clone(),
                            None,
                        )))
                        .await?;
                }

                let result_action = if file_exists { "OVERWRITTEN" } else { "COPIED" };
                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] {}: {} → {} (commit: {})",
                    result_action, source, &dst_data_path, sha_short
                ))
            }
            "delete_file" => {
                let (data_path, full_path) =
                    match self.resolve_data_path(args["path"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let path = data_path.as_str();

                if let Some(reason) = read_only_reason(path) {
                    return Ok(format!("Error: Cannot delete '{}': {}", path, reason));
                }

                // Check if file exists
                if !full_path.exists() {
                    return Ok(format!("[NO ACTION NEEDED] File '{}' does not exist (may have been deleted already).", path));
                }

                // Delete and commit via appropriate manager
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Delete {}", path));
                let commit_sha = if let Some(app_path) = path.strip_prefix("apps/") {
                    self.app_manager
                        .delete_file_and_commit(app_path, &commit_msg)?
                } else {
                    self.artifact_manager
                        .delete_data_path_and_commit(path, &commit_msg)
                        .await?
                };

                // Only emit event for artifacts/ paths
                if path.starts_with("artifacts/") {
                    let artifact_path = path.strip_prefix("artifacts/").unwrap();
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::ArtifactDeleted {
                            artifact_path: artifact_path.to_string(),
                            commit: commit_sha.clone(),
                        }))
                        .await?;
                }

                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] DELETED: {} (commit: {})",
                    path, sha_short
                ))
            }
            _ => Ok(format!("Unknown file tool: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_reason_blocks_system_knowhow() {
        assert!(read_only_reason("system-knowhow/best-practices.md").is_some());
        assert!(read_only_reason("system-knowhow/scripts/list.sh").is_some());
    }

    #[test]
    fn read_only_reason_allows_user_paths() {
        assert!(read_only_reason("artifacts/notes.md").is_none());
        assert!(read_only_reason("knowhow/lucidos/best-practices.md").is_none());
        assert!(read_only_reason("apps/foo/index.html").is_none());
        assert!(read_only_reason("triggers/daily/check.md").is_none());
    }

    #[test]
    fn test_image_media_type_mapping() {
        assert_eq!(image_media_type("png"), Some("image/png"));
        assert_eq!(image_media_type("jpg"), Some("image/jpeg"));
        assert_eq!(image_media_type("jpeg"), Some("image/jpeg"));
        assert_eq!(image_media_type("gif"), Some("image/gif"));
        assert_eq!(image_media_type("webp"), Some("image/webp"));
        assert_eq!(image_media_type("svg"), None);
        assert_eq!(image_media_type("pdf"), None);
        assert_eq!(image_media_type("txt"), None);
    }

    #[test]
    fn test_image_content_marker_format() {
        let media_type = "image/png";
        let b64_data = "iVBORw0KGgo=";
        let marker = format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64_data);

        // Verify parsing matches agentic_loop logic
        let rest = marker.strip_prefix("[IMAGE_CONTENT:").unwrap();
        let end_bracket = rest.find("]\n").unwrap();
        let parsed_media = &rest[..end_bracket];
        let parsed_data = rest[end_bracket + 2..].trim();

        assert_eq!(parsed_media, "image/png");
        assert_eq!(parsed_data, "iVBORw0KGgo=");
    }

    #[test]
    fn test_image_size_guard() {
        // Verify the constant is 5 MB
        assert_eq!(IMAGE_MAX_BYTES, 5 * 1024 * 1024);
    }

    #[test]
    fn test_read_image_file_returns_base64_marker() {
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("test.png");
        // Minimal valid PNG (1x1 transparent pixel)
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49,
            0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27,
            0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&img_path, &png_bytes).unwrap();

        let extension = "png";
        let media_type = image_media_type(extension).unwrap();
        let bytes = std::fs::read(&img_path).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let result = format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64);

        assert!(result.starts_with("[IMAGE_CONTENT:image/png]\n"));
        // Verify round-trip: decode the base64 back
        let rest = result.strip_prefix("[IMAGE_CONTENT:image/png]\n").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(rest.trim())
            .unwrap();
        assert_eq!(decoded, png_bytes);
    }

    #[test]
    fn strip_image_content_marker_png() {
        let input = "[IMAGE_CONTENT:image/png]\niVBORw0KGgo=";
        let stub = strip_image_content_marker(input).expect("should match marker");
        assert!(stub.contains("image/png"), "stub mentions media type: {}", stub);
        assert!(stub.contains("omitted"), "stub flags omission: {}", stub);
        assert!(stub.len() < 100, "stub is small: {} chars", stub.len());
    }

    #[test]
    fn strip_image_content_marker_jpeg() {
        let input = "[IMAGE_CONTENT:image/jpeg]\nABCDEFGH";
        let stub = strip_image_content_marker(input).unwrap();
        assert!(stub.contains("image/jpeg"));
    }

    #[test]
    fn strip_image_content_marker_includes_size_label() {
        // 4 base64 chars = ~3 decoded bytes
        let stub_small = strip_image_content_marker("[IMAGE_CONTENT:image/png]\nABCD").unwrap();
        assert!(stub_small.contains("bytes"), "{}", stub_small);

        // ~1.4 MB of base64 → ~1 MB decoded
        let big_b64 = "A".repeat(1_400_000);
        let stub_big =
            strip_image_content_marker(&format!("[IMAGE_CONTENT:image/png]\n{}", big_b64)).unwrap();
        assert!(stub_big.contains("MB"), "{}", stub_big);
    }

    #[test]
    fn strip_image_content_marker_returns_none_for_plain_text() {
        assert!(strip_image_content_marker("Hello, world").is_none());
        assert!(strip_image_content_marker("File contents:\nline1\nline2").is_none());
    }

    #[test]
    fn strip_image_content_marker_returns_none_for_malformed() {
        // Missing the closing `]\n` separator
        assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png ABCDEFGH").is_none());
        // Has prefix but no body separator
        assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png]ABCDEFGH").is_none());
    }

    #[test]
    fn strip_image_content_marker_strips_real_size() {
        // Sanity: a 2 MB base64 string should produce a stub under 100 bytes — the whole
        // point of the helper. This is the behaviour the bugfix relies on.
        let huge = "A".repeat(2 * 1024 * 1024);
        let input = format!("[IMAGE_CONTENT:image/png]\n{}", huge);
        let stub = strip_image_content_marker(&input).unwrap();
        assert!(stub.len() < 100, "stub size {} should be < 100", stub.len());
        assert!(input.len() > 1_000_000);
    }

    #[test]
    fn test_large_image_error_message() {
        let size: u64 = 6 * 1024 * 1024; // 6 MB
        assert!(size > IMAGE_MAX_BYTES);
        let mb = size as f64 / (1024.0 * 1024.0);
        let msg = format!("Image too large to read directly ({:.1} MB). Max 5MB.", mb);
        assert_eq!(msg, "Image too large to read directly (6.0 MB). Max 5MB.");
    }

    #[test]
    fn test_svg_not_treated_as_image() {
        // SVG should not match image_media_type — it's text-based
        assert_eq!(image_media_type("svg"), None);
        // SVG is also not in is_binary_extension, so it goes through text path
        assert!(!crate::core::is_binary_extension("svg"));
    }

    #[test]
    fn test_dot_path_to_pointer_simple_key() {
        assert_eq!(dot_path_to_pointer("title"), "/title");
    }

    #[test]
    fn test_dot_path_to_pointer_nested() {
        assert_eq!(
            dot_path_to_pointer("metadata.author.name"),
            "/metadata/author/name"
        );
    }

    #[test]
    fn test_dot_path_to_pointer_array_index() {
        assert_eq!(dot_path_to_pointer("sections[1]"), "/sections/1");
    }

    #[test]
    fn test_dot_path_to_pointer_mixed() {
        assert_eq!(
            dot_path_to_pointer("sections[1].slides[0].content[2].text"),
            "/sections/1/slides/0/content/2/text"
        );
    }

    #[test]
    fn test_dot_path_to_pointer_empty() {
        assert_eq!(dot_path_to_pointer(""), "");
    }

    #[test]
    fn test_dot_path_to_pointer_already_pointer() {
        assert_eq!(
            dot_path_to_pointer("/sections/1/title"),
            "/sections/1/title"
        );
    }

    #[test]
    fn test_json_set_value_simple_key() {
        let mut doc: serde_json::Value = serde_json::from_str(r#"{"title": "Old"}"#).unwrap();
        let new_val = serde_json::Value::String("New".to_string());
        let result = json_set_value(&mut doc, "/title", new_val);
        assert!(result.is_ok());
        assert_eq!(doc["title"], "New");
    }

    #[test]
    fn test_json_set_value_nested() {
        let mut doc: serde_json::Value =
            serde_json::from_str(r#"{"sections": [{"title": "A", "slides": [{"title": "S1"}]}]}"#)
                .unwrap();
        let new_val = serde_json::Value::String("Updated".to_string());
        let result = json_set_value(&mut doc, "/sections/0/slides/0/title", new_val);
        assert!(result.is_ok());
        assert_eq!(doc["sections"][0]["slides"][0]["title"], "Updated");
    }

    #[test]
    fn test_json_set_value_replace_object() {
        let mut doc: serde_json::Value =
            serde_json::from_str(r#"{"meta": {"version": 1}}"#).unwrap();
        let new_val = serde_json::json!({"version": 2, "author": "test"});
        let result = json_set_value(&mut doc, "/meta", new_val.clone());
        assert!(result.is_ok());
        assert_eq!(doc["meta"], new_val);
    }

    #[test]
    fn test_json_set_value_replace_array_element() {
        let mut doc: serde_json::Value =
            serde_json::from_str(r#"{"items": ["a", "b", "c"]}"#).unwrap();
        let new_val = serde_json::Value::String("B".to_string());
        let result = json_set_value(&mut doc, "/items/1", new_val);
        assert!(result.is_ok());
        assert_eq!(doc["items"][1], "B");
    }

    #[test]
    fn test_json_set_value_invalid_path() {
        let mut doc: serde_json::Value = serde_json::from_str(r#"{"title": "X"}"#).unwrap();
        let new_val = serde_json::Value::String("Y".to_string());
        let result = json_set_value(&mut doc, "/nonexistent/deep/path", new_val);
        assert!(result.unwrap_err().contains("/nonexistent/deep/path"));
    }

    #[test]
    fn test_dot_path_to_pointer_consecutive_brackets() {
        assert_eq!(dot_path_to_pointer("matrix[1][2]"), "/matrix/1/2");
    }

    #[test]
    fn test_json_set_value_root() {
        let mut doc: serde_json::Value = serde_json::from_str(r#"{"old": true}"#).unwrap();
        let new_val = serde_json::json!({"new": true});
        let result = json_set_value(&mut doc, "", new_val.clone());
        assert!(result.is_ok());
        assert_eq!(doc, new_val);
    }
}
