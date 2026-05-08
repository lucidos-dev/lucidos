/// Replace markdown thread references — `[Title text](thread:UUID)` or
/// `[Title text](thread:workspace/UUID)` — with a neutral placeholder before
/// titling. The link's visible text is the *referenced* thread's title; left
/// in, the LLM happily reuses it as the new thread's title.
fn strip_thread_reference_links(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("thread:") {
        return std::borrow::Cow::Borrowed(text);
    }
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[[^\]]*\]\(thread:[^)]+\)")
            .expect("thread-reference regex must compile")
    });
    RE.replace_all(text, "[referenced thread]")
}

/// What to do when titling a thread for a given message.
///
/// Image-only messages used to hit the LLM with an empty prompt body and
/// produce hallucinated titles like the literal string "Generate a short
/// title". `Image` / `Images` short-circuit those cases; `Skip` covers
/// truly-empty inputs; `Llm` is the normal path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TitleDecision {
    Skip,
    Image,
    Images,
    Llm,
}

pub(crate) fn decide_title_path(
    message: &str,
    image_description: Option<&str>,
    image_count: usize,
) -> TitleDecision {
    let has_text = !message.trim().is_empty();
    let has_desc = image_description.is_some_and(|d| !d.trim().is_empty());
    if has_text || has_desc {
        return TitleDecision::Llm;
    }
    match image_count {
        0 => TitleDecision::Skip,
        1 => TitleDecision::Image,
        _ => TitleDecision::Images,
    }
}

/// Build the LLM prompt for thread title generation.
/// Truncates message to 1000 chars and image description to 300 chars.
fn build_title_prompt(message: &str, image_description: Option<&str>) -> String {
    let truncated: String = strip_thread_reference_links(message)
        .chars()
        .take(1000)
        .collect();
    let image_context = if let Some(desc) = image_description {
        let desc_truncated: String = desc.chars().take(300).collect();
        format!("\n\nAttached image description: {}", desc_truncated)
    } else {
        String::new()
    };
    format!(
        "Generate a very short title (3-6 words) for this conversation. \
         Title by what the user wants to do or know IN THIS THREAD — the action, \
         question, or topic of their request. If the message references another \
         thread, document, or example only as context (e.g. to fix a bug found there), \
         do not title by that referenced material's subject. \
         If the message contains an identifier the user will recognize \
         later — a case number, ticket key (e.g. JIRA-123), reference code, \
         or serial/registration number — include it verbatim in the title. \
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
///
/// `image_count` is the number of images attached to the message being titled,
/// used to short-circuit the LLM for image-only messages (see [`decide_title_path`]).
pub(crate) async fn emit_generated_title(
    bus: &crate::engine::event_bus::EventBus,
    provider: &crate::llm::vertex::VertexProvider,
    thread_id: uuid::Uuid,
    message: &str,
    image_description: Option<&str>,
    fallback_title: Option<String>,
    image_count: usize,
) {
    let title = match decide_title_path(message, image_description, image_count) {
        TitleDecision::Skip => return,
        TitleDecision::Image => "Image".to_string(),
        TitleDecision::Images => "Images".to_string(),
        TitleDecision::Llm => match generate_thread_title(provider, message, image_description)
            .await
        {
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

    /// A thread reference pasted via the copy-ref button arrives as
    /// `[Title text](thread:UUID)` or `[Title text](thread:workspace/UUID)`.
    /// The link's *visible* text is the referenced thread's title — exactly
    /// what biases the LLM into reusing it. Strip both forms before titling.
    #[test]
    fn title_prompt_strips_thread_reference_link_text() {
        let msg = "Fix this bug from [Misplaced section: AI Memory and Context Redundancy](thread:1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff)";
        let prompt = build_title_prompt(msg, None);
        assert!(
            !prompt.contains("Misplaced section"),
            "referenced thread title must not leak into the LLM prompt:\n{}",
            prompt
        );
        assert!(prompt.contains("Fix this bug"));
        assert!(prompt.contains("[referenced thread]"));
    }

    #[test]
    fn title_prompt_strips_workspace_qualified_thread_reference() {
        let msg = "Apply the pattern from [Some Other Thread Title](thread:dev/1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff) here";
        let prompt = build_title_prompt(msg, None);
        assert!(!prompt.contains("Some Other Thread Title"));
        assert!(prompt.contains("Apply the pattern"));
        assert!(prompt.contains("[referenced thread]"));
    }

    #[test]
    fn title_prompt_emphasizes_intent_over_referenced_topic() {
        // The prompt itself must instruct the LLM to title by the user's
        // intent in this thread, not by referenced material's subject.
        let prompt = build_title_prompt("anything", None);
        assert!(prompt.to_lowercase().contains("this thread"));
        assert!(prompt.to_lowercase().contains("referenc"));
    }

    #[test]
    fn title_prompt_instructs_to_include_identifiers() {
        // Pin breadth — the instruction must not collapse to dev-tickets only.
        let prompt = build_title_prompt("anything", None);
        let lower = prompt.to_lowercase();
        assert!(lower.contains("identifier"));
        assert!(prompt.contains("JIRA-123"));
        let non_dev_examples = ["case", "reference", "registration", "serial"];
        assert!(
            non_dev_examples.iter().any(|w| lower.contains(w)),
            "prompt must include at least one non-developer identifier example, got:\n{}",
            prompt
        );
    }

    #[test]
    fn decide_uses_llm_when_text_present() {
        assert_eq!(decide_title_path("hello", None, 0), TitleDecision::Llm);
        assert_eq!(decide_title_path("hello", None, 2), TitleDecision::Llm);
    }

    #[test]
    fn decide_uses_llm_when_image_description_present() {
        assert_eq!(
            decide_title_path("", Some("a movie ticket"), 1),
            TitleDecision::Llm
        );
    }

    #[test]
    fn decide_skips_when_no_content_and_no_images() {
        assert_eq!(decide_title_path("", None, 0), TitleDecision::Skip);
        assert_eq!(decide_title_path("   ", None, 0), TitleDecision::Skip);
        assert_eq!(decide_title_path("", Some("   "), 0), TitleDecision::Skip);
    }

    #[test]
    fn decide_returns_image_for_single_attachment_only() {
        assert_eq!(decide_title_path("", None, 1), TitleDecision::Image);
        // Whitespace-only message + description are equivalent to empty.
        assert_eq!(
            decide_title_path("  \n ", Some("   "), 1),
            TitleDecision::Image
        );
    }

    #[test]
    fn decide_returns_images_for_multiple_attachments_only() {
        assert_eq!(decide_title_path("", None, 2), TitleDecision::Images);
        assert_eq!(decide_title_path("", None, 5), TitleDecision::Images);
    }
}
