//! What a coding-agent session still owes its agent, driven by the agent's own
//! event stream.

use super::*;
use crate::engine::agent_session::lifecycle::{terminate_decision, TerminateDecision};

fn text(t: &str) -> AgentEvent {
    AgentEvent::Message {
        role: "assistant".into(),
        text: t.into(),
        opens_block: true,
    }
}

fn tool_use(name: &str) -> AgentEvent {
    AgentEvent::ToolUse {
        name: name.into(),
        input: serde_json::json!({}),
        id: "toolu-1".into(),
        parent_tool_use_id: None,
        api_call_id: None,
    }
}

fn tool_result() -> AgentEvent {
    AgentEvent::ToolResult {
        output: "Red".into(),
        status: "success".into(),
        id: "toolu-1".into(),
        parent_tool_use_id: None,
    }
}

/// The input the agent was sent.
fn sent(text: &str) -> AgentInput {
    AgentInput {
        text: text.into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    }
}

/// The agent's read of `input`: Claude Code's replay under its uuid, or Codex
/// starting its turn.
fn read(input: &AgentInput) -> AgentEvent {
    AgentEvent::InputRead { input: input.uuid }
}

/// What a read of inputs that did not go verbatim settles: one entry per event.
fn read_of(event_ids: &[Uuid]) -> Settled {
    read_flagged(event_ids, false)
}

/// What a read of a slash command settles: it went verbatim.
fn verbatim_read_of(event_ids: &[Uuid]) -> Settled {
    read_flagged(event_ids, true)
}

fn read_flagged(event_ids: &[Uuid], verbatim: bool) -> Settled {
    Settled::Read(
        event_ids
            .iter()
            .map(|&event_id| ReadInput { event_id, verbatim })
            .collect(),
    )
}

/// A replay of Claude Code's own output, under a uuid no input has.
fn replay_of_its_own_output() -> AgentEvent {
    AgentEvent::InputRead {
        input: Uuid::new_v4(),
    }
}

fn result() -> AgentEvent {
    AgentEvent::Result {
        text: String::new(),
        duration_ms: 1,
        error: None,
    }
}

/// The idle decision with no message waiting in the channel and no redirect
/// armed, so only what the agent still owes decides it.
fn at_idle(ledger: &InputLedger) -> TerminateDecision {
    terminate_decision(0, ledger.owed(), false)
}

/// The held-message repro. A parent's follow-up was held behind the child's
/// question (ADR 0256). The human answered in band, and the release forwarded
/// the message right after the answer's tool result. The agent then wrote its
/// closing text with no further tool call, so Claude Code ran the message as a
/// second turn. The session must survive the first turn's `Result` for it.
#[test]
fn a_message_released_after_an_in_band_answer_is_read_as_the_next_turn() {
    let [prompt_id, released_id] = [Uuid::new_v4(), Uuid::new_v4()];
    let (prompt, released) = (sent("prompt"), sent("released"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt_id], &prompt, None);

    let prompt_read = ledger.observe(&read(&prompt));
    ledger.observe(&tool_use("AskUserQuestion"));
    ledger.observe(&tool_result());
    ledger.forwarded(vec![released_id], &released, None);
    ledger.observe(&text("Red it is."));
    ledger.observe(&text(" Done."));
    ledger.observe(&result());
    assert!(
        matches!(
            at_idle(&ledger),
            TerminateDecision::KeepAliveForFollowup { .. }
        ),
        "the released message is still unread, so the subprocess must stay up for it"
    );

    assert_eq!(ledger.observe(&read(&released)), read_of(&[released_id]));
    ledger.observe(&text("Noted."));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
    assert_eq!(prompt_read, read_of(&[prompt_id]));
}

/// Claude Code folds inputs that arrive before a later tool call into the
/// running turn and replays each one there. That turn's `Result` then leaves
/// nothing owed, so the subprocess exits at idle as before.
#[test]
fn inputs_folded_into_one_turn_leave_nothing_owed_at_its_result() {
    let [prompt_id, second_id, third_id] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let (prompt, second, third) = (sent("prompt"), sent("second"), sent("third"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt_id], &prompt, None);
    ledger.observe(&read(&prompt));
    ledger.observe(&tool_use("Read"));
    ledger.forwarded(vec![second_id], &second, None);
    ledger.forwarded(vec![third_id], &third, None);
    ledger.observe(&tool_result());
    assert_eq!(ledger.observe(&read(&second)), read_of(&[second_id]));
    assert_eq!(ledger.observe(&read(&third)), read_of(&[third_id]));
    ledger.observe(&text("Both done."));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// The probed queue behind a busy turn. Claude Code replays each queued input
/// under its own uuid, then replays all of them joined under the last one's.
/// Every input is read once, and the joined replay reads nothing twice.
#[test]
fn queued_inputs_are_each_read_by_their_own_replay() {
    let [prompt_id, a_id, b_id, c_id] = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let (prompt, a, b, c) = (sent("prompt"), sent("say A"), sent("say B"), sent("say C"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt_id], &prompt, None);
    ledger.forwarded(vec![a_id], &a, None);
    ledger.forwarded(vec![b_id], &b, None);
    ledger.forwarded(vec![c_id], &c, None);
    ledger.observe(&read(&prompt));
    ledger.observe(&result());
    assert_eq!(ledger.observe(&read(&a)), read_of(&[a_id]));
    assert_eq!(ledger.observe(&read(&b)), read_of(&[b_id]));
    assert_eq!(ledger.observe(&read(&c)), read_of(&[c_id]));
    assert_eq!(ledger.observe(&read(&c)), Settled::Nothing, "already read");
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Recovery resumes with the continuation as the first prompt, keyed on its
/// `ContinuationRequested`. Its read settles that entry alone, so a follow-up
/// forwarded during the continuation turn stays owed.
#[test]
fn a_continuation_read_settles_only_the_continuation() {
    let [continuation_id, follow_up_id] = [Uuid::new_v4(), Uuid::new_v4()];
    let continuation = sent("continuation");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![continuation_id], &continuation, None);
    assert_eq!(
        ledger.observe(&read(&continuation)),
        read_of(&[continuation_id])
    );
    ledger.observe(&text("Continuing."));
    ledger.forwarded(vec![follow_up_id], &sent("follow_up"), None);
    ledger.observe(&text(" Almost done."));
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));
}

/// A local command such as `/cost` answers with no model turn. Claude Code
/// still replays it under its uuid, so it is read like any input and the
/// session exits at idle.
#[test]
fn a_local_command_is_read_by_its_own_replay() {
    let cost_id = Uuid::new_v4();
    let cost = sent("/cost");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![cost_id], &cost, None);
    assert_eq!(ledger.observe(&read(&cost)), verbatim_read_of(&[cost_id]));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// The probed `/compact`. Claude Code replays its stdout under a uuid of its
/// own, then replays the command itself. Neither may read the follow-up queued
/// behind it, or the idle reads nothing owed and kills the follow-up's turn.
#[test]
fn a_compaction_never_reads_the_input_queued_behind_it() {
    let [compact_id, follow_up_id] = [Uuid::new_v4(), Uuid::new_v4()];
    let compact = sent("/compact");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![compact_id], &compact, None);
    ledger.forwarded(vec![follow_up_id], &sent("and then the totals"), None);
    assert_eq!(
        ledger.observe(&replay_of_its_own_output()),
        Settled::Nothing
    );
    assert_eq!(
        ledger.observe(&read(&compact)),
        verbatim_read_of(&[compact_id])
    );
    assert_eq!(ledger.owed(), 1, "the follow-up is still unread");
}

/// The slow-hook window. Claude Code reads an input only after its
/// `UserPromptSubmit` hook returns, and says nothing the engine parses while
/// the hook runs. Nothing settles the input meanwhile, so the idle keeps the
/// subprocess, and the late read lands on its input.
#[test]
fn a_read_that_follows_a_slow_hook_still_reads_its_input() {
    let follow_up_id = Uuid::new_v4();
    let follow_up = sent("follow-up");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![follow_up_id], &follow_up, None);
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));

    assert_eq!(
        ledger.observe(&read(&follow_up)),
        read_of(&[follow_up_id]),
        "the agent's own read is the only thing that settles its input"
    );
}

/// An engine re-entry, such as a child's completion, is owed like any input,
/// but carries no message to mark read.
#[test]
fn an_input_with_nothing_to_acknowledge_is_still_owed() {
    let wake = sent("child finished");
    let mut ledger = InputLedger::new();
    ledger.forwarded(Vec::new(), &wake, None);
    ledger.observe(&result());
    assert_eq!(ledger.owed(), 1);
    assert_eq!(ledger.observe(&read(&wake)), read_of(&[]));
    assert_eq!(ledger.owed(), 0);
}

/// The run loop can forward an input and only then receive a `Result` the agent
/// produced before the input arrived. No ordering guess is needed: the input is
/// owed until it is read.
#[test]
fn a_result_buffered_before_a_forward_settles_nothing() {
    let prompt = sent("prompt");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![Uuid::new_v4()], &prompt, None);
    ledger.observe(&read(&prompt));
    ledger.forwarded(vec![Uuid::new_v4()], &sent("follow_up"), None);
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));
}

/// A read settles the input it names and no other, even an older one still
/// owed: nothing is inferred from the order Claude Code consumed them in.
#[test]
fn a_read_settles_only_the_input_it_names() {
    let [first_id, second_id] = [Uuid::new_v4(), Uuid::new_v4()];
    let second = sent("second");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![first_id], &sent("first"), None);
    ledger.forwarded(vec![second_id], &second, None);
    assert_eq!(ledger.observe(&read(&second)), read_of(&[second_id]));
    assert_eq!(ledger.owed(), 1, "the first is still owed");
}

/// A read says whether its input went verbatim, so the turn gap knows the
/// command carried no note. Every event of a coalesced write shares its flag.
#[test]
fn a_read_says_whether_its_input_went_verbatim() {
    let [command, leader, follower] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let (cost, framed) = (sent("/cost"), sent("[Note from engine: …]\n\nhello"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![command], &cost, None);
    ledger.forwarded(vec![leader, follower], &framed, None);
    assert_eq!(ledger.observe(&read(&cost)), verbatim_read_of(&[command]));
    assert_eq!(ledger.observe(&read(&framed)), read_of(&[leader, follower]));
}

/// Codex runs one turn per input and reports each read when that turn starts.
/// After the first turn's `Result` the queued input is still unread, so the
/// driver must stay up to run it.
#[test]
fn codex_keeps_the_driver_up_for_a_queued_turn() {
    let [prompt_id, queued_id] = [Uuid::new_v4(), Uuid::new_v4()];
    let (prompt, queued) = (sent("prompt"), sent("queued"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt_id], &prompt, None);
    ledger.observe(&read(&prompt));
    ledger.forwarded(vec![queued_id], &queued, None);
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));

    assert_eq!(ledger.observe(&read(&queued)), read_of(&[queued_id]));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Codex rejected the turn that carried an input. Nobody will ever read it, so
/// it leaves the ledger unread and the idle that follows owes nothing.
#[test]
fn a_rejected_input_leaves_unread() {
    let rejected = sent("prompt");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![Uuid::new_v4()], &rejected, None);
    assert_eq!(
        ledger.observe(&AgentEvent::InputRejected {
            input: rejected.uuid
        }),
        Settled::Rejected
    );
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Messages sent before a session starts are coalesced into its first prompt.
/// The agent reads them in one write, so one read marks every one of them.
#[test]
fn coalesced_messages_are_all_read_with_the_first_prompt() {
    let [leader, follower] = [Uuid::new_v4(), Uuid::new_v4()];
    let prompt = sent("leader");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![leader, follower], &prompt, None);
    assert_eq!(ledger.observe(&read(&prompt)), read_of(&[leader, follower]));
    assert_eq!(ledger.owed(), 0);
}

/// A withdrawn input leaves the ledger, so a later read reads the input it
/// names and never the withdrawn one.
#[test]
fn a_withdrawn_input_is_never_read_in_place_of_a_later_one() {
    let [a, b, c] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let (a_input, b_input, c_input) = (sent("a"), sent("b"), sent("c"));
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![a], &a_input, None);
    ledger.forwarded(vec![b], &b_input, None);
    ledger.forwarded(vec![c], &c_input, None);
    assert_eq!(ledger.observe(&read(&a_input)), read_of(&[a]));

    assert_eq!(ledger.withdrawable(b), Some(b_input.uuid));
    ledger.withdrawn(b);
    assert_eq!(ledger.owed(), 1);
    assert_eq!(
        ledger.observe(&read(&b_input)),
        Settled::Nothing,
        "b was dropped"
    );
    assert_eq!(ledger.observe(&read(&c_input)), read_of(&[c]));
}

/// Only an owed input can be withdrawn, and only when its write carried that
/// message alone: a coalesced write cannot be taken back one message at a time.
#[test]
fn only_an_owed_input_written_alone_is_withdrawable() {
    let [read_id, leader, follower, never_sent] = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let already_read = sent("read");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![read_id], &already_read, None);
    ledger.observe(&read(&already_read));
    ledger.forwarded(vec![leader, follower], &sent("coalesced"), None);
    assert_eq!(ledger.withdrawable(read_id), None, "already read");
    assert_eq!(ledger.withdrawable(leader), None, "coalesced");
    assert_eq!(ledger.withdrawable(follower), None, "coalesced");
    assert_eq!(ledger.withdrawable(never_sent), None, "never forwarded");
}

/// A withdrawal waits for its tombstone. Until it is recorded, a retry must be
/// able to record it again, so the ledger keeps asking for it.
#[test]
fn a_withdrawal_awaits_its_tombstone_until_recorded() {
    let id = Uuid::new_v4();
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![id], &sent("queued"), None);
    ledger.withdrawn(id);
    assert!(ledger.awaits_tombstone(id));
    assert_eq!(
        ledger.withdrawable(id),
        None,
        "the agent already dropped it"
    );
    ledger.tombstone_recorded(id);
    assert!(!ledger.awaits_tombstone(id));
}

/// Invariant I10: Apply's wait ends only when the turn that read its prompt
/// ends. The prompt lands while a turn is running, and Claude Code reads it as
/// the next turn. The running turn's `Result` must not answer it.
#[test]
fn a_prompt_sent_mid_turn_is_answered_by_the_turn_that_read_it() {
    let (completion, mut answered) = tokio::sync::oneshot::channel();
    let mut ledger = InputLedger::new();
    let harden = sent("/harden");
    ledger.forwarded(Vec::new(), &harden, Some(completion));

    ledger.observe(&result());
    ledger.turn_ended();
    assert_eq!(
        answered.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty),
        "the turn that was already running did not read the prompt"
    );

    ledger.observe(&read(&harden));
    assert_eq!(
        answered.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty),
        "the turn that read it is still running"
    );

    ledger.observe(&tool_use("Bash"));
    ledger.observe(&result());
    ledger.turn_ended();
    assert_eq!(answered.try_recv(), Ok(InputAnswered::TurnEnded));
}

/// A prompt the agent rejects is never read, so its waiter hears the
/// rejection rather than waiting for a turn that will not come.
#[test]
fn a_rejected_prompt_answers_its_waiter() {
    let (completion, mut answered) = tokio::sync::oneshot::channel();
    let mut ledger = InputLedger::new();
    let harden = sent("/harden");
    ledger.forwarded(Vec::new(), &harden, Some(completion));

    assert_eq!(
        ledger.observe(&AgentEvent::InputRejected { input: harden.uuid }),
        Settled::Rejected
    );
    assert_eq!(answered.try_recv(), Ok(InputAnswered::Rejected));
}

/// A session that ends with the prompt unanswered drops its ledger, and the
/// waiter reads the session as gone rather than hanging.
#[test]
fn a_session_that_ends_first_closes_the_waiter() {
    let (completion, mut answered) = tokio::sync::oneshot::channel();
    let mut ledger = InputLedger::new();
    let harden = sent("/harden");
    ledger.forwarded(Vec::new(), &harden, Some(completion));
    ledger.observe(&read(&harden));

    drop(ledger);
    assert_eq!(
        answered.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}

/// The strand repro. An effort pick interrupted a new session's first turn
/// before Claude Code read its prompt, and no read ever came. Owing the prompt
/// kept the process up, and the effort resume never ran.
#[test]
fn an_interrupt_before_the_first_read_owes_nothing() {
    let mut ledger = InputLedger::new();
    let prompt_event = Uuid::new_v4();
    ledger.forwarded(vec![prompt_event], &sent("Hey"), None);

    ledger.observe(&result());
    assert_eq!(ledger.turn_interrupted(), [prompt_event]);
    ledger.turn_ended();
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Claude Code reads its queue after an interrupt, so only the input the
/// interrupt discarded stops being owed. A turn that read its input discards
/// nothing.
#[test]
fn an_interrupt_keeps_the_inputs_queued_behind_the_turn_owed() {
    let (prompt, queued) = (sent("Hey"), sent("and also"));
    let prompt_event = Uuid::new_v4();
    let mut unread_first = InputLedger::new();
    unread_first.forwarded(vec![prompt_event], &prompt, None);
    unread_first.forwarded(vec![Uuid::new_v4()], &queued, None);
    unread_first.observe(&result());
    assert_eq!(unread_first.turn_interrupted(), [prompt_event]);
    unread_first.turn_ended();
    assert!(unread_first.owes(queued.uuid));
    assert!(!unread_first.owes(prompt.uuid));

    let mut read_first = InputLedger::new();
    read_first.forwarded(vec![Uuid::new_v4()], &prompt, None);
    read_first.observe(&read(&prompt));
    read_first.forwarded(vec![Uuid::new_v4()], &queued, None);
    read_first.observe(&result());
    assert!(read_first.turn_interrupted().is_empty());
    read_first.turn_ended();
    assert!(read_first.owes(queued.uuid));
}
