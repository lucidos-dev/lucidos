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

const IMAGE_HANDLE_PREFIX: &str = "img-";

/// Whether a reference is an [`image_handle`](crate::core::events::image_handle),
/// as opposed to a path under `data/artifacts/`.
///
/// The hex body is what decides, not the prefix alone. An artifact really can
/// be called `img-cat.png`, and claiming that as a handle would answer a
/// perfectly good file read with "handle not found". A wrong-length hex body
/// IS claimed, so a mistyped handle says so instead of turning into a missing
/// file.
fn is_image_handle(reference: &str) -> bool {
    reference
        .strip_prefix(IMAGE_HANDLE_PREFIX)
        .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The two shapes that name an image already in this thread, as opposed to a
/// path under `data/artifacts/`. `img-<hex>` is the stable handle of ADR 0085
/// Decision 11. `thread:N` is the positional form it replaces, kept working
/// because existing threads and existing habits both still use it.
pub(crate) fn is_thread_image_ref(reference: &str) -> bool {
    reference.starts_with("thread:") || is_image_handle(reference)
}

/// Pick out the one image a reference names. One resolver for both forms, so
/// every tool taking an image reference accepts both.
fn find_thread_image<'a>(
    images: &'a [crate::core::events::ThreadImage],
    reference: &str,
) -> Result<&'a crate::core::events::ThreadImage, String> {
    let total = images.len();
    if is_image_handle(reference) {
        // Compared in full, never as a prefix: a truncated handle would
        // silently resolve to whichever image happened to sort first. Case
        // folded, because the `evt-` address the model also handles round-trips
        // through `Uuid::parse_str`, which accepts either case.
        return images
            .iter()
            .find(|i| i.handle.eq_ignore_ascii_case(reference))
            .ok_or_else(|| {
                format!(
                    "Image handle '{reference}' not found. Thread contains \
                     {total} images total."
                )
            });
    }
    let index_str = reference.strip_prefix("thread:").ok_or_else(|| {
        format!(
            "Invalid image reference '{}': expected 'thread:N' or an \
             'img-<hex>' handle",
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
        ));
    }
    images.iter().find(|i| i.index == index).ok_or_else(|| {
        format!(
            "Thread image '{}' not found. Thread contains {} images total.",
            reference, total
        )
    })
}

pub(crate) fn resolve_thread_image_refs<E: HasEventPayload>(
    workspace: &Path,
    events: &[E],
    refs: &[String],
) -> Result<Vec<ChatImage>, Box<dyn std::error::Error + Send + Sync>> {
    let images = walk_thread_images(workspace, events);

    let mut result = Vec::with_capacity(refs.len());
    for reference in refs {
        let img = find_thread_image(&images, reference)?;
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
                 it, call view_image with its 'img-<hex>' handle first to bring it back \
                 into view, then describe it."
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
        let input_image_bytes: usize = input_images.iter().map(Vec::len).sum();
        let result = provider.generate(prompt, input_images, size).await?;

        // Account for the call. OpenAI's image endpoints report tokens and
        // those are recorded; Imagen prices per image and reports none, so
        // that row carries no usage. The row exists either way, because an
        // image the engine paid for must not be missing from the ledger.
        crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::ImageGen,
        )
        .record_usage(
            // The serving model id, not `provider.name()`, which is a display
            // label ("OpenAI gpt-image-2"). Every other capture site records
            // the model, and a cost breakdown groups on it.
            &result.model,
            prompt.chars().count() + input_image_bytes,
            crate::engine::aux_capture::usage_from_image(result.input_tokens, result.output_tokens),
        )
        .await;

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
            .ok_or("image is required (an 'img-<hex>' handle, or 'thread:1')")?;

        if !is_thread_image_ref(reference) {
            return Err(
                "image must name an image already in this thread: its 'img-<hex>' \
                 handle, or 'thread:1'"
                    .into(),
            );
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
    /// vision. Takes an `img-<hex>` handle or a `thread:N` reference, and
    /// returns the `[IMAGE_CONTENT:<type>]\n<base64>` sentinel. The agentic
    /// loop (`parse_image_content_marker`) lifts that into a real
    /// `ContentBlock::Image`, the same path `read_file` uses for image files.
    /// This is the only way the model can see an image that has aged out of
    /// the auto-included window.
    pub(crate) async fn execute_view_image(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let reference = args
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or("image is required (an 'img-<hex>' handle, or 'thread:1')")?;

        if !is_thread_image_ref(reference) {
            return Err(
                "image must name an image already in this thread: its 'img-<hex>' \
                 handle, or 'thread:1'. To view an image file saved under \
                 data/artifacts/, use read_file instead."
                    .into(),
            );
        }

        let img = self.resolve_thread_image(reference, thread_id).await?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&img.base64)?;

        crate::log!("[Image] view_image loaded {} into vision", reference);
        Ok(crate::engine::tools::files::encode_image_for_read(
            bytes,
            &img.mime_type,
        ))
    }

    /// Load one of this thread's images, by `img-<hex>` handle or `thread:N`.
    ///
    /// The single reader for both, so every tool taking an image reference
    /// resolves it identically and reports the same errors. It used to be
    /// duplicated here and in [`resolve_thread_image_refs`], with the two
    /// copies wording their failures differently.
    async fn resolve_thread_image(
        &self,
        reference: &str,
        thread_id: uuid::Uuid,
    ) -> Result<ChatImage, Box<dyn std::error::Error + Send + Sync>> {
        let events = self
            .event_store
            .get_thread_events(&thread_id.to_string())
            .await?;
        // On Ok the vec holds exactly the one requested image: the resolver
        // validates the form, the range and the missing-blob case, and errors
        // with a message the model can act on.
        resolve_thread_image_refs(&self.workspace_path, &events, &[reference.to_string()])?
            .pop()
            .ok_or_else(|| "internal: resolve_thread_image_refs returned no image".into())
    }

    /// Resolve an image reference to raw bytes. Three shapes:
    /// - `img-<hex>`: the stable handle of an image already in the thread
    /// - `thread:N`: Nth image in the conversation (1-based)
    /// - an artifact path, read from `data/artifacts/`
    async fn resolve_image_reference(
        &self,
        reference: &str,
        thread_id: uuid::Uuid,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        if is_thread_image_ref(reference) {
            let img = self.resolve_thread_image(reference, thread_id).await?;
            Ok(base64::engine::general_purpose::STANDARD.decode(&img.base64)?)
        } else {
            // Through `resolve_data_path`, never a bare `data/`-join. The join
            // took any traversal-free name, so `.env` read the workspace's
            // gitignored secrets and shipped them to the image provider.
            // Normalization refuses the loose `data/` root and defaults a bare
            // name under `artifacts/`, which is what this tool advertises.
            let (_, full_path) = self.resolve_data_path(reference)?;
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
    use crate::llm::tools::{
        get_default_tools, get_save_thread_image_tool, get_view_image_tool, ToolCapabilities,
    };
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
        let markers: Vec<u8> = (0..n).collect();
        message_event_from_markers(workspace, &markers)
    }

    /// Same, with the discriminators named. A repeated marker is the SAME
    /// image under content-addressing. So a test about two distinct messages
    /// picks disjoint markers, or it compares a picture with itself.
    fn message_event_from_markers(workspace: &Path, markers: &[u8]) -> (Vec<String>, EventRow) {
        let hashes: Vec<String> = markers
            .iter()
            .map(|m| write_blob(workspace, &png_with_marker(*m)).unwrap().hash)
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
            err.contains("thread:") && err.contains("img-"),
            "error should name both accepted forms, got: {}",
            err
        );
    }

    // ------------------------------------------------------------------
    // The stable image handle (ADR 0085 Decision 11)
    // ------------------------------------------------------------------

    /// The handle resolves to the same bytes `thread:N` does, so the two are
    /// two names for one image rather than two lookups that can disagree.
    #[test]
    fn a_handle_and_its_thread_index_name_the_same_image() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 3);
        let events = vec![event];

        let walked = crate::core::events::walk_thread_images(tmp.path(), &events);
        let second = &walked[1];
        assert!(second.handle.starts_with("img-"), "{}", second.handle);

        let by_handle =
            resolve_thread_image_refs(tmp.path(), &events, std::slice::from_ref(&second.handle))
                .unwrap();
        let by_index =
            resolve_thread_image_refs(tmp.path(), &events, &["thread:2".to_string()]).unwrap();
        assert_eq!(by_handle[0].base64, by_index[0].base64);
    }

    /// THE point of the handle. `thread:N` counts from the start of the
    /// thread, so an image arriving earlier renumbers every later one. A
    /// handle noted in one turn must still name its own picture after that.
    #[test]
    fn a_handle_survives_the_renumbering_that_breaks_thread_n() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let target = crate::core::events::walk_thread_images(tmp.path(), &events)[0]
            .handle
            .clone();
        let before =
            resolve_thread_image_refs(tmp.path(), &events, std::slice::from_ref(&target)).unwrap();
        let before_by_index =
            resolve_thread_image_refs(tmp.path(), &events, &["thread:1".to_string()]).unwrap();
        assert_eq!(before[0].base64, before_by_index[0].base64);

        // An earlier message now carries two other images, so every index
        // shifts by two.
        let (_older_hashes, older) = message_event_from_markers(tmp.path(), &[40, 41]);
        let shifted = vec![older, events.into_iter().next().unwrap()];

        let after = resolve_thread_image_refs(tmp.path(), &shifted, &[target]).unwrap();
        assert_eq!(
            after[0].base64, before[0].base64,
            "the handle must still name its own image"
        );
        let now_at_index_1 =
            resolve_thread_image_refs(tmp.path(), &shifted, &["thread:1".to_string()]).unwrap();
        assert_ne!(
            now_at_index_1[0].base64, before[0].base64,
            "this test is only meaningful if thread:1 really did move"
        );
    }

    /// A user image's handle is derived from the blob hash already in the
    /// payload. That is what lets the history annotation print the handle
    /// without reading a blob, and what keeps the metadata walk read-free.
    #[test]
    fn a_user_images_handle_comes_from_the_hash_the_payload_already_carries() {
        use crate::core::events::{image_handle, ImageRef};
        let tmp = tempfile::TempDir::new().unwrap();
        let (hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let walked = crate::core::events::walk_thread_images(tmp.path(), &events);
        for (i, hash) in hashes.iter().enumerate() {
            assert_eq!(walked[i].handle, image_handle(ImageRef::BlobHash(hash)));
        }
        // And the metadata-only walk agrees, so the two never diverge.
        let meta = crate::core::events::walk_thread_images_meta(tmp.path(), &events);
        assert_eq!(meta[0].handle, walked[0].handle);
        assert_eq!(meta[1].handle, walked[1].handle);
    }

    /// A handle nobody minted is refused. The refusal says how many images
    /// the thread holds, so the model can tell "wrong id" from "no images".
    #[test]
    fn an_unknown_handle_is_refused_rather_than_falling_back_to_a_position() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let err =
            resolve_thread_image_refs(tmp.path(), &events, &["img-0000000000000000".to_string()])
                .unwrap_err()
                .to_string();
        assert!(err.contains("not found"), "got: {}", err);
        assert!(err.contains("2 images total"), "got: {}", err);
    }

    /// An artifact really can be called `img-cat.png`. Claiming every `img-`
    /// string as a handle would answer a perfectly good file read with
    /// "handle not found", so the hex body is what decides.
    #[test]
    fn an_artifact_path_that_merely_starts_with_img_is_not_a_handle() {
        assert!(!is_thread_image_ref("img-cat.png"));
        assert!(!is_thread_image_ref("img-notes/screenshot.png"));
        assert!(!is_thread_image_ref("img-"));
        // A wrong-length hex body IS a handle, so a mistyped one says so
        // rather than turning into a missing file.
        assert!(is_thread_image_ref("img-abc"));
        assert!(is_thread_image_ref("img-0123456789abcdef"));
        assert!(is_thread_image_ref("thread:2"));
    }

    /// The `evt-` address the model also handles round-trips through
    /// `Uuid::parse_str`, which accepts either case. An image handle folds the
    /// same way, so one habit works for both.
    #[test]
    fn a_handle_resolves_whatever_case_the_model_writes_it_in() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let handle = crate::core::events::walk_thread_images(tmp.path(), &events)[1]
            .handle
            .clone();
        // Only the hex body: the `img-` prefix is the literal that marks the
        // namespace, and folding it would make `IMG-` a handle too.
        let hex = handle.strip_prefix("img-").expect("handle is prefixed");
        let shouted = format!("img-{}", hex.to_uppercase());
        assert_ne!(shouted, handle, "the fixture must have hex letters in it");

        let lower =
            resolve_thread_image_refs(tmp.path(), &events, std::slice::from_ref(&handle)).unwrap();
        let upper = resolve_thread_image_refs(tmp.path(), &events, &[shouted]).unwrap();
        assert_eq!(lower[0].base64, upper[0].base64);
    }

    /// A truncated handle must not resolve. Prefix matching would hand back
    /// whichever image happened to come first, silently and wrongly.
    #[test]
    fn a_truncated_handle_does_not_resolve() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_hashes, event) = message_event_with_blobs(tmp.path(), 2);
        let events = vec![event];

        let full = crate::core::events::walk_thread_images(tmp.path(), &events)[0]
            .handle
            .clone();
        let truncated: String = full.chars().take(full.chars().count() - 4).collect();
        assert!(
            resolve_thread_image_refs(tmp.path(), &events, &[truncated]).is_err(),
            "a prefix of a handle is not a handle"
        );
    }

    #[test]
    fn view_image_tool_advertises_the_handle_and_still_accepts_thread_n() {
        let tool = get_view_image_tool();
        let desc = tool.parameters["properties"]["image"]["description"]
            .as_str()
            .unwrap();
        assert!(desc.contains("img-"), "must advertise the handle: {desc}");
        assert!(desc.contains("thread:"), "must keep thread:N: {desc}");
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
        let tools = get_default_tools(&ToolCapabilities::all_open());
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
