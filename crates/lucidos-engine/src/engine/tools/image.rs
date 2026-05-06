use super::super::LucidosEngine;
use crate::api::ChatImage;
use crate::core::events::{walk_thread_images, HasEventPayload};
use crate::llm::image::ImageSize;
use base64::Engine as _;

pub(crate) fn resolve_thread_image_refs<E: HasEventPayload>(
    events: &[E],
    refs: &[String],
) -> Result<Vec<ChatImage>, Box<dyn std::error::Error + Send + Sync>> {
    let images = walk_thread_images(events);
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
        result.push(ChatImage {
            base64: img.base64.to_string(),
            mime_type: img.mime_type.to_string(),
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
            .ok_or("No image provider configured. Set OPENAI_API_KEY or VERTEX_PROJECT_ID.")?;

        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or("prompt is required")?;

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
            return Ok(format!(
                "Error: The current image provider ({}) only supports one input image for editing. \
                 You provided {} images. Please ask the user which image they'd like to use, \
                 or switch to a provider that supports multiple images.",
                provider.name(),
                input_refs.len()
            ));
        }

        // Resolve each reference to raw bytes
        let mut input_images: Vec<Vec<u8>> = Vec::new();
        for reference in &input_refs {
            match self.resolve_image_reference(reference, thread_id).await {
                Ok(bytes) => input_images.push(bytes),
                Err(e) => return Ok(format!("Error resolving image '{}': {}", reference, e)),
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
            self.artifact_manager
                .write_and_commit(
                    artifact_path,
                    &raw_bytes,
                    &format!("feat: generated image {}", artifact_path),
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
            .ok_or("path is required (e.g. 'artifacts/photo.jpg')")?;

        if crate::api::is_path_traversal(artifact_path) {
            return Err("Invalid path (must not contain '..' or start with '/' or '\\')".into());
        }

        let raw_bytes = self.resolve_image_reference(reference, thread_id).await?;

        self.artifact_manager
            .write_and_commit(
                artifact_path,
                &raw_bytes,
                &format!("feat: save thread image to {}", artifact_path),
            )
            .await?;

        crate::log!("[Image] Saved thread image to artifact: {}", artifact_path);

        Ok(format!("Image saved to {}.", artifact_path))
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
            let images = crate::core::events::walk_thread_images(&events);
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

            Ok(base64::engine::general_purpose::STANDARD.decode(img.base64)?)
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
    use crate::core::events::EventRow;
    use crate::llm::tool_names as tn;
    use crate::llm::tools::{get_default_tools, get_save_thread_image_tool};

    fn message_event_with_images(imgs: &[(&str, &str)]) -> EventRow {
        EventRow::new(
            "MessageReceived",
            serde_json::json!({
                "text": "hi",
                "images": imgs.iter().map(|(b64, mime)| serde_json::json!({
                    "base64": b64,
                    "mime_type": mime,
                })).collect::<Vec<_>>(),
            }),
        )
    }

    #[test]
    fn resolve_thread_image_refs_returns_only_requested_indices() {
        let events = vec![message_event_with_images(&[
            ("AAA", "image/png"),
            ("BBB", "image/jpeg"),
            ("CCC", "image/png"),
        ])];

        let resolved =
            resolve_thread_image_refs(&events, &["thread:1".to_string(), "thread:3".to_string()])
                .unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].base64, "AAA");
        assert_eq!(resolved[0].mime_type, "image/png");
        assert_eq!(resolved[1].base64, "CCC");
        assert_eq!(resolved[1].mime_type, "image/png");
    }

    #[test]
    fn resolve_thread_image_refs_empty_input_returns_empty() {
        let events = vec![message_event_with_images(&[("AAA", "image/png")])];
        let resolved = resolve_thread_image_refs(&events, &[]).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_thread_image_refs_invalid_index_errors() {
        let events = vec![message_event_with_images(&[("AAA", "image/png")])];
        let err = resolve_thread_image_refs(&events, &["thread:5".to_string()])
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
        let events = vec![message_event_with_images(&[("AAA", "image/png")])];
        let err = resolve_thread_image_refs(&events, &["thread:0".to_string()])
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
        let events = vec![message_event_with_images(&[("AAA", "image/png")])];
        let err = resolve_thread_image_refs(&events, &["artifacts/foo.png".to_string()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("thread:"),
            "error should explain expected format, got: {}",
            err
        );
    }

    #[test]
    fn run_claude_tool_has_optional_images_parameter() {
        let tools = get_default_tools();
        let tool = tools
            .iter()
            .find(|t| t.name == tn::RUN_CLAUDE)
            .expect("run_claude tool must be registered");

        let props = tool.parameters.get("properties").unwrap();
        let images = props
            .get("images")
            .expect("run_claude must declare an `images` parameter");

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
}
