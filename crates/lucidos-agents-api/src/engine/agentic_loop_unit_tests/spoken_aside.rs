mod spoken_aside_tests {
    use super::super::injections_reopen_a_finished_answer;
    use crate::engine::{thread_events::ActorMode, InjectedPrompt, InjectedPromptKind};

    fn prompt(kind: InjectedPromptKind) -> InjectedPrompt {
        InjectedPrompt {
            text: "what the caller heard".to_string(),
            event_id: None,
            mode: ActorMode::Agent,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind,
        }
    }

    #[test]
    fn a_spoken_aside_alone_leaves_a_finished_answer_alone() {
        assert!(!injections_reopen_a_finished_answer(&[prompt(
            InjectedPromptKind::SpokenAside
        )]));
        assert!(!injections_reopen_a_finished_answer(&[
            prompt(InjectedPromptKind::SpokenAside),
            prompt(InjectedPromptKind::SpokenAside),
        ]));
    }

    #[test]
    fn a_real_follow_up_still_reopens_a_finished_answer() {
        for kind in [
            InjectedPromptKind::UserText,
            InjectedPromptKind::ReentryFromEngine,
            InjectedPromptKind::ReentryFromWait,
        ] {
            assert!(
                injections_reopen_a_finished_answer(&[prompt(kind.clone())]),
                "{kind:?} must still reopen a finished answer"
            );
        }
    }

    #[test]
    fn the_rest_of_a_spoken_sentence_reopens_a_finished_answer() {
        assert!(injections_reopen_a_finished_answer(&[prompt(
            InjectedPromptKind::SpokenContinuation
        )]));
    }

    #[test]
    fn the_rest_of_a_spoken_sentence_carries_no_turn() {
        let kind = InjectedPromptKind::SpokenContinuation;
        assert!(!kind.can_carry_a_turn());
        assert!(
            kind.is_engine_reentry(),
            "it would add a PromptInjected row"
        );
    }

    #[test]
    fn an_aside_rides_along_with_a_real_follow_up() {
        assert!(injections_reopen_a_finished_answer(&[
            prompt(InjectedPromptKind::SpokenAside),
            prompt(InjectedPromptKind::UserText),
        ]));
        assert!(injections_reopen_a_finished_answer(&[
            prompt(InjectedPromptKind::UserText),
            prompt(InjectedPromptKind::SpokenAside),
        ]));
    }

    #[test]
    fn an_empty_drain_reopens_nothing() {
        assert!(!injections_reopen_a_finished_answer(&[]));
    }

    #[test]
    fn a_kind_that_reopens_nothing_is_never_resubmitted_from_the_teardown_window() {
        let placed = |kind: &InjectedPromptKind| match kind {
            InjectedPromptKind::UserText
            | InjectedPromptKind::ReentryFromEngine
            | InjectedPromptKind::ReentryFromWait
            | InjectedPromptKind::SpokenAside
            | InjectedPromptKind::SpokenContinuation => (),
        };
        for kind in [
            InjectedPromptKind::UserText,
            InjectedPromptKind::ReentryFromEngine,
            InjectedPromptKind::ReentryFromWait,
            InjectedPromptKind::SpokenAside,
            InjectedPromptKind::SpokenContinuation,
        ] {
            placed(&kind);
            if !injections_reopen_a_finished_answer(&[prompt(kind.clone())]) {
                assert!(
                    !kind.can_carry_a_turn(),
                    "{kind:?} reopens nothing here yet the teardown drain would re-submit it"
                );
            }
        }
    }
}
