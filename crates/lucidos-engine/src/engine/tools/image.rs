use super::super::LucidosEngine;
use crate::api::ChatImage;
use crate::core::events::{walk_thread_images, HasEventPayload};
use crate::core::WriteAnnouncement;
use crate::llm::image::ImageSize;
use base64::Engine as _;
use std::path::Path;

/// True when `prompt` looks like a request to describe/analyse an existing
/// image instead of synthesise a new one. Anchored at the start of the
/// prompt so phrases like "a robot describing a painting" still synthesise
/// a picture, while "describe this image in detail" gets blocked.
/// ASCII-case-insensitive; only inspects the first ~32 bytes so ~4 KB
/// generation prompts don't pay for a full lowercase allocation.
fn looks_like_description_prompt(prompt: &str) -> bool {
    /// Prefixes that signal a vision / analysis intent. Membership is a
    /// boolean OR over the whole set (`.any()` below), so order has no effect
    /// on the result — a prompt is blocked if it starts with ANY of these.
    const PREFIXES: &[&[u8]] = &[
        b"describe ",
        b"analyse ",
        b"analyze ",
        b"summarize ",
        b"summarise ",
        b"transcribe ",
        b"explain ",
        b"identify ",
        b"read the ",
        b"what is in ",
        b"what's in ",
        b"what does this ",
        b"what do you see ",
        b"tell me about this image",
        b"tell me what ",
    ];
    let trimmed = prompt.trim_start().as_bytes();
    PREFIXES.iter().any(|needle| {
        trimmed.len() >= needle.len() && trimmed[..needle.len()].eq_ignore_ascii_case(needle)
    })
}

pub(crate) fn resolve_thread_image_refs<E: HasEventPayload>(
    workspace: &Path,
    events: &[E],
    refs: &[String],
) -> Result<Vec<ChatImage>, Box<dyn std::error::Error + Send + Sync>> {
    let images = walk_thread_images(workspace, events);
    let total = images.len();

    let mut result = Vec::with_capacity(refs.len());
    for reference in refs {
        let index_str = reference.strip_prefix("thread:").ok_or_else(|| {
            format!(
                "Invalid image reference '{}': expected 'thread:N' format",
                reference
            )
        })?;
        let index: usize = index_str
            .parse()
            .map_err(|_| format!("Invalid thread image index in '{}'", reference))?;
        if index == 0 {
            return Err(format!(
                "Thread image index must be 1 or greater (got '{}')",
                reference
            )
            .into());
        }
        let img = images.iter().find(|i| i.index == index).ok_or_else(|| {
            format!(
                "Thread image '{}' not found. Thread contains {} images total.",
                reference, total
            )
        })?;
        // walk_thread_images yields empty base64 when the user-image blob
        // is missing on disk (partial backup-restore). The numbering slot
        // exists so downstream history annotations stay stable; we
        // surface a clear error here instead of feeding empty bytes to
        // the LLM API.
        if img.base64.is_empty() {
            return Err(format!(
                "Thread image '{}' is referenced but its blob is missing on disk.",
                reference
            )
            .into());
        }
        result.push(ChatImage {
            base64: img.base64.clone(),
            mime_type: img.mime_type.clone(),
        });
    }
    Ok(result)
}

impl LucidosEngine {
    /// Build the image provider implied by the current `image_model`
    /// preference. Resolved per call so Settings changes take effect
    /// without an engine restart.
    pub(crate) async fn current_image_provider(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::llm::ImageProvider>> {
        crate::llm::image::build_image_provider(
            &self.pool,
            self.openai_api_key.as_deref(),
            &self.vertex_project_id,
            &self.vertex_location,
            &self.vertex_token_cache,
        )
        .await
    }

    pub(crate) async fn execute_generate_image(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let provider = self
            .current_image_provider()
            .await
            // Both keys are already-resolved chains (a stored credential, then
            // the env var, then the Codex CLI for OpenAI; ADC and gcloud for
            // Vertex), so naming only the env vars sends a user who configured
            // the provider in Settings to fix the wrong thing. This one reaches
            // the user in chat rather than the log, which is why it matters most.
            .ok_or(
                "No image provider configured. Add an OpenAI key under Settings, Models, \
                 Providers (or set OPENAI_API_KEY), or configure Google Cloud (set \
                 VERTEX_PROJECT_ID or run `gcloud auth application-default login`).",
            )?;

        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or("prompt is required")?;

        // Guard against tool misuse: the model sometimes calls generate_image
        // with prompts like "describe this image in detail" expecting back a
        // text description. The provider would then synthesise a derivative
        // image of nothing useful. Block these prompts with a pointer at the
        // model's native vision instead.
        // TEMPORARY MEASURE — model-tolerance (removable; see
        // docs/temporary-measures.md § "generate_image vision-misuse guard",
        // governed by .claude/rules/temporary-measures.md). Drop once the model
        // reliably stops mistaking generate_image for a vision tool.
        if looks_like_description_prompt(prompt) {
            return Err(
                "this prompt looks like a request to describe/analyse an image, but \
                 `generate_image` SYNTHESISES new images and returns image bytes — not \
                 text descriptions. To describe or analyse an image, just describe it \
                 directly in your reply: you have native vision over recent images in the \
                 conversation. If the image was posted earlier and you can no longer see \
                 it, call view_image (e.g. image: 'thread:2') first to bring it back into \
                 view, then describe it."
                    .into(),
            );
        }

        let size = args
            .get("size")
            .and_then(|v| v.as_str())
            .map(ImageSize::parse_size)
            .unwrap_or(ImageSize::Auto);

        let save_as_artifact = args.get("save_as_artifact").and_then(|v| v.as_str());

        // Resolve input images
        let input_refs: Vec<String> = args
            .get("input_images")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Multi-image validation: error if provider doesn't support it
        if input_refs.len() > 1 && !provider.supports_multi_image() {
            return Err(format!(
                "the current image provider ({}) only supports one input image for editing. \
                 You provided {} images. Please ask the user which image they'd like to use, \
                 or switch to a provider that supports multiple images.",
                provider.name(),
                input_refs.len()
            )
            .into());
        }

        // Resolve each reference to raw bytes
        let mut input_images: Vec<Vec<u8>> = Vec::new();
        for reference in &input_refs {
            match self.resolve_image_reference(reference, thread_id).await {
                Ok(bytes) => input_images.push(bytes),
                Err(e) => return Err(format!("resolving image '{}': {}", reference, e).into()),
            }
        }

        crate::log!(
            "[Image] Generating with {} (prompt: {}, inputs: {}, size: {:?})",
            provider.name(),
            &prompt[..prompt.floor_char_boundary(50)],
            input_images.len(),
            size
        );

        // Call the provider
        let result = provider.generate(prompt, input_images, size).await?;

        // Compress the result through the same pipeline as user images
        let compressed = ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(&result.bytes),
            mime_type: result.mime_type,
        }
        .compress();

        // Save as artifact if requested
        if let Some(artifact_path) = save_as_artifact {
            if crate::api::is_path_traversal(artifact_path) {
                return Err(format!(
                    "Invalid save_as_artifact path (must not contain '..' or start with '/' or '\\'): {}",
                    artifact_path
                )
                .into());
            }
            let raw_bytes = base64::engine::general_purpose::STANDARD.decode(&compressed.base64)?;
            // The store announces the write. This tool used to call the raw
            // writer and emit nothing at all, so a generated image appeared in no
            // artifact list until a reload and was never indexed into memory.
            self.artifact_manager
                .write_and_commit(
                    &self.event_bus,
                    artifact_path,
                    &raw_bytes,
                    &format!("feat: generated image {}", artifact_path),
                    WriteAnnouncement::Entity {
                        source: Some("generate_image".to_string()),
                    },
                )
                .await?;
            crate::log!("[Image] Saved to artifact: {}", artifact_path);
        }

        // Return a JSON result with the image info — the agentic loop will
        // store the base64 in the ToolResult event's images field
        let result_text = if let Some(path) = save_as_artifact {
            format!("Image generated and saved to {}.", path)
        } else {
            "Image generated successfully.".to_string()
        };

        // Pack as special format so agentic loop can extract image
        Ok(format!(
            "[GENERATED_IMAGE:{}]\n{}",
            compressed.base64, result_text
        ))
    }

    pub(crate) async fn execute_save_thread_image(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let reference = args
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or("image is required (e.g. 'thread:1')")?;

        if !reference.starts_with("thread:") {
            return Err("image must be a thread reference (e.g. 'thread:1')".into());
        }

        let artifact_path = args
            .get("path")
            .and_then(|v| v.as_str())
            // Relative to `data/artifacts/`, matching the tool schema in
            // `llm/tools/images.rs` and what `write_and_commit` joins onto. An
            // `artifacts/` prefix here would land at `data/artifacts/artifacts/`.
            .ok_or("path is required, relative to data/artifacts/ (e.g. 'reports/photo.jpg')")?;

        if crate::api::is_path_traversal(artifact_path) {
            return Err("Invalid path (must not contain '..' or start with '/' or '\\')".into());
        }

        let raw_bytes = self.resolve_image_reference(reference, thread_id).await?;

        self.artifact_manager
            .write_and_commit(
                &self.event_bus,
                artifact_path,
                &raw_bytes,
                &format!("feat: save thread image to {}", artifact_path),
                WriteAnnouncement::Entity {
                    source: Some("save_thread_image".to_string()),
                },
            )
            .await?;

        crate::log!("[Image] Saved thread image to artifact: {}", artifact_path);

        Ok(format!("Image saved to {}.", artifact_path))
    }

    /// Re-load an image posted earlier in the thread back into the model's
    /// vision. Resolves a `thread:N` reference to bytes and returns the
    /// `[IMAGE_CONTENT:<type>]\n<base64>` sentinel, which the agentic loop
    /// (`parse_image_content_marker`) lifts into a real `ContentBlock::Image` —
    /// the same path `read_file` uses for image files. This is the only way the
    /// model can see an image that has aged out of the auto-included window.
    pub(crate) async fn execute_view_image(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let reference = args
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or("image is required (e.g. 'thread:1')")?;

        if !reference.starts_with("thread:") {
            return Err(
                "image must be a thread reference like 'thread:1'. To view an \
                 image file saved under data/artifacts/, use read_file instead."
                    .into(),
            );
        }

        let events = self
            .event_store
            .get_thread_events(&thread_id.to_string())
            .await?;
        // resolve_thread_image_refs validates the ref (format, index range,
        // missing-blob) and errors with an actionable message, so on Ok the vec
        // holds exactly the one requested image.
        let mut resolved =
            resolve_thread_image_refs(&self.workspace_path, &events, &[reference.to_string()])?;
        let img = resolved
            .pop()
            .ok_or("internal: resolve_thread_image_refs returned no image")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&img.base64)?;

        crate::log!("[Image] view_image loaded {} into vision", reference);
        Ok(crate::engine::tools::files::encode_image_for_read(
            bytes,
            &img.mime_type,
        ))
    }

    /// Resolve an image reference to raw bytes.
    /// Supports:
    /// - "thread:N" — Nth image in the conversation (1-based)
    /// - artifact path — reads from data/artifacts/...
    async fn resolve_image_reference(
        &self,
        reference: &str,
        thread_id: uuid::Uuid,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(index_str) = reference.strip_prefix("thread:") {
            let target_index: usize = index_str
                .parse()
                .map_err(|_| format!("Invalid thread image index: {}", index_str))?;
            if target_index == 0 {
                return Err("Thread image index must be 1 or greater".into());
            }

            let events = self
                .event_store
                .get_thread_events(&thread_id.to_string())
                .await?;
            let images = crate::core::events::walk_thread_images(&self.workspace_path, &events);
            let total = images.len();

            let img = images
                .into_iter()
                .find(|i| i.index == target_index)
                .ok_or_else(|| {
                    format!(
                        "Thread image {} not found. Thread contains {} images total.",
                        target_index, total
                    )
                })?;

            if img.base64.is_empty() {
                return Err("Image missing base64 data".into());
            }

            Ok(base64::engine::general_purpose::STANDARD.decode(&img.base64)?)
        } else {
            // Artifact path — read from data/ directory
            if crate::api::is_path_traversal(reference) {
                return Err(format!(
                    "Invalid image path (must not contain '..' or start with '/' or '\\'): {}",
                    reference
                )
                .into());
            }
            let full_path = self.workspace_path.join("data").join(reference);
            if !full_path.exists() {
                return Err(format!("Image file not found: {}", reference).into());
            }
            Ok(std::fs::read(&full_path)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::blobs::write_blob;
    use crate::core::events::EventRow;
    use crate::llm::tool_names as tn;
    use crate::llm::tools::{get_default_tools, get_save_thread_image_tool, get_view_image_tool};
    use std::path::Path;

    /// Minimal valid PNG with a per-test discriminator byte so each call
    /// produces a distinct content-addressed hash.
    fn png_with_marker(marker: u8) -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R', // IHDR start
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, marker,
        ]
    }

    /// Write `n` distinct image blobs into `workspace`, then return both the
    /// hashes and a `MessageReceived` event whose payload references them
    /// — mirrors the post-Phase-3b storage shape so the tests exercise the
    /// real walk_thread_images + resolve_thread_image_refs path.
    fn message_event_with_blobs(workspace: &Path, n: u8) -> (Vec<String>, EventRow) {
        let hashes: Vec<String> = (0..n)
            .map(|i| write_blob(workspace, &png_with_marker(i)).unwrap().hash)
            .collect();
        let event = EventRow::new(
            "MessageReceived",
            serde_json::json!({
                "text": "hi",
                "user_image_hashes": hashes,
            }),
        );
        (hashes, event)
    }

    #[test]
    fn resolve_thread_image_refs_returns_only_requested_indices() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 3);
        let events = vec![event];

        let resolved = resolve_thread_image_refs(
            tmp.path(),
            &events,
            &["thread:1".to_string(), "thread:3".to_string()],
        )
        .unwrap();

        // The 1st and 3rd images are returned (not the 2nd). All blobs are
        // PNGs (sniffed by magic bytes), so each comes back with mime image/png.
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].mime_type, "image/png");
        assert_eq!(resolved[1].mime_type, "image/png");
        // Different hashes → different bytes → different base64.
        assert_ne!(resolved[0].base64, resolved[1].base64);
    }

    #[test]
    fn resolve_thread_image_refs_empty_input_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 1);
        let events = vec![event];
        let resolved = resolve_thread_image_refs(tmp.path(), &events, &[]).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_thread_image_refs_invalid_index_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 1);
        let events = vec![event];
        let err = resolve_thread_image_refs(tmp.path(), &events, &["thread:5".to_string()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("thread:5") || err.contains("not found"),
            "error should mention missing index, got: {}",
            err
        );
    }

    #[test]
    fn resolve_thread_image_refs_rejects_zero_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 1);
        let events = vec![event];
        let err = resolve_thread_image_refs(tmp.path(), &events, &["thread:0".to_string()])
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("1 or greater") || err.contains("thread:0"),
            "error should reject zero, got: {}",
            err
        );
    }

    #[test]
    fn resolve_thread_image_refs_rejects_unknown_format() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 1);
        let events = vec![event];
        let err =
            resolve_thread_image_refs(tmp.path(), &events, &["artifacts/foo.png".to_string()])
                .unwrap_err()
                .to_string();
        assert!(
            err.contains("thread:"),
            "error should explain expected format, got: {}",
            err
        );
    }

    #[test]
    fn view_image_tool_definition() {
        let tool = get_view_image_tool();
        assert_eq!(tool.name, tn::VIEW_IMAGE);
        let props = tool.parameters.get("properties").unwrap();
        assert!(props.get("image").is_some(), "must declare an `image` arg");
        let required = tool.parameters.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert!(required.iter().any(|v| v.as_str() == Some("image")));
        // The description must steer the model to use it for re-viewing earlier
        // conversation images (the whole point of the tool).
        let lower = tool.description.to_lowercase();
        assert!(
            lower.contains("view") || lower.contains("see"),
            "description should explain it brings an image back into view: {}",
            tool.description
        );
    }

    /// The exact pipeline `execute_view_image` runs (minus arg-parsing + the DB
    /// event fetch): resolve a `thread:N` ref to a `ChatImage`, decode it, and
    /// re-encode via `encode_image_for_read`. The result MUST be an
    /// `[IMAGE_CONTENT:…]` sentinel that the agentic loop's
    /// `parse_image_content_marker` lifts into a real vision block — otherwise
    /// the model never actually sees the re-loaded image.
    #[test]
    fn view_image_pipeline_produces_a_vision_sentinel() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let img = resolve_thread_image_refs(tmp.path(), &events, &["thread:2".to_string()])
            .unwrap()
            .pop()
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&img.base64)
            .unwrap();
        let sentinel = crate::engine::tools::files::encode_image_for_read(bytes, &img.mime_type);

        let parsed = crate::engine::tools::files::parse_image_content_marker(&sentinel);
        assert!(
            parsed.is_some(),
            "view_image output must be a vision sentinel the loop lifts into an image block, got: {}",
            &sentinel[..sentinel.len().min(60)]
        );
        let (media_type, b64) = parsed.unwrap();
        assert!(
            media_type.starts_with("image/"),
            "media type: {}",
            media_type
        );
        assert!(!b64.is_empty(), "sentinel must carry base64 image data");
    }

    #[test]
    fn run_coding_agent_tool_has_optional_images_parameter() {
        let tools = get_default_tools();
        let tool = tools
            .iter()
            .find(|t| t.name == tn::RUN_CODING_AGENT)
            .expect("run_coding_agent tool must be registered");

        let props = tool.parameters.get("properties").unwrap();
        let images = props
            .get("images")
            .expect("run_coding_agent must declare an `images` parameter");

        assert_eq!(
            images.get("type").and_then(|v| v.as_str()),
            Some("array"),
            "images must be an array"
        );

        let items = images.get("items").expect("images must declare items");
        assert_eq!(
            items.get("type").and_then(|v| v.as_str()),
            Some("string"),
            "image refs must be strings"
        );

        let required = tool.parameters.get("required").unwrap().as_array().unwrap();
        assert!(
            !required.iter().any(|v| v.as_str() == Some("images")),
            "images must be optional"
        );
    }

    #[test]
    fn generated_image_marker_format() {
        let b64 = "dGVzdA=="; // "test" in base64
        let output = format!("[GENERATED_IMAGE:{}]\nImage generated.", b64);
        assert!(output.starts_with("[GENERATED_IMAGE:"));
        assert!(output.contains("]\nImage generated."));
    }

    #[test]
    fn save_thread_image_tool_definition() {
        let tool = get_save_thread_image_tool();
        assert_eq!(tool.name, "save_thread_image");
        let props = tool.parameters.get("properties").unwrap();
        assert!(props.get("image").is_some());
        assert!(props.get("path").is_some());
        let required = tool.parameters.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 2);
        assert!(required.iter().any(|v| v.as_str() == Some("image")));
        assert!(required.iter().any(|v| v.as_str() == Some("path")));
    }

    #[test]
    fn save_thread_image_path_description_is_relative_to_artifacts() {
        // The path parameter must describe paths relative to data/artifacts/,
        // NOT relative to data/. write_and_commit already prepends data/artifacts/.
        // If the description says "under data/" the LLM passes "artifacts/projects/..."
        // which becomes data/artifacts/artifacts/projects/... — double nesting.
        let tool = get_save_thread_image_tool();
        let path_desc = tool.parameters["properties"]["path"]["description"]
            .as_str()
            .unwrap();

        assert!(
            !path_desc.contains("under data/"),
            "path description must NOT say 'under data/' — paths are relative to data/artifacts/. Got: {}",
            path_desc
        );
        assert!(
            !path_desc.contains("'artifacts/"),
            "path example must NOT start with 'artifacts/' — that causes double nesting. Got: {}",
            path_desc
        );
    }

    #[test]
    fn generate_image_save_as_artifact_description_is_relative_to_artifacts() {
        use crate::llm::tools::get_image_generation_tool;

        let tool = get_image_generation_tool();
        let desc = tool.parameters["properties"]["save_as_artifact"]["description"]
            .as_str()
            .unwrap();

        assert!(
            !desc.contains("under data/"),
            "save_as_artifact description must NOT say 'under data/' — paths are relative to data/artifacts/. Got: {}",
            desc
        );
        assert!(
            !desc.contains("'artifacts/"),
            "save_as_artifact example must NOT start with 'artifacts/' — that causes double nesting. Got: {}",
            desc
        );
    }

    #[test]
    fn looks_like_description_prompt_blocks_describe_variants() {
        // Real prompts observed in the wild — the LLM mistaking generate_image
        // for a vision/analysis tool.
        let blocked = [
            "describe this image in detail",
            "Describe the screenshot",
            "describe this image in detail, focus on any UI elements",
            "  Analyse what's shown here",
            "ANALYZE the chart",
            "summarize the contents of this picture",
            "transcribe the text in this image",
            "what is in this image?",
            "what's in this picture",
            "what does this screenshot show",
            "what do you see here",
            "tell me about this image",
            "tell me what the diagram represents",
            "identify the objects in this photo",
            "explain this UI",
            "read the labels in the picture",
        ];
        for prompt in blocked {
            assert!(
                looks_like_description_prompt(prompt),
                "expected to block describe-like prompt: {:?}",
                prompt
            );
        }
    }

    #[test]
    fn looks_like_description_prompt_allows_real_generation_prompts() {
        // Genuine generation prompts that happen to contain the trigger
        // words mid-sentence — should still be allowed through.
        let allowed = [
            "a robot describing a painting in a museum",
            "a chart that summarizes Q3 revenue",
            "an isometric illustration of a UI dashboard",
            "edit this photo to make the sky purple",
            "transcript-style title card for a podcast",
            "a sunset over mountains, painterly",
            "logo for an analyser product",
        ];
        for prompt in allowed {
            assert!(
                !looks_like_description_prompt(prompt),
                "should not block generation prompt: {:?}",
                prompt
            );
        }
    }

    #[test]
    fn generate_image_tool_description_warns_against_vision_misuse() {
        use crate::llm::tools::get_image_generation_tool;
        let tool = get_image_generation_tool();
        let lower = tool.description.to_lowercase();
        // Must contain a clear NOT-a-vision-tool warning. Asserting on a
        // single marker substring (rather than just "describe" + "not"
        // anywhere in the text) prevents regressions like "Generates
        // images. Describes nothing." from passing the gate. The exact
        // wording is part of the contract — change the marker here AND
        // the description text together.
        const REQUIRED: &[&str] = &["not a vision", "not for"];
        assert!(
            REQUIRED.iter().any(|m| lower.contains(m)),
            "tool description must contain one of {:?} so the LLM is told it is \
             not a vision/analysis tool. Got: {}",
            REQUIRED,
            tool.description
        );
        // And it must mention the alternative — "describe directly" / native
        // vision — otherwise the LLM has nowhere to redirect the request.
        assert!(
            lower.contains("describe") && lower.contains("directly"),
            "tool description must instruct the model to describe images \
             directly with native vision. Got: {}",
            tool.description
        );
    }
}
