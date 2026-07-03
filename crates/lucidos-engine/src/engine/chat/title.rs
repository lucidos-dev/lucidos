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

/// Title-generation instruction. Lives in the system message so it
/// stays out of the model's "thing to summarize" — keeps the model
/// from emitting the instruction text itself as the title on garbled
/// input (e.g. "Generate Very Short Conversation Title").
const TITLE_SYSTEM_PROMPT: &str = "Generate a very short title (3-6 words) for the user's conversation. \
     The message may be an instruction, request, or task addressed to an \
     assistant (e.g. a coding request). Do NOT carry it out, answer it, plan \
     it, or ask for clarification — only summarize what it is about into a title. \
     Title by what the user wants to do or know IN THIS THREAD — the action, \
     question, or topic of their request. If the message references another \
     thread, document, or example only as context (e.g. to fix a bug found there), \
     do not title by that referenced material's subject. \
     If the message contains a human-readable identifier the user will \
     recognize later — a case number, ticket key (e.g. JIRA-123), reference \
     code, or serial/registration number — include it verbatim in the title. \
     Do NOT include opaque identifiers like UUIDs, hex hashes, or long \
     random strings; use the matching human name instead (e.g. the plugin's \
     name, not its install id). \
     Return ONLY the title text, nothing else. No quotes.";

/// True when the LLM echoed the system-prompt instruction back as the
/// title instead of generating one. Substring matches on topic words
/// (e.g. "conversation title") cannot distinguish an echoed instruction
/// from a legitimate title whose subject IS conversation titles, so
/// matches are anchored to instruction shape: imperative "Generate X"
/// prefix, exact-match bare instruction fragments (LLM bailed to a
/// 2-word stub), and a few distinctive instruction phrases that
/// virtually never appear in real titles.
fn is_prompt_echo(title: &str) -> bool {
    let lower = title.to_lowercase();
    let trimmed = lower.trim();

    const EXACT_BARE_FRAGMENTS: &[&str] = &[
        "conversation title",
        "very short title",
        "very short conversation",
        "title",
        "conversation",
    ];
    if EXACT_BARE_FRAGMENTS.contains(&trimmed) {
        return true;
    }

    const ECHO_PATTERNS: &[&str] = &[
        "title for the conversation",
        "title for this conversation",
        "3-6 word",
    ];
    if ECHO_PATTERNS.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // "generated" (adjective) is a different word and stays allowed.
    trimmed.starts_with("generate ")
}

/// The title system prompt asks for a "very short title (3-6 words)". An
/// output past double that maximum is no longer a title — it's the model
/// answering the message (a clarifying question, refusal, or explanation)
/// instead of summarizing it. Observed with a Gemini Flash title model: a
/// screenshot-less "whats this app, and why does it have a filter entry but
/// no threads" message produced the literal title "Please provide more
/// context, a screenshot, or the name of the app you are referring to!".
/// Reject by word count rather than brittle per-phrase matching so the check
/// stays provider-agnostic.
const MAX_TITLE_WORDS: usize = 12;

fn is_oversized_for_title(title: &str) -> bool {
    title.split_whitespace().count() > MAX_TITLE_WORDS
}

/// Validate an LLM-produced title. `Err` carries a human-readable reason and
/// signals the caller to resample or fall back. A title can fail three ways:
/// empty, an echo of the system instruction, or a full conversational
/// response (too long to be a title).
fn validate_title(
    title: String,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if title.is_empty() {
        return Err("LLM returned empty title".into());
    }
    if is_oversized_for_title(&title) {
        return Err(format!("LLM returned a non-title response: {:?}", title).into());
    }
    if is_prompt_echo(&title) {
        return Err(format!("LLM echoed title prompt: {:?}", title).into());
    }
    Ok(title)
}

/// Build the user message for thread title generation — the conversation
/// body, no instruction. Truncates message to 1000 chars and image
/// description to 300 chars.
fn build_title_user_content(message: &str, image_description: Option<&str>) -> String {
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
    format!("{}{}", truncated, image_context)
}

/// Generate a short title (3-6 words) for a new thread using Flash.
/// Standalone function so it can be spawned into a background task.
pub(crate) async fn generate_thread_title(
    provider: &dyn crate::llm::provider::LlmProvider,
    message: &str,
    image_description: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{Message, MessageContent};

    let user_content = build_title_user_content(message, image_description);

    // Smaller/faster title models intermittently answer the message (a
    // clarifying question, refusal, or explanation) instead of titling it;
    // `validate_title` rejects those. Gemini's default sampling is
    // non-deterministic, so one resample usually returns a real title —
    // this mirrors the user manually re-generating. Transport errors are
    // deterministic (auth/region/config) and propagate via `?` without a
    // retry.
    const MAX_ATTEMPTS: usize = 2;
    let mut last_err: Box<dyn std::error::Error + Send + Sync> =
        "title generation produced no candidate".into();

    for _ in 0..MAX_ATTEMPTS {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(user_content.clone()),
        }];
        let response = provider
            .chat(
                messages,
                vec![],
                None,
                Some(TITLE_SYSTEM_PROMPT),
                None,
                Some("none"),
            )
            .await?;
        let title = response.content.ok_or("No title returned")?.trim().to_string();
        match validate_title(title) {
            Ok(t) => return Ok(t),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Generate a thread title via LLM and emit it as a ThreadTitleGenerated event.
/// Used by scheduled triggers, follow-up threads, and spawn_thread.
///
/// `image_count` is the number of images attached to the message being titled,
/// used to short-circuit the LLM for image-only messages (see [`decide_title_path`]).
pub(crate) async fn emit_generated_title(
    bus: &crate::engine::event_bus::EventBus,
    provider: &dyn crate::llm::provider::LlmProvider,
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
        TitleDecision::Llm => {
            // Log model + duration on every round-trip so a slow call
            // (success path emits no other line) is triageable.
            let started = std::time::Instant::now();
            let model = provider.default_model().to_string();
            let result = generate_thread_title(provider, message, image_description).await;
            let outcome = match &result {
                Ok(_) => "generated".to_string(),
                Err(e) if fallback_title.is_some() => format!("failed ({}), using fallback", e),
                Err(e) => format!("failed: {}", e),
            };
            log!(
                "[Title] {} for {} in {:?} via {}",
                outcome,
                thread_id,
                started.elapsed(),
                model
            );
            match result {
                Ok(t) => t,
                Err(_) => match fallback_title {
                    Some(name) => name,
                    None => return,
                },
            }
        }
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
    fn user_content_text_only() {
        let body = build_title_user_content("legg inn denne i familiekalenderen", None);
        assert!(body.contains("legg inn denne i familiekalenderen"));
        assert!(!body.contains("Attached image description"));
    }

    #[test]
    fn user_content_includes_image_description() {
        let body = build_title_user_content(
            "legg inn denne i familiekalenderen",
            Some("A movie ticket for Super Mario Galaxy Filmen at ODEON"),
        );
        assert!(body.contains("legg inn denne i familiekalenderen"));
        assert!(body
            .contains("Attached image description: A movie ticket for Super Mario Galaxy Filmen"));
    }

    #[test]
    fn user_content_truncates_message_to_1000_chars() {
        let long_msg = "a".repeat(1500);
        let body = build_title_user_content(&long_msg, None);
        // The whole user-message body is just the (truncated) message — no
        // instruction preamble, no marker — so its length equals the cap.
        assert_eq!(body.chars().count(), 1000);
    }

    #[test]
    fn user_content_truncates_image_description_to_300_chars() {
        let long_desc = "b".repeat(500);
        let body = build_title_user_content("hello", Some(&long_desc));
        let marker = "Attached image description: ";
        let after_marker = &body[body.find(marker).unwrap() + marker.len()..];
        assert_eq!(after_marker.len(), 300);
    }

    /// Simulates the summary format that suggest_title builds from messages
    /// with image descriptions — both text and image context must reach the LLM.
    #[test]
    fn user_content_with_suggest_title_summary_format() {
        // This mirrors the summary format built by suggest_title in api/threads.rs
        let summary = "legg inn denne i familiekalenderen\n[Attached image: A movie ticket for Super Mario Galaxy Filmen]\n---\nKino i kveld! La meg legge det inn.";
        let body = build_title_user_content(summary, None);
        assert!(body.contains("Super Mario Galaxy Filmen"));
        assert!(body.contains("familiekalenderen"));
        assert!(body.contains("Kino i kveld"));
    }

    /// A thread reference pasted via the copy-ref button arrives as
    /// `[Title text](thread:UUID)` or `[Title text](thread:workspace/UUID)`.
    /// The link's *visible* text is the referenced thread's title — exactly
    /// what biases the LLM into reusing it. Strip both forms before titling.
    #[test]
    fn user_content_strips_thread_reference_link_text() {
        let msg = "Fix this bug from [Misplaced section: AI Memory and Context Redundancy](thread:1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff)";
        let body = build_title_user_content(msg, None);
        assert!(
            !body.contains("Misplaced section"),
            "referenced thread title must not leak into the LLM input:\n{}",
            body
        );
        assert!(body.contains("Fix this bug"));
        assert!(body.contains("[referenced thread]"));
    }

    #[test]
    fn user_content_strips_workspace_qualified_thread_reference() {
        let msg = "Apply the pattern from [Some Other Thread Title](thread:dev/1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff) here";
        let body = build_title_user_content(msg, None);
        assert!(!body.contains("Some Other Thread Title"));
        assert!(body.contains("Apply the pattern"));
        assert!(body.contains("[referenced thread]"));
    }

    #[test]
    fn system_prompt_emphasizes_intent_over_referenced_topic() {
        // The system prompt must instruct the LLM to title by the user's
        // intent in this thread, not by referenced material's subject.
        let lower = TITLE_SYSTEM_PROMPT.to_lowercase();
        assert!(lower.contains("this thread"));
        assert!(lower.contains("referenc"));
    }

    #[test]
    fn system_prompt_instructs_not_to_perform_the_instruction() {
        // The dominant title-gen failure on coding-agent threads: the small
        // title model executes/answers the user's instruction (writing a whole
        // plan, or asking for the screenshot) instead of summarizing it, which
        // validate_title then rejects as oversized — leaving the thread with no
        // ThreadTitleGenerated event and only the raw first-message fallback.
        // The prompt must explicitly steer the model away from performing the
        // instruction.
        let lower = TITLE_SYSTEM_PROMPT.to_lowercase();
        assert!(lower.contains("instruction"));
        assert!(
            lower.contains("do not carry it out") || lower.contains("not carry it out"),
            "prompt must tell the model not to perform the instruction, got:\n{}",
            TITLE_SYSTEM_PROMPT
        );
    }

    #[test]
    fn system_prompt_instructs_to_include_identifiers() {
        // Pin breadth — the instruction must not collapse to dev-tickets only.
        let lower = TITLE_SYSTEM_PROMPT.to_lowercase();
        assert!(lower.contains("identifier"));
        assert!(TITLE_SYSTEM_PROMPT.contains("JIRA-123"));
        let non_dev_examples = ["case", "reference", "registration", "serial"];
        assert!(
            non_dev_examples.iter().any(|w| lower.contains(w)),
            "prompt must include at least one non-developer identifier example, got:\n{}",
            TITLE_SYSTEM_PROMPT
        );
    }

    #[test]
    fn system_prompt_excludes_opaque_identifiers() {
        // Real production case: a "please install this plugin" message carried
        // a 32-char hex install_id, which the LLM dutifully pasted into the
        // title ("Install plugin 410b5e1a6b2b40d0b7c28b09f32e2178"). The
        // identifier instruction must explicitly steer the model away from
        // opaque IDs and toward the human name.
        let lower = TITLE_SYSTEM_PROMPT.to_lowercase();
        assert!(
            lower.contains("human-readable"),
            "prompt must scope identifiers to human-readable ones, got:\n{}",
            TITLE_SYSTEM_PROMPT
        );
        assert!(
            lower.contains("uuid") && lower.contains("hash"),
            "prompt must call out UUIDs and hashes as opaque, got:\n{}",
            TITLE_SYSTEM_PROMPT
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

    /// Real string returned by gemini-3-flash-preview in production for
    /// thread 8c3b5619-c35c-4120-b47d-1a5701b70a14. The user typed the
    /// single word "Release"; with no intent to summarize, the model
    /// echoed the system-instruction phrasing ("Generate a very short
    /// title for the user's conversation") back as the title. Routing
    /// the instruction through `system_instruction` reduces but does
    /// not eliminate this — the validator has to catch it at the output.
    #[test]
    fn echo_detected_for_observed_production_string() {
        assert!(is_prompt_echo("Generate conversation title"));
    }

    #[test]
    fn echo_detected_for_known_paraphrases() {
        assert!(is_prompt_echo("Generate Very Short Conversation Title"));
        assert!(is_prompt_echo("Generate a title (3-6 words)"));
        assert!(is_prompt_echo("Title for the conversation today"));
        assert!(is_prompt_echo("3-6 word summary please"));
    }

    #[test]
    fn echo_detected_for_bare_instruction_fragments() {
        assert!(is_prompt_echo("conversation title"));
        assert!(is_prompt_echo("Conversation Title"));
        assert!(is_prompt_echo("  Very Short Title  "));
        assert!(is_prompt_echo("Title"));
    }

    /// Substring matching on topic words used to false-reject titles
    /// whose subject was conversation titles themselves; these pin that
    /// regression so the detector cannot collapse back to substring rules.
    #[test]
    fn echo_not_detected_for_real_titles() {
        assert!(!is_prompt_echo("Fix auth handshake bug"));
        assert!(!is_prompt_echo("Family calendar event"));
        assert!(!is_prompt_echo("Movie title brainstorming"));
        assert!(!is_prompt_echo("Conversation analysis tips"));
        assert!(!is_prompt_echo("Generated report for Q4"));
        assert!(!is_prompt_echo("Release notes for v2.1"));
        assert!(!is_prompt_echo("Missing Conversation Title Investigation"));
        assert!(!is_prompt_echo("Conversation Title Preference Analysis"));
        assert!(!is_prompt_echo("Thread Title Generation Bug"));
        assert!(!is_prompt_echo("Very short summary of Q4 revenue"));
    }

    /// The user-facing message sent to the LLM must contain ONLY the
    /// conversation text — no instruction, no meta-vocabulary. Anything
    /// from the system prompt that leaks into the user message gives the
    /// model something to echo back as the title on garbled input.
    #[test]
    fn user_content_excludes_instruction_vocabulary() {
        let body = build_title_user_content(
            "The instr rhat ids should be in title is a little much here",
            None,
        );
        let lower = body.to_lowercase();
        // Distinctive phrases from TITLE_SYSTEM_PROMPT only — avoid
        // generic words like "identifier" or "this thread" that real
        // users plausibly type, which would false-positive if the test
        // input were widened.
        for forbidden in [
            "generate",
            "very short",
            "3-6 words",
            "conversation:",
            "return only",
            "jira-123",
        ] {
            assert!(
                !lower.contains(forbidden),
                "user-facing content must not contain instruction phrase {:?}, got:\n{}",
                forbidden,
                body
            );
        }
        // The conversation body itself is preserved.
        assert!(body.contains("instr rhat ids"));
    }

    /// Real string emitted as a thread title in production (thread
    /// d4e28e98-…): the user typed "whats this app, and why does it have a
    /// filter entry but no threads" with a screenshot, but the title path
    /// sends only the text — so the Gemini Flash title model answered the
    /// message instead of titling it. A 16-word sentence is not a title.
    #[test]
    fn oversized_detected_for_observed_production_string() {
        assert!(is_oversized_for_title(
            "Please provide more context, a screenshot, or the name of the app you are referring to!"
        ));
    }

    #[test]
    fn oversized_not_detected_for_real_titles() {
        // 3-6 word titles the prompt asks for.
        assert!(!is_oversized_for_title("Fix auth handshake bug"));
        assert!(!is_oversized_for_title("Release notes for v2.1"));
        assert!(!is_oversized_for_title("Very short summary of Q4 revenue"));
        // A plausibly-verbose-but-still-valid 11-word title for THIS thread
        // must pass — the check must not clip real titles.
        assert!(!is_oversized_for_title(
            "Why the thread filter shows entries but lists no threads"
        ));
    }

    /// Pin the boundary so the threshold can't silently drift: exactly
    /// MAX_TITLE_WORDS is accepted, one word over is rejected.
    #[test]
    fn oversized_boundary_is_max_title_words() {
        let at_max = ["w"; MAX_TITLE_WORDS].join(" ");
        let over_max = ["w"; MAX_TITLE_WORDS + 1].join(" ");
        assert!(!is_oversized_for_title(&at_max));
        assert!(is_oversized_for_title(&over_max));
    }

    #[test]
    fn validate_title_rejects_oversized_conversational_response() {
        assert!(validate_title(
            "Please provide more context, a screenshot, or the name of the app you are referring to!"
                .to_string()
        )
        .is_err());
    }

    #[test]
    fn validate_title_rejects_empty_and_echo() {
        assert!(validate_title(String::new()).is_err());
        assert!(validate_title("Generate conversation title".to_string()).is_err());
    }

    #[test]
    fn validate_title_accepts_real_title() {
        assert_eq!(
            validate_title("Fix auth handshake bug".to_string()).unwrap(),
            "Fix auth handshake bug"
        );
    }
}
