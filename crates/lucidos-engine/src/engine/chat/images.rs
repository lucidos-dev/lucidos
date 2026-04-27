use crate::llm::{ContentBlock, MessageContent};

/// Maximum total base64 bytes for all images included in a single LLM call.
const MAX_TOTAL_IMAGE_BASE64: usize = 10_000_000;

/// Maximum number of prior user messages whose images are included in LLM context.
/// Only the N most recent image-bearing messages are kept; older ones appear in
/// conversation history text as "[attached image]" but without the actual data.
/// This prevents stale screenshots from misleading the model in long threads.
pub(super) const MAX_HISTORY_IMAGE_MESSAGES: usize = 3;

/// Compute the user-message cutoff index for image recency: user messages at
/// index >= cutoff are "recent enough" to have their images included.
/// Recency is measured by total user messages, not just image-bearing ones.
pub(super) fn image_recency_cutoff(
    all_prior: &[crate::core::store::SessionMessage],
    max_messages: usize,
) -> usize {
    let user_count = all_prior.iter().filter(|m| m.role == "user").count();
    user_count.saturating_sub(max_messages)
}

/// Filter prior messages to only include images from the most recent N user messages.
/// In long threads, old screenshots can mislead the LLM into thinking they represent
/// current state. By only keeping recent images, the model sees relevant visual context
/// while stale images are referenced only as text annotations in the history.
pub(super) fn filter_recent_history_images(
    all_prior: &[crate::core::store::SessionMessage],
    max_messages: usize,
) -> Vec<Vec<crate::core::store::UserImagePayload>> {
    let cutoff = image_recency_cutoff(all_prior, max_messages);
    all_prior
        .iter()
        .filter(|m| m.role == "user")
        .skip(cutoff)
        .filter(|m| !m.user_images.is_empty())
        .map(|m| m.user_images.clone())
        .collect()
}

/// Build the user message content, combining text with images from both history
/// and the current message. Images are separated into history vs current groups
/// with metadata so the LLM knows which images belong to the current message
/// and which are from earlier in the conversation.
pub(super) fn build_user_content_with_images(
    user_message_text: String,
    history_images: &[Vec<crate::core::store::UserImagePayload>],
    current_images: Option<&[crate::api::ChatImage]>,
) -> MessageContent {
    let mut history_blocks: Vec<ContentBlock> = Vec::new();
    let mut current_blocks: Vec<ContentBlock> = Vec::new();
    let mut total_image_bytes: usize = 0;

    // Skip groups that would push us over the limit (don't break — later smaller groups may fit).
    for imgs in history_images.iter() {
        let group_size: usize = imgs.iter().map(|i| i.base64.len()).sum();
        if total_image_bytes + group_size > MAX_TOTAL_IMAGE_BASE64 {
            continue;
        }

        for img in imgs {
            total_image_bytes += img.base64.len();
            history_blocks.push(ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: img.mime_type.clone(),
                data: img.base64.clone(),
            });
        }
    }

    // Then: images from the current message
    if let Some(imgs) = current_images {
        for img in imgs {
            if total_image_bytes + img.base64.len() > MAX_TOTAL_IMAGE_BASE64 {
                break;
            }
            total_image_bytes += img.base64.len();
            current_blocks.push(ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: img.mime_type.clone(),
                data: img.base64.clone(),
            });
        }
    }

    let history_count = history_blocks.len();
    let current_count = current_blocks.len();
    let total = history_count + current_count;

    if total == 0 {
        return MessageContent::Text(user_message_text);
    }

    let read_instruction = "Read all text and content from every image. Do NOT ask the user to provide details visible in the images.";
    let read_instruction_single = "Read all text and content from the image. Do NOT ask the user to provide details visible in the image.";

    let stale_warning = "IMPORTANT: These images may not reflect current state — code, UI, or configuration may have changed since they were sent. Do NOT assume they show current state.";

    let hint = if history_count > 0 && current_count > 0 {
        // Both history and current — total is always >= 2
        format!(
            "\n\n[{} images total: {} from earlier in the conversation, {} attached to current message. {} {}]",
            total,
            history_count,
            current_count,
            read_instruction,
            stale_warning,
        )
    } else if current_count > 0 {
        if current_count == 1 {
            format!(
                "\n\n[1 image attached to this message. {}]",
                read_instruction_single
            )
        } else {
            format!(
                "\n\n[{} images attached to this message. {}]",
                current_count, read_instruction
            )
        }
    } else {
        // history only
        if history_count == 1 {
            format!(
                "\n\n[1 image from earlier in the conversation. {} {}]",
                read_instruction_single, stale_warning
            )
        } else {
            format!(
                "\n\n[{} images from earlier in the conversation. {} {}]",
                history_count, read_instruction, stale_warning
            )
        }
    };

    let mut blocks = vec![ContentBlock::Text {
        text: format!("{}{}", user_message_text, hint),
    }];

    if history_count > 0 && current_count > 0 {
        // History images first, then separator, then current
        blocks.extend(history_blocks);
        blocks.push(ContentBlock::Text {
            text: format!(
                "[Below: {} attached to current message]",
                if current_count == 1 {
                    "image"
                } else {
                    "images"
                },
            ),
        });
        blocks.extend(current_blocks);
    } else {
        blocks.extend(history_blocks);
        blocks.extend(current_blocks);
    }

    MessageContent::Blocks(blocks)
}

/// Save user-attached images to .lucidos/tmp/images/ so the LLM can reference them by path.
/// Returns a list of relative paths (e.g., ".lucidos/tmp/images/20260317-143052-0.jpg").
pub(super) fn save_images_to_tmp(
    workspace_path: &std::path::Path,
    images: &[crate::api::ChatImage],
) -> Vec<String> {
    use base64::Engine as _;

    if images.is_empty() {
        return Vec::new();
    }

    let images_dir = workspace_path.join(".lucidos/tmp/images");
    if let Err(e) = std::fs::create_dir_all(&images_dir) {
        crate::log!("[Image] Failed to create tmp/images dir: {}", e);
        return Vec::new();
    }

    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut paths = Vec::new();

    for (i, img) in images.iter().enumerate() {
        let ext = match img.mime_type.as_str() {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/svg+xml" => "svg",
            "image/bmp" => "bmp",
            _ => "jpg",
        };
        let filename = format!("{}-{}.{}", timestamp, i, ext);
        let rel_path = format!(".lucidos/tmp/images/{}", filename);
        let full_path = workspace_path.join(&rel_path);

        match base64::engine::general_purpose::STANDARD.decode(&img.base64) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&full_path, &bytes) {
                    crate::log!("[Image] Failed to write {}: {}", rel_path, e);
                    continue;
                }
                paths.push(rel_path);
            }
            Err(e) => {
                crate::log!("[Image] Failed to decode base64 for image {}: {}", i, e);
            }
        }
    }

    if !paths.is_empty() {
        crate::log!(
            "[Image] Saved {} image(s) to .lucidos/tmp/images/",
            paths.len()
        );
    }

    paths
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
