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
    }
}

fn tool_result() -> AgentEvent {
    AgentEvent::ToolResult {
        output: "Red".into(),
        status: "success".into(),
        id: "toolu-1".into(),
    }
}

/// The input the agent was sent.
fn sent(text: &str) -> AgentInput {
    AgentInput {
        text: text.into(),
        images: Vec::new(),
    }
}

/// Claude Code's replay of plain inputs, joined as it joins them.
fn replay(texts: &[&str]) -> AgentEvent {
    AgentEvent::InputRead(Some(ReplayedInput {
        texts: vec![texts.join("\n")],
        images: 0,
    }))
}

/// A read that names no input, as Codex reports one.
const READ: AgentEvent = AgentEvent::InputRead(None);

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
    let prompt = Uuid::new_v4();
    let released = Uuid::new_v4();
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt], &sent("prompt"));

    let prompt_read = ledger.observe(&replay(&["prompt"]));
    ledger.observe(&tool_use("AskUserQuestion"));
    ledger.observe(&tool_result());
    ledger.forwarded(vec![released], &sent("released"));
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

    assert_eq!(ledger.observe(&replay(&["released"])), vec![released]);
    ledger.observe(&text("Noted."));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
    assert_eq!(
        prompt_read,
        vec![prompt],
        "the first prompt's replay marks it read"
    );
}

/// Claude Code folds inputs that arrive before a later tool call into the
/// running turn and replays each one there. That turn's `Result` then leaves
/// nothing owed, so the subprocess exits at idle as before.
#[test]
fn inputs_folded_into_one_turn_leave_nothing_owed_at_its_result() {
    let [prompt, second, third] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt], &sent("prompt"));
    ledger.observe(&replay(&["prompt"]));
    ledger.observe(&tool_use("Read"));
    ledger.forwarded(vec![second], &sent("second"));
    ledger.forwarded(vec![third], &sent("third"));
    ledger.observe(&tool_result());
    assert_eq!(ledger.observe(&replay(&["second"])), vec![second]);
    assert_eq!(ledger.observe(&replay(&["third"])), vec![third]);
    ledger.observe(&text("Both done."));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Recovery resumes with the continuation as the first prompt, keyed on its
/// `ContinuationRequested`. Its replay settles that entry alone, so a follow-up
/// forwarded during the continuation turn stays owed.
#[test]
fn a_continuation_replay_settles_only_the_continuation() {
    let [continuation, follow_up] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![continuation], &sent("continuation"));
    assert_eq!(
        ledger.observe(&replay(&["continuation"])),
        vec![continuation]
    );
    ledger.observe(&text("Continuing."));
    ledger.forwarded(vec![follow_up], &sent("follow_up"));
    ledger.observe(&text(" Almost done."));
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));
}

/// A local command such as `/cost` runs a turn and never replays. After the
/// silent grace the engine takes it as answered, marks it read, and exits.
#[test]
fn an_input_answered_without_a_replay_settles_after_the_silent_grace() {
    let cost = Uuid::new_v4();
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![cost], &sent("cost"));
    ledger.observe(&result());
    assert_eq!(ledger.owed(), 1);

    assert_eq!(ledger.settle_silent(), vec![cost]);
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// An engine re-entry, such as a child's completion, is owed like any input,
/// but carries no message to mark read.
#[test]
fn an_input_with_nothing_to_acknowledge_is_still_owed() {
    let mut ledger = InputLedger::new();
    ledger.forwarded(Vec::new(), &sent("child finished"));
    ledger.observe(&result());
    assert_eq!(ledger.owed(), 1);
    assert!(ledger.observe(&READ).is_empty());
    assert_eq!(ledger.owed(), 0);
}

/// The run loop can forward an input and only then receive a `Result` the agent
/// produced before the input arrived. No ordering guess is needed: the input is
/// owed until it is replayed.
#[test]
fn a_result_buffered_before_a_forward_settles_nothing() {
    let [prompt, follow_up] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt], &sent("prompt"));
    ledger.observe(&READ);
    ledger.forwarded(vec![follow_up], &sent("follow_up"));
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));
}

/// Codex runs one turn per input and reports each read when that turn starts.
/// After the first turn's `Result` the queued input is still unread, so the
/// driver must stay up to run it.
#[test]
fn codex_keeps_the_driver_up_for_a_queued_turn() {
    let [prompt, queued] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt], &sent("prompt"));
    ledger.observe(&READ);
    ledger.forwarded(vec![queued], &sent("queued"));
    ledger.observe(&result());
    assert!(matches!(
        at_idle(&ledger),
        TerminateDecision::KeepAliveForFollowup { unread: 1, .. }
    ));

    assert_eq!(ledger.observe(&READ), vec![queued]);
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// Messages sent before a session starts are coalesced into its first prompt.
/// The agent reads them in one write, so one read marks every one of them.
#[test]
fn coalesced_messages_are_all_read_with_the_first_prompt() {
    let [leader, follower] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![leader, follower], &sent("leader"));
    assert_eq!(ledger.observe(&READ), vec![leader, follower]);
    assert_eq!(ledger.owed(), 0);
}

/// The nightly e2e repro. Two follow-ups reached a busy agent, and Claude Code
/// answered both in one turn behind ONE replay. Reading only the first left the
/// second owed, so the silent grace marked it read after the turn had ended.
#[test]
fn queued_inputs_sharing_one_replay_are_all_read() {
    let [prompt, third, second] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![prompt], &sent("Say exactly: one"));
    ledger.forwarded(vec![third], &sent("Say exactly: three"));
    ledger.forwarded(vec![second], &sent("Say exactly: two"));
    assert_eq!(ledger.observe(&replay(&["Say exactly: one"])), vec![prompt]);
    ledger.observe(&result());
    assert_eq!(
        ledger.observe(&replay(&["Say exactly: three", "Say exactly: two"])),
        vec![third, second]
    );
    ledger.observe(&text("three\ntwo"));
    ledger.observe(&result());
    assert_eq!(at_idle(&ledger), TerminateDecision::Terminate);
}

/// With an image among the queued inputs, Claude Code keeps every input's
/// blocks apart instead of joining their texts. An image-only input sends no
/// text block at all.
#[test]
fn a_replay_with_an_image_reads_every_input_it_carries() {
    let [two, three, picture, four] = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let image = crate::api::ChatImage {
        mime_type: "image/png".into(),
        base64: "AA==".into(),
    };
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![two], &sent("two"));
    ledger.forwarded(
        vec![three],
        &AgentInput {
            text: "three".into(),
            images: vec![image.clone()],
        },
    );
    ledger.forwarded(
        vec![picture],
        &AgentInput {
            text: String::new(),
            images: vec![image],
        },
    );
    ledger.forwarded(vec![four], &sent("four"));
    let replayed = AgentEvent::InputRead(Some(ReplayedInput {
        texts: vec!["two".into(), "three".into(), "four".into()],
        images: 2,
    }));
    assert_eq!(ledger.observe(&replayed), vec![two, three, picture, four]);
    assert_eq!(ledger.owed(), 0);
}

/// A replay of the oldest input alone leaves the others owed.
#[test]
fn a_replay_reads_only_the_inputs_it_carries() {
    let [first, second] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![first], &sent("first"));
    ledger.forwarded(vec![second], &sent("second"));
    assert_eq!(ledger.observe(&replay(&["first"])), vec![first]);
    assert_eq!(ledger.owed(), 1);
}

/// `/compact` replays its output rather than the input. A replay that matches
/// no run of owed inputs reads the oldest one, as every read did before.
#[test]
fn a_replay_matching_no_input_reads_the_oldest() {
    let [compact, follow_up] = [Uuid::new_v4(), Uuid::new_v4()];
    let mut ledger = InputLedger::new();
    ledger.forwarded(vec![compact], &sent("/compact"));
    ledger.forwarded(vec![follow_up], &sent("and then the totals"));
    let output = replay(&["<local-command-stdout>Compacted </local-command-stdout>"]);
    assert_eq!(ledger.observe(&output), vec![compact]);
    assert_eq!(ledger.owed(), 1);
}
