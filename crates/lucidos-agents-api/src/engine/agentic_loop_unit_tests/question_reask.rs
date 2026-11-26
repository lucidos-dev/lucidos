mod question_reask_tests {
    use super::super::{
        question_reask_cause, reply_ends_in_a_question, QuestionReaskCause, QuestionReaskInputs,
        MAX_PROSE_QUESTION_NUDGE, MAX_QUESTION_REASK,
    };

    fn clean() -> QuestionReaskInputs {
        QuestionReaskInputs {
            ask_failed_last_iter: false,
            leaked_as_text: false,
            ends_in_a_question: false,
            human_can_answer: true,
            reask_forced: 0,
            prose_nudges_forced: 0,
        }
    }

    #[test]
    fn no_force_when_no_cause_holds() {
        assert_eq!(question_reask_cause(clean()), None);
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                reask_forced: MAX_QUESTION_REASK - 1,
                ..clean()
            }),
            None
        );
    }

    #[test]
    fn forces_while_budget_remains() {
        for forced in 0..MAX_QUESTION_REASK {
            assert_eq!(
                question_reask_cause(QuestionReaskInputs {
                    ask_failed_last_iter: true,
                    reask_forced: forced,
                    ..clean()
                }),
                Some(QuestionReaskCause::CallRejected),
                "a rejected call should force at forced={forced}"
            );
            assert_eq!(
                question_reask_cause(QuestionReaskInputs {
                    leaked_as_text: true,
                    reask_forced: forced,
                    ..clean()
                }),
                Some(QuestionReaskCause::LeakedAsText),
                "a leaked tag should force at forced={forced}"
            );
        }
    }

    #[test]
    fn a_rejected_call_wins_over_a_leaked_tag() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ask_failed_last_iter: true,
                leaked_as_text: true,
                ..clean()
            }),
            Some(QuestionReaskCause::CallRejected)
        );
    }

    #[test]
    fn stops_forcing_at_cap() {
        for inputs in [
            QuestionReaskInputs {
                ask_failed_last_iter: true,
                reask_forced: MAX_QUESTION_REASK,
                ..clean()
            },
            QuestionReaskInputs {
                leaked_as_text: true,
                reask_forced: MAX_QUESTION_REASK,
                ..clean()
            },
            QuestionReaskInputs {
                ask_failed_last_iter: true,
                leaked_as_text: true,
                reask_forced: MAX_QUESTION_REASK + 5,
                ..clean()
            },
        ] {
            assert_eq!(question_reask_cause(inputs), None, "{inputs:?}");
        }
    }

    #[test]
    fn the_budget_is_shared_so_the_causes_cannot_alternate_past_the_cap() {
        let mut forced = 0;
        while question_reask_cause(QuestionReaskInputs {
            ask_failed_last_iter: forced % 2 == 0,
            leaked_as_text: forced % 2 == 1,
            reask_forced: forced,
            ..clean()
        })
        .is_some()
        {
            forced += 1;
        }
        assert_eq!(forced, MAX_QUESTION_REASK);
    }

    #[test]
    fn a_reply_ending_on_a_question_is_nudged() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                ..clean()
            }),
            Some(QuestionReaskCause::AskedInProse)
        );
    }

    #[test]
    fn a_reply_ending_on_a_statement_is_not_nudged() {
        assert_eq!(question_reask_cause(clean()), None);
    }

    #[test]
    fn the_prose_nudge_has_its_own_budget_and_stops_at_it() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                prose_nudges_forced: MAX_PROSE_QUESTION_NUDGE,
                ..clean()
            }),
            None
        );
    }

    #[test]
    fn one_diagnosis_per_turn() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                reask_forced: 1,
                ..clean()
            }),
            None
        );
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                ask_failed_last_iter: true,
                reask_forced: MAX_QUESTION_REASK,
                ..clean()
            }),
            None
        );
    }

    #[test]
    fn a_broken_call_outranks_a_trailing_question() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ask_failed_last_iter: true,
                ends_in_a_question: true,
                ..clean()
            }),
            Some(QuestionReaskCause::CallRejected)
        );
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                leaked_as_text: true,
                ends_in_a_question: true,
                ..clean()
            }),
            Some(QuestionReaskCause::LeakedAsText)
        );
    }

    #[test]
    fn a_turn_nobody_can_answer_is_never_nudged() {
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                human_can_answer: false,
                ..clean()
            }),
            None
        );
    }

    #[test]
    fn the_caller_resolves_the_audience_and_a_trigger_is_the_only_exception() {
        let call_site = include_str!("../chat/process/run.rs");
        assert!(
            call_site.lines().any(|l| l.trim() == "!is_trigger,"),
            "run_agentic_loop's `human_can_answer` argument must stay the \
             trigger test, or the prose nudge silently changes who it fires for"
        );
    }

    #[test]
    fn the_detector_reads_the_end_of_the_reply() {
        assert!(reply_ends_in_a_question("Want me to apply both?"));
        assert!(reply_ends_in_a_question("Want me to apply both?\n\n  "));
        assert!(!reply_ends_in_a_question("Applied both."));
        assert!(!reply_ends_in_a_question(""));
        assert!(!reply_ends_in_a_question(
            "Why did it fail? The frame is sandboxed."
        ));
    }

    #[test]
    fn each_cause_names_its_own_problem() {
        let rejected = QuestionReaskCause::CallRejected.instruction();
        let leaked = QuestionReaskCause::LeakedAsText.instruction();
        let prose = QuestionReaskCause::AskedInProse.instruction();
        assert!(rejected.contains("rejected because a question object"));
        assert!(!rejected.contains("`<ask_user_question>` tag"));
        assert!(leaked.contains("typed an `<ask_user_question>` tag"));
        assert!(!leaked.contains("rejected"));
        assert!(prose.contains("question typed as prose"));
        assert!(!prose.contains("rejected"));
        assert!(!prose.contains("`<ask_user_question>` tag"));
        for text in [rejected, leaked, prose] {
            assert!(
                text.contains("ask_user_question"),
                "every instruction must name the tool to re-call"
            );
        }
    }

    #[test]
    fn the_prose_nudge_keeps_the_open_ended_carve_out() {
        let prose = QuestionReaskCause::AskedInProse.instruction();
        assert!(prose.contains("open-ended"));
        assert!(prose.contains("would be guesses"));
    }

    #[test]
    fn declining_the_nudge_keeps_the_draft_and_the_reason_stays_private() {
        let prose = QuestionReaskCause::AskedInProse.instruction();
        assert!(prose.contains("stays as your answer"));
        assert!(prose.contains("The user never sees that line"));
        assert!(
            !prose.contains("ask it again"),
            "the draft is already on screen, so a repeat shows it twice"
        );
    }

    #[test]
    fn every_cause_logs_a_distinct_reason() {
        let reasons = [
            QuestionReaskCause::CallRejected.retry_reason(),
            QuestionReaskCause::LeakedAsText.retry_reason(),
            QuestionReaskCause::AskedInProse.retry_reason(),
        ];
        for (i, a) in reasons.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &reasons[i + 1..] {
                assert_ne!(a, b, "two causes log the same reason");
            }
        }
    }

    #[test]
    fn caps_are_bounded_and_below_the_default_tool_call_cap() {
        const { assert!(MAX_QUESTION_REASK >= 1) };
        assert!((MAX_QUESTION_REASK as f64) < crate::core::prefs::MAX_TOOL_CALLS.default_number());
        const { assert!(MAX_PROSE_QUESTION_NUDGE >= 1) };
        const { assert!(MAX_PROSE_QUESTION_NUDGE <= MAX_QUESTION_REASK) };
    }
}
