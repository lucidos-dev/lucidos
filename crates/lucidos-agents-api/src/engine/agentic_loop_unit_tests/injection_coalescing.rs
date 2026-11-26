mod injection_coalescing_tests {
    use super::super::{
        coalesced_images_for_reprocess, coalesced_user_text_for_reprocess,
        coalesced_user_text_message, framed_injected_prompt, group_injected_prompts,
        InjectedPromptGroup, InjectionDelivery,
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
            prompt("reentry", InjectedPromptKind::ReentryFromEngine),
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
        assert!(matches!(groups[1], InjectedPromptGroup::Standalone(_)));
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
        let msg = coalesced_user_text_message(
            std::path::Path::new(""),
            &[
                prompt("first follow-up", InjectedPromptKind::UserText),
                prompt("second follow-up", InjectedPromptKind::UserText),
            ],
        );

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
                    text.matches("[USER INTERJECTION").count(),
                    2,
                    "each original prompt keeps its own interjection framing"
                );
            }
            MessageContent::Text(_) => panic!("multiple prompts must use one block message"),
        }
    }

    #[test]
    fn human_framing_says_resume_after_answering_and_still_allows_a_redirect() {
        let framed = framed_injected_prompt(
            &prompt("status?", InjectedPromptKind::UserText),
            InjectionDelivery::MidTurn,
        );

        assert!(framed.contains("status?"), "the user's text must survive");
        assert!(
            framed.starts_with("[USER INTERJECTION"),
            "human interjections must not be framed as corrections: {framed}"
        );
        assert!(
            !framed.contains("Prioritize this over your current plan"),
            "the plan-override wording is what made a status question end the turn: {framed}"
        );
        assert!(
            framed.contains("carry on with the work you had in progress"),
            "the model must be told to resume after answering: {framed}"
        );
        assert!(
            framed.contains("If it redirects you"),
            "a genuine redirect must still override the old plan: {framed}"
        );
    }

    #[test]
    fn no_framing_calls_an_injected_message_a_correction() {
        for mode in [ActorMode::Human, ActorMode::Agent, ActorMode::Engine] {
            for delivery in [InjectionDelivery::MidTurn, InjectionDelivery::NewTurn] {
                let mut p = prompt("status?", InjectedPromptKind::UserText);
                p.mode = mode;
                let framed = framed_injected_prompt(&p, delivery);
                assert!(
                    !framed.to_lowercase().contains("correction"),
                    "{mode:?} {delivery:?}: {framed}"
                );
            }
        }
    }

    #[test]
    fn non_human_framing_stays_a_system_update() {
        let mut p = prompt("build finished", InjectedPromptKind::UserText);
        p.mode = ActorMode::Engine;
        let framed = framed_injected_prompt(&p, InjectionDelivery::MidTurn);

        assert!(framed.starts_with("[SYSTEM UPDATE"), "{framed}");
        assert!(framed.contains("build finished"));
    }

    #[test]
    fn new_turn_framing_carries_no_resume_directive() {
        for mode in [ActorMode::Human, ActorMode::Engine] {
            let mut p = prompt("did it work?", InjectedPromptKind::UserText);
            p.mode = mode;
            let framed = framed_injected_prompt(&p, InjectionDelivery::NewTurn);

            assert!(framed.contains("did it work?"), "{framed}");
            assert!(
                framed.contains("previous turn was still finishing"),
                "the model must be told this arrived late, not mid-turn: {framed}"
            );
            assert!(
                !framed.contains("carry on with the work you had in progress"),
                "there is no work in progress on a re-processed orphan: {framed}"
            );
            assert!(
                !framed.contains("not a reason to end your turn"),
                "a new turn whose only content is this message may end normally: {framed}"
            );
            assert!(
                !framed.contains("current response"),
                "there is no response in flight to fold an update into: {framed}"
            );
        }
    }

    #[test]
    fn reprocess_text_uses_new_turn_framing_and_midturn_message_does_not() {
        let prompts = vec![prompt("first", InjectedPromptKind::UserText)];

        let reprocess = coalesced_user_text_for_reprocess(&prompts);
        assert!(reprocess.starts_with("[USER MESSAGE"), "{reprocess}");

        match coalesced_user_text_message(std::path::Path::new(""), &prompts).content {
            MessageContent::Text(text) => {
                assert!(text.starts_with("[USER INTERJECTION"), "{text}");
            }
            MessageContent::Blocks(_) => panic!("a lone text prompt must use a text message"),
        }
    }

    #[test]
    fn coalesced_message_preserves_images_in_prompt_order() {
        let mut first = prompt("with image", InjectedPromptKind::UserText);
        first.images = Some(vec![crate::engine::types::ChatImage {
            base64: "abc".to_string(),
            mime_type: "image/png".to_string(),
        }]);
        let second = prompt("after image", InjectedPromptKind::UserText);

        let msg = coalesced_user_text_message(std::path::Path::new(""), &[first, second]);
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
    fn a_mid_turn_image_is_labeled_with_its_handle() {
        use base64::Engine as _;
        let tmp = tempfile::TempDir::new().unwrap();
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D,
        ];
        let image = crate::engine::types::ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(png),
            mime_type: "image/png".to_string(),
        };
        let images = std::slice::from_ref(&image);
        crate::engine::message_events::images_to_hashes(tmp.path(), Some(images));
        let handle = crate::engine::chat::current_image_handles(tmp.path(), images)
            .pop()
            .flatten()
            .expect("a stored PNG has a handle");
        let mut with_image = prompt("save this", InjectedPromptKind::UserText);
        with_image.images = Some(vec![image]);

        let MessageContent::Blocks(blocks) =
            coalesced_user_text_message(tmp.path(), &[with_image]).content
        else {
            panic!("image prompts must use blocks");
        };
        assert!(
            matches!(&blocks[0], ContentBlock::Text { text } if text.contains(&handle)),
            "the prompt text must name {handle}, got {:?}",
            blocks[0]
        );
    }

    #[test]
    fn orphan_reprocess_payload_combines_text_and_images() {
        let mut first = prompt("one", InjectedPromptKind::UserText);
        first.images = Some(vec![crate::engine::types::ChatImage {
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
