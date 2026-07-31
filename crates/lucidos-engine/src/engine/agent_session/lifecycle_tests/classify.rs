use super::*;

/// Both backends' question tools must suppress the `CodingAgentToolCalled`
/// emit — the `UserQuestionAsked` event renders the card; a tool-call step
/// on top double-surfaces the question. Every other tool (including OTHER
/// lucidos MCP tools like the permission `approve`) must keep emitting, or
/// its step silently vanishes from the timeline.
#[test]
fn question_tools_are_suppressed_other_tools_are_not() {
    assert!(is_user_question_tool("AskUserQuestion"));
    assert!(is_user_question_tool("mcp__lucidos__ask_user_question"));
    for name in [
        "Bash",
        "Edit",
        "command_execution",
        "file_change",
        "mcp__lucidos__approve",
        "mcp__other__ask_user_question",
    ] {
        assert!(
            !is_user_question_tool(name),
            "{name} must emit CodingAgentToolCalled"
        );
    }
}

/// CC merges back-to-back stdin inputs into a single Result, so the
/// engine's `pending_followups` counter cannot be relied on to predict
/// whether more Results are coming. A normal `Generated` Result must
/// always emit `CodingAgentIdled` regardless of inflight inputs —
/// otherwise the thread sits in stored=Archived → DisplaySection::Archive
/// instead of Current. Inflight-followup race protection lives in the
/// run-loop's subprocess-termination decision, not here.
#[test]
fn generated_result_always_emits_idle() {
    let (terminal, emit_idle) = classify_result(false, false, false, false, None, false);
    assert_eq!(terminal, Some(TerminalKind::Generated));
    assert!(
        emit_idle,
        "Generated Result must emit CodingAgentIdled so the thread reaches \
         Current — not Archive — after CC produces a Result"
    );
}

/// CC's stream-json result with `is_error: true` — produced when the
/// upstream API call dies mid-stream — must classify as `Failed` so the
/// frontend renders the partial response with a red failure indicator
/// instead of the green check ResponseGenerated would produce.
#[test]
fn cc_error_classifies_as_failed() {
    let err = "Stream interrupted: connection reset".to_string();
    let (terminal, emit_idle) =
        classify_result(false, false, false, false, Some(err.clone()), false);
    assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
    assert!(
        emit_idle,
        "Failed Result is still a turn boundary — must emit CodingAgentIdled \
         so the next follow-up resumes via --resume"
    );
}

/// Shutdown beats CC error — the engine is going down regardless of how
/// CC ended its turn. Without this, an `Aborted` recovery path would
/// double-emit on restart (Failed already landed, then Aborted overwrites).
#[test]
fn shutdown_wins_over_cc_error() {
    let (terminal, emit_idle) = classify_result(
        false,
        false,
        false,
        true,
        Some("api timeout".to_string()),
        false,
    );
    assert_eq!(
        terminal,
        Some(TerminalKind::Aborted(
            crate::engine::thread_events::AbortCause::EngineShutdown
        ))
    );
    assert!(
        !emit_idle,
        "shutdown must skip CodingAgentIdled regardless of cc_error"
    );
}

/// user_hit_stop beats cc_error: this is the interrupt-cancel case. The
/// `Stop` button (Cancel = Esc) routes through CC's native interrupt, and an
/// interrupted turn comes back as a `Result` with `is_error: true` (CC
/// reports the aborted turn, e.g. `stop_reason=tool_use` / an
/// `[ede_diagnostic]` line). That error is *caused by* the user's cancel, so
/// it must classify as `Canceled`, not `Failed`, otherwise the user sees a
/// red "Failed" dot for a turn they deliberately stopped and the
/// branch-preservation gate (keyed on `Canceled`) never fires. A real
/// failure on a turn the user did NOT stop still classifies as `Failed`
/// (user_hit_stop is false there, see `cc_error_wins_over_empty_text`).
#[test]
fn user_hit_stop_wins_over_cc_error() {
    use crate::engine::thread_events::CancelCause;
    let err = "[ede_diagnostic] result_type=user stop_reason=tool_use".to_string();
    let (terminal, emit_idle) = classify_result(false, true, false, false, Some(err), false);
    assert_eq!(
        terminal,
        Some(TerminalKind::Canceled(CancelCause::UserStop)),
        "an interrupted turn (cc_error set by the cancel) must be Canceled, not Failed"
    );
    assert!(
        emit_idle,
        "cancel is a turn boundary: CodingAgentIdled must follow so the session stays resumable"
    );
}

/// Empty assistant text on an otherwise-clean turn classifies as `Failed`,
/// not `Generated`. Without this branch, a Claude Code subprocess that bailed after
/// an OOM-killed Bash (exit 137) emits `ResponseGenerated { text: "" }`
/// and `CodingAgentIdled { has_changes: true }` — the UI then shows a
/// silent "completed" turn even though the user got nothing back. Routing
/// through `Failed` surfaces the red dot in the UI and (via
/// `may_touch_change_state_at_idle`) refuses to auto-propose the partial
/// worktree state for Apply.
#[test]
fn empty_text_classifies_as_failed_with_empty_response_error() {
    let (terminal, emit_idle) = classify_result(false, false, false, false, None, true);
    assert_eq!(
        terminal,
        Some(TerminalKind::Failed {
            error: EMPTY_RESPONSE_ERROR.to_string(),
        })
    );
    assert!(
        emit_idle,
        "empty-text Failed is still a turn boundary — must emit \
         CodingAgentIdled so the dispatcher closes the turn"
    );
}

/// CC's own error message wins over the generic empty-text fallback —
/// without this, an `is_error: true` Result with empty text would surface
/// "no visible response" instead of the actual upstream cause (e.g. a
/// rate-limit or 5xx). The engine's failure message must be the more
/// specific one when CC has told us why.
#[test]
fn cc_error_wins_over_empty_text() {
    let err = "rate_limit_error".to_string();
    let (terminal, _) = classify_result(false, false, false, false, Some(err.clone()), true);
    assert_eq!(terminal, Some(TerminalKind::Failed { error: err }));
}

/// User-driven cancel that happens to land on an empty Result is still a
/// cancel: the user clicked Stop, the turn ended deliberately. Routing
/// to Failed here would mislabel a deliberate stop as an unexpected
/// failure and break the cancel UX.
#[test]
fn user_hit_stop_wins_over_empty_text() {
    use crate::engine::thread_events::CancelCause;
    let (terminal, _) = classify_result(false, true, false, false, None, true);
    assert_eq!(
        terminal,
        Some(TerminalKind::Canceled(CancelCause::UserStop))
    );
}

/// A Codex mid-turn follow-up redirect (interrupt_is_redirect=true) classifies
/// the interrupted turn as `Canceled(SupersededByFollowup)`, NOT `UserStop`,
/// so the frontend renders it neutrally (like the chat/CC follow-up) instead of
/// "Canceled ✕". Still a cancel mechanically (no Generated, no proposal); only
/// the cause differs. The redirect flag is meaningful only when user_hit_stop
/// is set (an interrupt fired); it never overrides shutdown/error precedence.
#[test]
fn redirect_followup_classifies_as_superseded_by_followup() {
    use crate::engine::thread_events::CancelCause;
    let (terminal, emit_idle) = classify_result(false, true, true, false, None, false);
    assert_eq!(
        terminal,
        Some(TerminalKind::Canceled(CancelCause::SupersededByFollowup)),
        "a follow-up redirect interrupt must carry the SupersededByFollowup cause"
    );
    assert!(
        emit_idle,
        "redirect cancel is still a turn boundary: CodingAgentIdled must follow"
    );
    // Same inputs but with an interrupt-caused cc_error still classify as the
    // redirect cancel (user_hit_stop ranks above cc_error).
    let (with_err, _) = classify_result(
        false,
        true,
        true,
        false,
        Some("[ede_diagnostic] result_type=user stop_reason=tool_use".to_string()),
        true,
    );
    assert_eq!(
        with_err,
        Some(TerminalKind::Canceled(CancelCause::SupersededByFollowup)),
    );
    // redirect flag without user_hit_stop is inert: a clean Result is Generated.
    let (clean, _) = classify_result(false, false, true, false, None, false);
    assert_eq!(clean, Some(TerminalKind::Generated));
    // Shutdown still wins over a redirect interrupt.
    let (shutdown, _) = classify_result(false, true, true, true, None, false);
    assert_eq!(
        shutdown,
        Some(TerminalKind::Aborted(
            crate::engine::thread_events::AbortCause::EngineShutdown
        )),
    );
}

/// Shutdown wins over empty text: engine going down classifies as
/// `Aborted` (not `Failed`) regardless of what CC sent. Without this,
/// a shutdown that lands on an empty Result would emit ResponseFailed
/// and the recovery path would skip re-resuming the session.
#[test]
fn shutdown_wins_over_empty_text() {
    use crate::engine::thread_events::AbortCause;
    let (terminal, emit_idle) = classify_result(false, false, false, true, None, true);
    assert_eq!(
        terminal,
        Some(TerminalKind::Aborted(AbortCause::EngineShutdown))
    );
    assert!(
        !emit_idle,
        "shutdown must skip CodingAgentIdled even when text is empty"
    );
}

/// Silent resume drops the empty-text Failed too — a warmup with no user
/// content always emits nothing, regardless of what CC produced. Without
/// this, an engine-internal warmup resume would surface a spurious
/// "no visible response" failure on a thread the user never engaged.
#[test]
fn silent_resume_drops_empty_text_too() {
    let (terminal, emit_idle) = classify_result(true, false, false, false, None, true);
    assert!(terminal.is_none());
    assert!(!emit_idle);
}

/// Empty-text Result on an otherwise-clean turn classifies as `Failed`
/// (the OOM-killed Bash / SIGTERM'd subprocess scenario). The propose
/// gate refuses Failed, so this turn does NOT surface as a pending
/// change — the user gets a red-dot terminal in the UI and the partial
/// worktree state stays uncommitted on the branch for resume. Without
/// this branch the empty Result would render as a green "completed"
/// turn AND auto-propose the partial work as Apply-ready.
#[test]
fn empty_text_failed_does_not_propose() {
    let (terminal, _) = classify_result(false, false, false, false, None, true);
    assert!(
        !may_touch_change_state_at_idle(false, false, false, &terminal),
        "empty-text Failed (OOM / SIGTERM) must NOT auto-propose — half-assed"
    );
}

/// The maximally stale-looking turn: resumed, the backend did NOT confirm the
/// attach, empty output, zero activity, no error. Every other stale-resume test
/// flips exactly one field of this baseline, so each asserts one gate.
fn stale_baseline() -> StaleResumeInputs {
    StaleResumeInputs {
        has_resume_session: true,
        resume_attach_confirmed: false,
        result_text_empty: true,
        buffered_text_empty: true,
        no_prior_results_this_turn: true,
        no_tool_calls_this_turn: true,
        user_message_present: true,
        cc_error: false,
    }
}

/// Empty Result on a resumed turn with no error AND no tool calls AND no
/// confirmed attach → real stale-resume signal (a dead session produces an
/// immediate empty answer with zero activity). The run-loop retries with a
/// fresh spawn, REUSING the worktree (it no longer deletes it).
#[test]
fn empty_result_on_resume_with_no_error_is_stale_resume() {
    assert!(is_stale_resume_signal(stale_baseline()));
}

/// Empty Result on a resumed turn WITH a CC-reported error → real
/// upstream failure, NOT stale resume. Without this guard a transient
/// network drop would trigger a spurious fresh-spawn retry.
#[test]
fn empty_result_on_resume_with_cc_error_is_not_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        cc_error: true,
        ..stale_baseline()
    }));
}

/// THE FABLE FALSE-POSITIVE REGRESSION GUARD (2026-07-02). A terse model
/// (Fable-5) routinely emits empty assistant text and jumps straight to a tool
/// call — so `result_text_empty && buffered_text_empty` are both true even
/// though the session is alive and working. A tool call this turn
/// (`no_tool_calls_this_turn == false`) MUST veto the stale-resume verdict.
/// Before this gate, every terse Fable resume was misclassified as stale,
/// cancelled, and re-spawned → a duplicate CC process on the shared worktree
/// (2x quota burn).
#[test]
fn empty_result_on_resume_but_made_tool_calls_is_not_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        // a tool call happened → ALIVE
        no_tool_calls_this_turn: false,
        ..stale_baseline()
    }));
}

/// THE SWITCH-RESUME FALSE-POSITIVE REGRESSION GUARD (2026-07-29, thread
/// `cb503361`). The backend reported back the SAME session id we asked to
/// `--resume`, which proves the conversation is live — so NO amount of empty
/// output may call it stale. Here the turn is empty and inactive purely because
/// `claude --print --resume` emitted a `result` for its own synthetic
/// `Continue from where you left off.` / `No response requested.` turn (injected
/// to close a tool_use the switch teardown interrupted) before reading our
/// stdin. Without this gate the healthy Opus-5 session was cancelled 10 ms after
/// Init and the thread wedged at `running` for 8 minutes.
#[test]
fn confirmed_attach_is_never_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        resume_attach_confirmed: true,
        ..stale_baseline()
    }));
}

/// The complement of the guard above: the backend reported a DIFFERENT session
/// id than the one we asked to resume (CC silently started a fresh conversation
/// / Codex fell back to `thread/start`). There is no structural proof of life,
/// so the empty-echo heuristic still governs and this IS stale. Keeps the
/// `dev/bf997e21` CLAUDE_CONFIG_DIR-relocation recovery working.
#[test]
fn unconfirmed_attach_still_falls_back_to_the_empty_echo_heuristic() {
    assert!(is_stale_resume_signal(StaleResumeInputs {
        resume_attach_confirmed: false,
        ..stale_baseline()
    }));
}

/// Non-resumed turn never qualifies (the retry path only makes sense
/// when the dead session id actually came from a prior CodingAgentIdled).
#[test]
fn fresh_session_is_never_stale_resume() {
    assert!(!is_stale_resume_signal(StaleResumeInputs {
        has_resume_session: false,
        ..stale_baseline()
    }));
}

/// CC's EXPLICIT session-not-found error IS a definitive stale-resume signal —
/// the one whitelisted `cc_error` string, because re-resuming a gone session can
/// never succeed. This is the exact error from dev/bf997e21 (a mid-flight
/// `CLAUDE_CONFIG_DIR` switch relocated CC's transcript store).
#[test]
fn explicit_no_conversation_found_is_definitive() {
    assert!(is_definitive_session_not_found(Some(
        "No conversation found with session ID: 7c61f11b-414e-4e5e-9da7-bdbd3b3649d6"
    )));
}

/// A generic `cc_error` (transient upstream failure) must NOT be treated as
/// session-not-found — otherwise the recovery would `worktree remove` on a 5xx
/// and strand the user's in-flight work. Only the exact whitelisted string
/// qualifies.
#[test]
fn generic_cc_error_is_not_session_not_found() {
    assert!(!is_definitive_session_not_found(Some(
        "API Error: 529 overloaded"
    )));
    assert!(!is_definitive_session_not_found(Some("error_max_turns")));
    assert!(!is_definitive_session_not_found(Some("")));
}

/// No error at all is not a session-not-found signal (that path is the
/// empty-echo `is_stale_resume_signal` heuristic instead).
#[test]
fn no_error_is_not_session_not_found() {
    assert!(!is_definitive_session_not_found(None));
}

/// Silent resume (warmup with no user content) still drops every Result,
/// even when CC reports an error. Without this, an error-during-warmup
/// would emit ResponseFailed against a thread the user never engaged.
#[test]
fn silent_resume_drops_cc_error_too() {
    let (terminal, emit_idle) = classify_result(
        true,
        false,
        false,
        false,
        Some("error_during_execution".to_string()),
        false,
    );
    assert!(terminal.is_none());
    assert!(!emit_idle);
}

/// Pin every input combination to its expected (terminal, emit_idle) pair.
/// The invariants this guards:
///   - `Generated` → `emit_idle = true`. Skipping idle here is the bug —
///     CC really finished, so the thread row must flip to `waiting`/`idle`
///     and the section must move to Current.
///   - `Aborted` → `emit_idle = false`. Emitting idle on shutdown makes
///     `recover_orphaned_worktrees` think the session is "truly idle" and
///     skip recovery on restart.
///   - `Canceled` (user-driven stop) → `emit_idle = true`. Cancel is a
///     turn boundary; the dispatcher needs `CodingAgentIdled` to pick up
///     the next message via `--resume`.
#[test]
fn classify_result_table() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let cases = [
        // (is_silent_resume, user_hit_stop, is_shutdown) → (terminal, emit_idle)
        ((false, false, false), (Some(TerminalKind::Generated), true)),
        (
            (false, true, false),
            (Some(TerminalKind::Canceled(CancelCause::UserStop)), true),
        ),
        (
            (false, false, true),
            (
                Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
                false,
            ),
        ),
        // Shutdown overrides user_hit_stop — Aborted, idle skipped.
        (
            (false, true, true),
            (
                Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
                false,
            ),
        ),
        // Silent resume / warmup (no user content) emits nothing.
        ((true, false, false), (None, false)),
        ((true, true, false), (None, false)),
        ((true, false, true), (None, false)),
        ((true, true, true), (None, false)),
    ];
    for ((silent, stop, shutdown), expected) in cases {
        assert_eq!(
            // redirect=false: this table pins the non-redirect cause matrix.
            classify_result(silent, stop, false, shutdown, None, false),
            expected,
            "(is_silent_resume={}, user_hit_stop={}, is_shutdown={})",
            silent,
            stop,
            shutdown,
        );
    }
}

/// Real Cancel click on an actively-working CC emits `ResponseCanceled` —
/// this is the only path that should ever produce that event.
#[test]
fn real_cancel_on_working_cc_emits_canceled() {
    use crate::engine::thread_events::CancelCause;
    assert_eq!(
        stop_terminal_kind(false, false, false),
        Some(TerminalKind::Canceled(CancelCause::UserStop)),
        "real Cancel click on actively-working CC must emit Canceled"
    );
}

/// Cancel click that races in after CC went idle emits nothing — the
/// previous turn's `ResponseGenerated` already terminated it. Without
/// this, the late Cancel would land a phantom "Canceled the response"
/// on a turn that finished cleanly.
#[test]
fn cancel_racing_idle_emits_no_terminal_event() {
    assert_eq!(
        stop_terminal_kind(false, true, false),
        None,
        "Cancel that raced after CC went idle must NOT emit Canceled — \
         previous turn already finished cleanly"
    );
}

/// Apply / Discard / Archive trigger the stop signal but their own
/// lifecycle event (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`)
/// is the terminator. The stop arm must NOT emit `ResponseCanceled` on
/// top — the user didn't cancel.
#[test]
fn user_action_suppresses_terminal_regardless_of_idle() {
    assert_eq!(
        stop_terminal_kind(false, false, true),
        None,
        "Apply/Discard/Archive on actively-working CC must NOT emit Canceled — \
         the lifecycle event is the terminator"
    );
    assert_eq!(
        stop_terminal_kind(false, true, true),
        None,
        "Apply/Discard/Archive on idle CC must NOT emit Canceled either, \
         nothing in flight to cancel and the lifecycle event is the terminator"
    );
}

/// Shutdown of an idle CC must emit nothing: the prior exchange already
/// completed cleanly via `CodingAgentIdled`, and emitting `Aborted` here
/// would relabel a finished exchange as crashed when the engine goes
/// down.
#[test]
fn shutdown_of_idle_session_emits_no_terminal_event() {
    assert_eq!(
        stop_terminal_kind(true, true, false),
        None,
        "shutdown when CC is already idle must NOT emit a terminal event"
    );
    assert_eq!(
        stop_terminal_kind(true, true, true),
        None,
        "shutdown wins over user-action — idle still emits nothing"
    );
}

/// Shutdown of a working CC emits `Aborted` — the in-flight exchange was
/// killed by the engine, not finished by CC. Shutdown wins over the
/// user-action suppress flag — the system kill is the dominant cause.
#[test]
fn shutdown_of_working_session_emits_aborted() {
    use crate::engine::thread_events::AbortCause;
    assert_eq!(
        stop_terminal_kind(true, false, false),
        Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
        "shutdown during active work must emit Aborted"
    );
    assert_eq!(
        stop_terminal_kind(true, false, true),
        Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
        "shutdown wins over user-action suppression — Aborted still fires"
    );
}

/// Auto-commit on cleanup ONLY fires for clean Generated turns the user
/// hasn't asked to discard. The bug we're fixing: previously the cleanup
/// path auto-committed any worktree dirt on safety-net abort, the
/// per-commit hook fired for that commit, and a spurious ChangeProposed
/// landed for partial work. Pin every input combination to the expected
/// commit/no-commit decision.
#[test]
fn should_auto_commit_on_cleanup_table() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let cases: &[(bool, &Option<TerminalKind>, bool, &str)] = &[
        // Discard always wins — never commit, even on a clean Generated.
        (
            true,
            &Some(TerminalKind::Generated),
            false,
            "discard wins over Generated",
        ),
        (true, &None, false, "discard wins over safety-net abort"),
        // Generated + not discarded → the only commit path.
        (
            false,
            &Some(TerminalKind::Generated),
            true,
            "clean Generated commits",
        ),
        // Every non-Generated terminal refuses, regardless of discard.
        (
            false,
            &Some(TerminalKind::Failed {
                error: "stream interrupted".into(),
            }),
            false,
            "Failed must NOT auto-commit — half-assed work",
        ),
        (
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            false,
            "Canceled must NOT auto-commit — user stopped mid-work",
        ),
        (
            false,
            &Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
            false,
            "Aborted (shutdown) must NOT auto-commit — mid-work",
        ),
        // Safety-net abort sets terminal to None inside cleanup. THIS is
        // the regression test for the original bug: cleanup auto-commit
        // on safety-net fired the per-commit hook → spurious ChangeProposed.
        (
            false,
            &None,
            false,
            "safety-net abort (None terminal) must NOT auto-commit",
        ),
    ];
    for (should_discard, terminal, expected, label) in cases {
        assert_eq!(
            should_auto_commit_on_cleanup(*should_discard, terminal),
            *expected,
            "{label}",
        );
    }
}

#[test]
fn conflict_resolution_cleanup_only_applies_clean_generated_turns() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    assert_eq!(
        conflict_resolution_cleanup_action(false, &Some(TerminalKind::Generated), false),
        ConflictResolutionCleanupAction::Apply,
        "a clean generated merge-fix turn is the only path that may land the apply"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(true, &Some(TerminalKind::Generated), false),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution incomplete — merge aborted. The change is still pending; try applying again.",
        },
        "unmerged paths must keep the original change pending even after a generated turn"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            false
        ),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution canceled — merge aborted. The change is still pending; try applying again.",
        },
        "canceling the merge-fix session must not allow cleanup to fast-forward main"
    );
    for terminal in [
        Some(TerminalKind::Failed {
            error: "api dropped".to_string(),
        }),
        Some(TerminalKind::Aborted(AbortCause::EngineShutdown)),
        None,
    ] {
        assert_eq!(
            conflict_resolution_cleanup_action(false, &terminal, false),
            ConflictResolutionCleanupAction::Abort {
                message: "Conflict resolution did not finish cleanly — merge aborted. The change is still pending; try applying again.",
            },
            "non-generated terminal {terminal:?} must keep the original change pending"
        );
    }
}

/// A pending auto-recovery continuation transfers the merge duty instead of
/// aborting — for interrupted turns, regardless of leftover unmerged files (a
/// stray-killed merge turn is EXPECTED to leave conflicts behind for the
/// continuation to finish). The 2026-07-10 incident: a stray SIGTERM 84ms
/// after the merge-session spawn aborted the apply and raced destructive git
/// cleanup against the continuation that was already resuming the same turn.
///
/// Two outcomes still beat the hand-off:
/// - a clean `Generated` turn with the merge fully committed applies on the
///   spot (an external-watchdog false positive racing a natural end must not
///   defer finished work to a fragile `--resume`);
/// - a user cancel aborts (Stop means "don't land this merge").
#[test]
fn conflict_resolution_hands_off_when_continuation_pending() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    for (has_unmerged, terminal) in [
        (true, None),
        (false, None),
        (
            true,
            Some(TerminalKind::Failed {
                error: "killed".to_string(),
            }),
        ),
        (false, Some(TerminalKind::Aborted(AbortCause::SafetyNet))),
        // Generated with unmerged files left behind: the turn didn't finish
        // the merge — the continuation picks it up.
        (true, Some(TerminalKind::Generated)),
    ] {
        assert_eq!(
            conflict_resolution_cleanup_action(has_unmerged, &terminal, true),
            ConflictResolutionCleanupAction::HandOff,
            "continuation_pending must hand off (has_unmerged={has_unmerged}, terminal {terminal:?})"
        );
    }

    assert_eq!(
        conflict_resolution_cleanup_action(false, &Some(TerminalKind::Generated), true),
        ConflictResolutionCleanupAction::Apply,
        "a committed merge with a clean Generated end applies immediately — \
         never deferred to the continuation (watchdog false-positive race)"
    );
    assert_eq!(
        conflict_resolution_cleanup_action(
            false,
            &Some(TerminalKind::Canceled(CancelCause::UserStop)),
            true
        ),
        ConflictResolutionCleanupAction::Abort {
            message: "Conflict resolution canceled — merge aborted. The change is still pending; try applying again.",
        },
        "a user Stop aborts even with a continuation pending"
    );
}

/// Abort cleanup deletes only what the merge attempt created: the temp
/// worktree + temp branch of the Tier-3 shape, and only when this session
/// actually ran on that temp branch. A Tier-2 merge (no `merge_temp_branch`)
/// ran in the thread's own worktree on the real change branch — deleting
/// those would destroy the user's committed work. And a STALE recorded temp
/// branch (a pruned-temp re-attach put the session on the change branch
/// while the row still carries the dead attempt's columns) must not condemn
/// the thread worktree the session actually ran in.
#[test]
fn conflict_abort_deletes_only_temp_merge_state() {
    assert!(conflict_abort_deletes_temp_state(
        Some("merge-tmp/x"),
        "merge-tmp/x"
    ));
    assert!(!conflict_abort_deletes_temp_state(
        Some("merge-tmp/x"),
        "claude-code/20260707-abc"
    ));
    assert!(!conflict_abort_deletes_temp_state(
        None,
        "claude-code/20260707-abc"
    ));
}

/// Only "no text AND no images" counts as silent — image-only turns are
/// real user content. Skipping the image check here freezes the thread row
/// at `running` after CC delivers an answer, because `classify_result`
/// then suppresses both the terminal event and `CodingAgentIdled`.
#[test]
fn image_only_message_is_not_silent_resume() {
    assert!(!is_silent_resume(true, true));
    assert!(is_silent_resume(true, false));
    assert!(!is_silent_resume(false, false));
    assert!(!is_silent_resume(false, true));
}

/// Pin every input combination of `classify_session_end_action`. The
/// invariants this guards:
///   - `(has_commits=true, files_empty=true, external=false)` → `KeepEmptyBranch`,
///     not `Propose`. This is the phantom-Change regression — observed
///     when CC's auto-commit on cleanup advances the branch ref while
///     the user concurrently clicked Apply Now, leaving the post-Apply
///     branch with commits whose contents already live on main. Without
///     this filter the engine emits a `ChangeProposed` with no files
///     and the thread title as the fallback description, which the
///     frontend renders as a pending Change the user can only Discard.
///   - External repos with commits keep their branch regardless of
///     `files_empty` — the user owns push/PR there; deleting the ref
///     because the net diff happens to be zero would lose work.
///   - No commits on the branch always routes to `KeepEmptyBranch`,
///     regardless of the other inputs (the diff signal is moot). The branch
///     is KEPT (the session is resumable) — `KeepEmptyBranch` no longer
///     deletes (the thread-9e37697e data-loss fix); only the explicit
///     Discard / conflict paths `git branch -D`.
///   - A user cancel (`user_canceled=true`) keeps the branch
///     (`KeepCanceledBranch`) so the session stays resumable — even with no
///     commits. It carries the cancel-specific "half-finished, don't propose"
///     semantics distinct from the ordinary `KeepEmptyBranch`, ranking below
///     the external/crash keep-branch arms.
#[test]
fn classify_session_end_action_table() {
    use SessionEndAction::*;
    let cases = [
        // (has_commits, files_empty, is_external, safety_net_fired, user_canceled) → action
        //
        // Healthy turn (safety_net_fired=false, user_canceled=false) — same as before:
        ((true, false, false, false, false), Propose),
        ((true, true, false, false, false), KeepEmptyBranch), // phantom-Change regression
        ((true, false, true, false, false), KeepExternalBranch),
        ((true, true, true, false, false), KeepExternalBranch),
        ((false, false, false, false, false), KeepEmptyBranch),
        ((false, true, false, false, false), KeepEmptyBranch),
        ((false, false, true, false, false), KeepEmptyBranch),
        ((false, true, true, false, false), KeepEmptyBranch),
        //
        // Safety-net fired — CC died mid-stream:
        //   - In our own repo with commits: CrashedKeepBranch (keep work,
        //     no ChangeProposed). files_empty doesn't matter; even an
        //     empty-diff commit is partial work.
        //   - External repo with commits: still KeepExternalBranch — user
        //     owns the ref regardless of how the session ended.
        //   - No commits: KeepEmptyBranch — nothing to propose (branch still kept, resumable).
        ((true, false, false, true, false), CrashedKeepBranch),
        ((true, true, false, true, false), CrashedKeepBranch),
        ((true, false, true, true, false), KeepExternalBranch),
        ((true, true, true, true, false), KeepExternalBranch),
        ((false, false, false, true, false), KeepEmptyBranch),
        ((false, true, false, true, false), KeepEmptyBranch),
        ((false, false, true, true, false), KeepEmptyBranch),
        ((false, true, true, true, false), KeepEmptyBranch),
        //
        // User cancel (Stop = Esc, user_canceled=true) — keep the branch so the
        // session stays resumable; never propose. The grilling-cancel bug is the
        // no-commits row: it MUST be KeepCanceledBranch, not KeepEmptyBranch.
        ((false, true, false, false, true), KeepCanceledBranch), // grilling cancel (the bug)
        ((false, false, false, false, true), KeepCanceledBranch),
        ((true, false, false, false, true), KeepCanceledBranch), // commits but cancelled → keep, don't propose
        ((true, true, false, false, true), KeepCanceledBranch),
        // External repo and crash arms still win over the cancel arm:
        ((true, false, true, false, true), KeepExternalBranch),
        ((true, false, false, true, true), CrashedKeepBranch), // defensive: can't really co-occur
    ];
    for ((has_commits, files_empty, is_external, safety_net_fired, user_canceled), expected) in
        cases
    {
        assert_eq!(
            classify_session_end_action(
                has_commits,
                files_empty,
                is_external,
                safety_net_fired,
                user_canceled,
            ),
            expected,
            "(has_commits={has_commits}, files_empty={files_empty}, is_external={is_external}, safety_net_fired={safety_net_fired}, user_canceled={user_canceled})",
        );
    }
}

/// The `user_hit_stop` latch must clear after the cancel/abort terminal it
/// produced is emitted — a `Result` is a turn boundary. `Generated` / `Failed`
/// can't co-occur with a set latch (it ranks above both in `classify_result`),
/// so only the cancel/abort terminals need to clear it.
#[test]
fn terminal_clears_user_hit_stop_for_cancel_and_abort_only() {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Canceled(
        CancelCause::UserStop
    )));
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Canceled(
        CancelCause::UserAction
    )));
    assert!(terminal_clears_user_hit_stop(&TerminalKind::Aborted(
        AbortCause::EngineShutdown
    )));
    assert!(!terminal_clears_user_hit_stop(&TerminalKind::Generated));
    assert!(!terminal_clears_user_hit_stop(&TerminalKind::Failed {
        error: "x".to_string()
    }));
}

/// Regression for the "successful turn mislabeled Canceled (twice)" bug.
///
/// When the Stop button interrupts an in-flight turn but follow-ups are already
/// queued, the run loop keeps the subprocess alive to drain them
/// (`TerminateDecision::KeepAliveForFollowup`). CC reports the interrupted turn
/// as a `Result` with `is_error` (`stop_reason=tool_use`) → the first
/// `Canceled`. The drained follow-ups then complete with real, successful output
/// and emit a SECOND `Result`. Without clearing the `user_hit_stop` latch after
/// the first cancel, that second `Result` re-classifies as `Canceled` —
/// stamping finished, committed work as "Canceled" and emitting a phantom
/// second `ResponseCanceled` carrying the completed text (exactly what the user
/// saw). Clearing the latch makes the completion classify as `Generated`.
#[test]
fn inflight_followup_completion_after_cancel_is_generated_not_double_cancel() {
    use crate::engine::thread_events::CancelCause;

    // 1) The Stop interrupts the in-flight turn; CC's Result carries the
    //    interrupt diagnostic (is_error) and the latch is set.
    let mut user_hit_stop = true;
    let (first, _) = classify_result(
        false,
        user_hit_stop,
        false,
        false,
        Some("[ede_diagnostic] result_type=user stop_reason=tool_use".to_string()),
        true,
    );
    assert_eq!(
        first,
        Some(TerminalKind::Canceled(CancelCause::UserStop)),
        "the interrupt must classify as the first Canceled"
    );

    // 2) The run loop clears the latch once that cancel terminal is emitted,
    //    because the subprocess is kept alive to drain inflight follow-ups.
    if let Some(kind) = &first {
        if terminal_clears_user_hit_stop(kind) {
            user_hit_stop = false;
        }
    }
    assert!(
        !user_hit_stop,
        "the user-stop latch must clear after the Canceled terminal so the \
         next Result classifies fresh"
    );

    // 3) The drained follow-ups complete successfully with full text. With the
    //    latch cleared this is a clean completion, NOT a second cancel.
    let (second, _) = classify_result(false, user_hit_stop, false, false, None, false);
    assert_eq!(
        second,
        Some(TerminalKind::Generated),
        "a successful completion after an interrupt superseded by inflight \
         follow-ups must be ResponseGenerated, not a second ResponseCanceled"
    );
}
