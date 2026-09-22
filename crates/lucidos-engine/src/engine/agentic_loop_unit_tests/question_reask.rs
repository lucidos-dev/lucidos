mod question_reask_tests {
    use super::super::{
        question_reask_cause, reply_ends_in_a_question, QuestionReaskCause, QuestionReaskInputs,
        MAX_PROSE_QUESTION_NUDGE, MAX_QUESTION_REASK,
    };

    /// A chat turn that ended cleanly: no broken call, no trailing question,
    /// no budget spent. Every test flips only the fields it is about, so a new
    /// guard cannot be forgotten by a test that predates it.
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
        // No failed ask, no leaked tag, no trailing question: the model's prose
        // answer is legitimate, so never force a re-ask.
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
        // Either broken-call cause forces a re-ask, for each force up to the cap.
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
        // The more specific diagnosis: the model did reach the tool, so tell it
        // what the call got wrong rather than that it typed a tag.
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
        // Once the per-response budget is spent, fall through to a normal prose
        // finalization so a stuck question path can't trap the loop.
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
        // One budget for the two broken-call causes, not one per cause.
        // Otherwise a model alternating between a bad call and a typed tag
        // would loop twice as long.
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
        // The widest cause and the common failure: the model never reached the
        // tool, it just typed "Want me to apply both?" and stopped.
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
        // The whole gate hangs off the trailing `?`. Without one there is no
        // question to route to a card.
        assert_eq!(question_reask_cause(clean()), None);
    }

    #[test]
    fn the_prose_nudge_has_its_own_budget_and_stops_at_it() {
        // Its own counter, so a turn that already spent the broken-call budget
        // does not borrow from it, and vice versa.
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
        // A turn already corrected for a broken call is not corrected again for
        // ending on a question. Three rounds of correction in one turn spends
        // the user's money to say the same thing twice.
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                reask_forced: 1,
                ..clean()
            }),
            None
        );
        // Including once the broken-call budget is exhausted.
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
        // Both hold when the model's ask was rejected and its prose fallback
        // ends on the same question. Name the call it got wrong.
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
        // A trigger run has nobody waiting. Nudging it parks a scheduled job on
        // a card no one asked for.
        assert_eq!(
            question_reask_cause(QuestionReaskInputs {
                ends_in_a_question: true,
                human_can_answer: false,
                ..clean()
            }),
            None
        );
    }

    /// The caller resolves the audience, and getting it wrong is silent: the
    /// nudge simply never fires. This gate read `response_channel ==
    /// Some(Chat)` once, which is never true, because a chat turn passes
    /// `None` there.
    ///
    /// So pin the call site's own argument rather than a channel the loop
    /// re-derives. A bare `!is_trigger,` line is that argument; the other
    /// `!is_trigger` in the file is part of a wider condition.
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
        // Trailing whitespace and newlines are how a streamed reply ends.
        assert!(reply_ends_in_a_question("Want me to apply both?\n\n  "));
        assert!(!reply_ends_in_a_question("Applied both."));
        assert!(!reply_ends_in_a_question(""));
        // A question mid-reply is usually rhetorical or quoted, and the model
        // went on to answer it.
        assert!(!reply_ends_in_a_question(
            "Why did it fail? The frame is sandboxed."
        ));
    }

    #[test]
    fn each_cause_names_its_own_problem() {
        // Telling the model a call was rejected when it never made one sends it
        // looking for a mistake that isn't there.
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
        // `ASK_USER_QUESTION_RULE` licenses plaintext for a question whose
        // options would be guesses. Without the carve-out the nudge pushes the
        // model to invent two, and it has no way to decline.
        let prose = QuestionReaskCause::AskedInProse.instruction();
        assert!(prose.contains("open-ended"));
        assert!(prose.contains("would be guesses"));
    }

    #[test]
    fn declining_the_nudge_repeats_the_question_rather_than_dropping_it() {
        // Only the LAST round's text becomes `ResponseGenerated`. A model told
        // to "drop the question and hand back" therefore erases a question the
        // user was supposed to answer. Taking the carve-out means repeating it.
        //
        // The exit needs no help from the instruction: `MAX_PROSE_QUESTION_NUDGE`
        // is one, so the repeated prose finalizes instead of looping.
        let prose = QuestionReaskCause::AskedInProse.instruction();
        assert!(
            prose.contains("ask it again here"),
            "the carve-out must tell the model to repeat the question"
        );
        assert!(
            !prose.contains("drop the question"),
            "never instruct the model to delete a question the user must answer"
        );
    }

    #[test]
    fn every_cause_logs_a_distinct_reason() {
        // The `LlmCallRetried` reason is how we tell which guard fired and how
        // often. Two causes sharing one string make that unreadable.
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
        // Sanity: both caps are small positive bounds, far under the outer
        // tool-call backstop. All operands are consts, so assert at compile time.
        //
        // This guards the DEFAULT cap, not the configured one: the tool-call cap
        // is a user setting with no ceiling and a floor of 1, so a user who sets
        // it to 1 has deliberately chosen for the backstop to fire before this
        // guard. What must not drift is the unconfigured relationship.
        const { assert!(MAX_QUESTION_REASK >= 1) };
        const { assert!(MAX_QUESTION_REASK < crate::core::DEFAULT_MAX_TOOL_CALLS) };
        const { assert!(MAX_PROSE_QUESTION_NUDGE >= 1) };
        const { assert!(MAX_PROSE_QUESTION_NUDGE <= MAX_QUESTION_REASK) };
    }
}
