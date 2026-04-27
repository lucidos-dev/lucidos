/// Build the LLM prompt for thread title generation.
/// Truncates message to 1000 chars and image description to 300 chars.
fn build_title_prompt(message: &str, image_description: Option<&str>) -> String {
    let truncated: String = message.chars().take(1000).collect();
    let image_context = if let Some(desc) = image_description {
        let desc_truncated: String = desc.chars().take(300).collect();
        format!("\n\nAttached image description: {}", desc_truncated)
    } else {
        String::new()
    };
    format!(
        "Generate a very short title (3-6 words) for this conversation. \
         Focus on the most specific/important topic — names, places, events, titles. \
         Return ONLY the title text, nothing else. No quotes.\n\n\
         Conversation:\n{}{}",
        truncated, image_context
    )
}

/// Generate a short title (3-6 words) for a new thread using Flash.
/// Standalone function so it can be spawned into a background task.
pub(crate) async fn generate_thread_title(
    provider: &crate::llm::vertex::VertexProvider,
    message: &str,
    image_description: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{LlmProvider, Message, MessageContent};

    let prompt = build_title_prompt(message, image_description);
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(prompt),
    }];
    let response = provider
        .chat(messages, vec![], None, None, None, Some("none"))
        .await?;
    let title = response.content.ok_or("No title returned")?;
    Ok(title.trim().to_string())
}

/// Generate a thread title via LLM and emit it as a ThreadTitleGenerated event.
/// Used by scheduled triggers, follow-up threads, and spawn_thread.
pub(crate) async fn emit_generated_title(
    bus: &crate::engine::event_bus::EventBus,
    provider: &crate::llm::vertex::VertexProvider,
    thread_id: uuid::Uuid,
    message: &str,
    image_description: Option<&str>,
    fallback_title: Option<String>,
) {
    let title = match generate_thread_title(provider, message, image_description).await {
        Ok(t) => t,
        Err(e) => match fallback_title {
            Some(name) => {
                log!(
                    "[Thread] LLM title generation failed, using fallback: {}",
                    e
                );
                name
            }
            None => {
                log!("[Thread] Failed to generate title: {}", e);
                return;
            }
        },
    };
    if let Err(e) = bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated { title },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
    {
        log!("[Thread] Failed to emit title: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_prompt_text_only() {
        let prompt = build_title_prompt("legg inn denne i familiekalenderen", None);
        assert!(prompt.contains("legg inn denne i familiekalenderen"));
        assert!(!prompt.contains("Attached image description"));
    }

    #[test]
    fn title_prompt_includes_image_description() {
        let prompt = build_title_prompt(
            "legg inn denne i familiekalenderen",
            Some("A movie ticket for Super Mario Galaxy Filmen at ODEON"),
        );
        assert!(prompt.contains("legg inn denne i familiekalenderen"));
        assert!(prompt
            .contains("Attached image description: A movie ticket for Super Mario Galaxy Filmen"));
    }

    #[test]
    fn title_prompt_truncates_message_to_1000_chars() {
        let long_msg = "a".repeat(1500);
        let prompt = build_title_prompt(&long_msg, None);
        // The message portion should be 1000 chars, not 1500
        let marker = "Conversation:\n";
        let after_marker = &prompt[prompt.find(marker).unwrap() + marker.len()..];
        assert_eq!(after_marker.len(), 1000);
    }

    #[test]
    fn title_prompt_truncates_image_description_to_300_chars() {
        let long_desc = "b".repeat(500);
        let prompt = build_title_prompt("hello", Some(&long_desc));
        let marker = "Attached image description: ";
        let after_marker = &prompt[prompt.find(marker).unwrap() + marker.len()..];
        assert_eq!(after_marker.len(), 300);
    }

    /// Simulates the summary format that suggest_title builds from messages
    /// with image descriptions — the LLM prompt should contain both text and image context.
    #[test]
    fn title_prompt_with_suggest_title_summary_format() {
        // This mirrors the summary format built by suggest_title in api/threads.rs
        let summary = "legg inn denne i familiekalenderen\n[Attached image: A movie ticket for Super Mario Galaxy Filmen]\n---\nKino med Emil! La meg legge det inn.";
        let prompt = build_title_prompt(summary, None);
        assert!(prompt.contains("Super Mario Galaxy Filmen"));
        assert!(prompt.contains("familiekalenderen"));
        assert!(prompt.contains("Kino med Emil"));
    }
}
