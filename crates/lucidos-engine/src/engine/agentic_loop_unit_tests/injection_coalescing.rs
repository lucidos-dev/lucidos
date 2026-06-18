mod injection_coalescing_tests {
    use super::super::{
        coalesced_images_for_reprocess, coalesced_user_text_for_reprocess,
        coalesced_user_text_message, group_injected_prompts, InjectedPromptGroup,
    };
    use crate::engine::{thread_events::ActorMode, InjectedPrompt, InjectedPromptKind};
    use crate::llm::{ContentBlock, MessageContent};
    use uuid::Uuid;

    fn prompt(text: &str, kind: InjectedPromptKind) -> InjectedPrompt {
        InjectedPrompt {
            text: text.to_string(),
            event_id: Some(Uuid::new_v4()),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind,
        }
    }

    #[test]
    fn groups_contiguous_user_text_prompts_and_keeps_wakes_separate() {
        let groups = group_injected_prompts(vec![
            prompt("one", InjectedPromptKind::UserText),
            prompt("two", InjectedPromptKind::UserText),
            prompt("wake", InjectedPromptKind::WakeFromChild),
            prompt("three", InjectedPromptKind::UserText),
        ]);

        assert_eq!(groups.len(), 3);
        match &groups[0] {
            InjectedPromptGroup::UserText(batch) => {
                assert_eq!(
                    batch.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
                    vec!["one", "two"]
                );
            }
            _ => panic!("first group must be user text"),
        }
        assert!(matches!(groups[1], InjectedPromptGroup::WakeFromChild(_)));
        match &groups[2] {
            InjectedPromptGroup::UserText(batch) => {
                assert_eq!(
                    batch.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
                    vec!["three"]
                );
            }
            _ => panic!("third group must be user text"),
        }
    }

    #[test]
    fn coalesced_user_text_message_uses_one_llm_message_for_multiple_prompts() {
        let msg = coalesced_user_text_message(&[
            prompt("first follow-up", InjectedPromptKind::UserText),
            prompt("second follow-up", InjectedPromptKind::UserText),
        ]);

        assert_eq!(msg.role, "user");
        match msg.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                let text = blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => text.as_str(),
                        _ => panic!("expected text block"),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains("first follow-up"));
                assert!(text.contains("second follow-up"));
                assert_eq!(
                    text.matches("[USER CORRECTION").count(),
                    2,
                    "each original prompt keeps its own correction framing"
                );
            }
            MessageContent::Text(_) => panic!("multiple prompts must use one block message"),
        }
    }

    #[test]
    fn coalesced_message_preserves_images_in_prompt_order() {
        let mut first = prompt("with image", InjectedPromptKind::UserText);
        first.images = Some(vec![crate::api::ChatImage {
            base64: "abc".to_string(),
            mime_type: "image/png".to_string(),
        }]);
        let second = prompt("after image", InjectedPromptKind::UserText);

        let msg = coalesced_user_text_message(&[first, second]);
        match msg.content {
            MessageContent::Blocks(blocks) => {
                assert!(matches!(blocks[0], ContentBlock::Text { .. }));
                match &blocks[1] {
                    ContentBlock::Image {
                        source_type,
                        media_type,
                        data,
                    } => {
                        assert_eq!(source_type, "base64");
                        assert_eq!(media_type, "image/png");
                        assert_eq!(data, "abc");
                    }
                    other => panic!("expected image block after first text, got {other:?}"),
                }
                assert!(matches!(blocks[2], ContentBlock::Text { .. }));
            }
            MessageContent::Text(_) => panic!("image prompts must use blocks"),
        }
    }

    #[test]
    fn orphan_reprocess_payload_combines_text_and_images() {
        let mut first = prompt("one", InjectedPromptKind::UserText);
        first.images = Some(vec![crate::api::ChatImage {
            base64: "img1".to_string(),
            mime_type: "image/png".to_string(),
        }]);
        let second = prompt("two", InjectedPromptKind::UserText);

        let prompts = vec![first, second];
        let text = coalesced_user_text_for_reprocess(&prompts);
        assert!(text.contains("one"));
        assert!(text.contains("two"));
        assert!(text.contains("---"));

        let images = coalesced_images_for_reprocess(&prompts).expect("image must be kept");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].base64, "img1");
    }
}
