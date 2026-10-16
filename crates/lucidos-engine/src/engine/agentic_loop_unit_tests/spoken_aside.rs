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

    /// The reported shape. A caller asked a second question mid-turn. The
    /// talker answered that one alone, and `call.rs` offered its reply to the
    /// running turn. The offer landed as the answer finished, so the turn
    /// drafted a whole second answer to an aside that asked nothing.
    ///
    /// The caller pays that round twice over: once in latency, and once in a
    /// rewritten answer that dropped every link the first one carried.
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

    /// Every other kind still reopens, which is what this path is FOR. A
    /// follow-up sent mid-call would otherwise orphan into a turn of its own.
    /// An engine re-entry would strand the work that woke it.
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

    /// An aside beside a real follow-up rides along rather than being dropped.
    /// The round is reopening anyway, and telling it what the caller was just
    /// told in its name is the whole point of offering the aside.
    ///
    /// Both orders, because the drain preserves arrival order and the answer
    /// must not depend on which landed first.
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

    /// The ordinary turn, where nothing was queued at all. It must fall through
    /// to the checks below rather than pay a round.
    #[test]
    fn an_empty_drain_reopens_nothing() {
        assert!(!injections_reopen_a_finished_answer(&[]));
    }
}
