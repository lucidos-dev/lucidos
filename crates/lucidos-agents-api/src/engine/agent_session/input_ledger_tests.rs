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

fn sent(text: &str) -> AgentInput {
    AgentInput {
        defer_on_steer_failure: false,
        text: text.into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    }
}

fn read(input: &AgentInput) -> AgentEvent {
    AgentEvent::InputRead { input: input.uuid }
}

fn read_of(event_ids: &[Uuid]) -> Settled {
    read_flagged(event_ids, false)
}

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
        provider_error: None,
        turn_end: None,
    }
}

fn at_idle(ledger: &InputLedger) -> TerminateDecision {
    terminate_decision(0, ledger.owed(), false, false)
}

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

#[test]
fn coalesced_messages_are_all_read_with_the_first_prompt() {
    let [leader, follower] = [Uuid::new_v4(), Uuid::new_v4()];
    let prompt = sent("leader");
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![leader, follower], &prompt, None);
    assert_eq!(ledger.observe(&read(&prompt)), read_of(&[leader, follower]));
    assert_eq!(ledger.owed(), 0);
}

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

#[test]
fn only_a_child_report_is_takeable() {
    let mut inputs = InputLedger::new();
    let (message, card, prompt) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    inputs.forwarded(vec![message], &sent("also check the totals"), None);
    inputs.forwarded_child_report(card, &sent("[child report]"), None, "report".into());
    inputs.forwarded(vec![prompt], &sent("run /harden"), None);

    assert_eq!(
        inputs.child_reports(),
        vec![ChildReport {
            card_id: card,
            text: "report".into(),
            sent: "[child report]".into(),
        }]
    );
    assert_eq!(inputs.owed_child_reports(), 1);
    inputs.child_report_taken(card);
    inputs.child_report_taken(message);
    assert!(inputs.child_reports().is_empty());
    assert_eq!(inputs.owed(), 2, "the follow-up and the prompt stay owed");
}

#[test]
fn a_taken_report_settles_nothing_later() {
    let mut inputs = InputLedger::new();
    let card = Uuid::new_v4();
    let report = sent("[child report]");
    inputs.forwarded_child_report(card, &report, None, "report".into());
    inputs.child_report_taken(card);
    assert_eq!(
        inputs.observe(&AgentEvent::InputRead { input: report.uuid }),
        Settled::Nothing
    );
    assert_eq!(inputs.owed(), 0);
}

#[test]
fn a_read_report_is_not_takeable() {
    let mut inputs = InputLedger::new();
    let card = Uuid::new_v4();
    let report = sent("[child report]");
    inputs.forwarded_child_report(card, &report, None, "report".into());
    inputs.observe(&AgentEvent::InputRead { input: report.uuid });
    assert!(inputs.child_reports().is_empty());
    assert_eq!(inputs.owed_child_reports(), 0);
}
