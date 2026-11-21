//! The swept window, driven through a round the way the loop drives it.
//!
//! The unit-level sweep tests live beside the pass in `context_mode_tests.rs`.
//! What these add is the ORDER inside a round, which is where a cache anchor is
//! kept or destroyed.

use super::context_mode::{sweep_expired_pairs, SweepSchedule};
use super::working_understanding as wu;
use crate::llm::{ContentBlock, Message, MessageContent};
use std::collections::{HashMap, HashSet};

fn address(byte: u8) -> String {
    format!("evt-{}", format!("{byte:02x}").repeat(16))
}

fn result_msg(id: &str, byte: u8, size: usize) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![
            ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: format!("{}\n[{}]", "x".repeat(size), address(byte)),
            },
            ContentBlock::Text {
                text: "Results above.".to_string(),
            },
        ]),
    }
}

fn holds(messages: &[Message], byte: u8) -> bool {
    let needle = address(byte);
    messages.iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks.iter().any(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.contains(&needle),
            _ => false,
        }),
        MessageContent::Text(_) => false,
    })
}

/// Everything the loop does to the array at the top of a round, in the order
/// `agentic_loop::run` does it. The trim is left out: it fires on the budget,
/// in both modes, and the anchor makes no promise about it.
fn mode_round(
    messages: &mut Vec<Message>,
    first_seen: &mut HashMap<String, usize>,
    round: usize,
    schedule: SweepSchedule,
) {
    use super::context_panel as panel;

    let sweeping = schedule.is_sweep_round(round);
    panel::collapse_tail_blocks(messages, sweeping);
    panel::note_first_seen(first_seen, messages, round);
    sweep_expired_pairs(messages, first_seen, round, schedule);

    let items = panel::tool_result_items(messages, first_seen, round, schedule);
    let document = wu::render(
        &wu::WorkingUnderstanding {
            body: "what round 1 said".to_string(),
            constraints: String::new(),
        },
        &[],
        &wu::RoundNotices::default(),
    );
    let held = HashSet::new();
    let rendered = panel::PanelView {
        items: &items,
        fixed: panel::FixedRegions {
            system_chars: 30_000,
            tool_defs_chars: 50_000,
        },
        held_open: &held,
        budget_chars: 288_000,
        round,
        schedule,
    }
    .render(90_000);
    panel::append_to_tail(messages, rendered);
    panel::append_to_tail(messages, document);
}

/// What the model says back, and what its call returns.
fn reply(messages: &mut Vec<Message>, round: usize, byte: u8) {
    messages.push(Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "reading it".to_string(),
            },
            ContentBlock::ToolUse {
                id: format!("call-{round}"),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "src/main.rs"}),
                thought_signature: None,
            },
        ]),
    });
    messages.push(result_msg(&format!("call-{round}"), byte, 9_000));
}

fn shape(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|m| format!("{:?}", m.content))
        .collect()
}

/// The prefix a request's anchor breakpoint covers, message by message.
///
/// `anthropic_wire::apply_cache_control_to_penultimate_message` marks the end of
/// `messages[len - 2]`, so the cached prefix is everything up to and including
/// it. `None` on round 1, which has no message in front of its tail.
fn anchored_prefix(messages: &[Message]) -> Option<Vec<String>> {
    let anchor = messages.len().checked_sub(2)?;
    Some(shape(&messages[..=anchor]))
}

fn start() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text("do the work".to_string()),
    }]
}

/// Two rounds of the mode, and the array as each one sent it.
fn two_rounds() -> (Vec<Message>, Vec<Message>) {
    let schedule = SweepSchedule::default();
    let mut messages = start();
    let mut first_seen: HashMap<String, usize> = HashMap::new();

    mode_round(&mut messages, &mut first_seen, 1, schedule);
    reply(&mut messages, 1, 1);
    mode_round(&mut messages, &mut first_seen, 2, schedule);
    let round_2 = messages.clone();
    reply(&mut messages, 2, 2);
    mode_round(&mut messages, &mut first_seen, 3, schedule);
    (round_2, messages)
}

/// The invariant the wire's fourth breakpoint rests on.
///
/// Round 3 rewrites the message round 2 sent last, because that is where round
/// 2 appended its panel and where the results it read arrived. Round 2's anchor
/// sits one message in front of that, so the prefix it cached survives.
#[test]
fn the_next_round_leaves_the_anchored_prefix_alone() {
    let (round_2, round_3) = two_rounds();
    let anchored = anchored_prefix(&round_2).expect("round 2 has a message in front of its tail");
    let after = shape(&round_3[..anchored.len()]);
    assert_eq!(
        anchored, after,
        "a round may only rewrite from the previous tail onward"
    );
}

/// The other half, so the test above cannot pass by the mutations doing nothing
/// at all. What round 2 sent last really is rewritten by round 3.
#[test]
fn the_next_round_does_rewrite_the_previous_tail() {
    let (round_2, round_3) = two_rounds();
    let tail = round_2.len() - 1;
    assert_ne!(
        format!("{:?}", round_2[tail].content),
        format!("{:?}", round_3[tail].content),
        "the collapse lands on the previous tail"
    );
}

/// Every tool result still in the array, in order, exactly as it reads.
fn results(messages: &[Message]) -> Vec<String> {
    let mut out = Vec::new();
    for message in messages {
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        for block in blocks {
            if let ContentBlock::ToolResult { content, .. } = block {
                out.push(content.clone());
            }
        }
    }
    out
}

/// Invariant 5. A round that is not a sweep removes nothing and rewrites no
/// result. What it does rewrite is the previous tail's own appends, which the
/// anchored-prefix test above bounds.
#[test]
fn an_ordinary_round_moves_no_result() {
    let schedule = SweepSchedule::default();
    let mut messages = start();
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    mode_round(&mut messages, &mut first_seen, 1, schedule);
    reply(&mut messages, 1, 1);
    mode_round(&mut messages, &mut first_seen, 2, schedule);
    reply(&mut messages, 2, 2);

    let before = results(&messages);
    let count = messages.len();
    mode_round(&mut messages, &mut first_seen, 3, schedule);
    assert_eq!(
        before,
        results(&messages),
        "round 3 touched a result between sweeps"
    );
    assert_eq!(messages.len(), count, "no message left and none arrived");
    assert!(holds(&messages, 1), "nothing leaves between sweeps");
    assert!(holds(&messages, 2));
}

/// Invariant 4, driven through the loop. A result appended during round N is
/// first SEEN at the top of round N+1. Its age at the sweep is therefore one
/// less than the round it was produced on.
#[test]
fn the_sweep_round_takes_everything_past_expiry() {
    let schedule = SweepSchedule::default();
    let mut messages = start();
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    for round in 1..=9 {
        mode_round(&mut messages, &mut first_seen, round, schedule);
        reply(&mut messages, round, round as u8);
    }
    assert!(holds(&messages, 1));
    mode_round(&mut messages, &mut first_seen, 10, schedule);
    assert!(!holds(&messages, 1), "first seen at 2, so age eight");
    assert!(!holds(&messages, 3), "first seen at 4, so age six");
    assert!(
        holds(&messages, 4),
        "first seen at 5, and age five is not past five"
    );
    assert!(holds(&messages, 9));
}

/// Invariant 25 and 26, driven through the loop. A keep written in round N is
/// in force at the top of round N+1, before the pass that drops pairs.
#[test]
fn a_keep_written_in_the_document_survives_the_next_sweep() {
    let schedule = SweepSchedule::default();
    let mut messages = start();
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    for round in 1..=9 {
        mode_round(&mut messages, &mut first_seen, round, schedule);
        reply(&mut messages, round, round as u8);
    }

    // The model writes its keep in round 9's reply, which the loop applies the
    // moment the reply lands.
    let written = format!(
        "{}\n[KEEP OPEN]\n{}\n{}",
        wu::OPEN_REPLACE,
        address(1),
        wu::CLOSE
    );
    let parsed = wu::parse_message(wu::ASSISTANT_ROLE, &written);
    let (_, applied) = wu::apply_spans(&wu::WorkingUnderstanding::default(), &parsed);
    assert_eq!(applied.keep_open, vec![address(1)]);
    for held in &applied.keep_open {
        crate::engine::chat::process::context_panel::hold_open(&mut first_seen, held, 9);
    }

    mode_round(&mut messages, &mut first_seen, 10, schedule);
    assert!(holds(&messages, 1), "the keep moved its clock");
    assert!(!holds(&messages, 2), "everything else still went");
    assert!(!holds(&messages, 3));

    // Invariant 33: the clock moved, it did not stop. The item goes at the
    // sweep after the one it was held past.
    for round in 11..=19 {
        mode_round(&mut messages, &mut first_seen, round, schedule);
    }
    mode_round(&mut messages, &mut first_seen, 20, schedule);
    assert!(!holds(&messages, 1), "a keep is not a pin");
}

/// Invariant 24, at the loop's own resolution step. `panel_first_seen` keeps an
/// address forever, so a keep on something a sweep already took would report
/// success and hold nothing. What the request is CARRYING is the answer.
#[test]
fn a_keep_on_a_swept_address_resolves_to_nothing() {
    let schedule = SweepSchedule::default();
    let mut messages = start();
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    for round in 1..=9 {
        mode_round(&mut messages, &mut first_seen, round, schedule);
        reply(&mut messages, round, round as u8);
    }
    mode_round(&mut messages, &mut first_seen, 10, schedule);
    assert!(!holds(&messages, 1), "the sweep took it");

    // The map still knows it, which is exactly the trap. `is_resident` is what
    // the loop asks, so resolving against the map instead fails here.
    assert!(first_seen.contains_key(&address(1)));
    assert!(
        !super::context_panel::carries_address(&messages, &address(1)),
        "a keep on this address must be reported, not confirmed"
    );
    assert!(
        super::context_panel::carries_address(&messages, &address(9)),
        "and this one holds"
    );
}

/// A result with no address is not addressable, so no sweep can take it: there
/// would be no way back.
#[test]
fn a_result_with_no_address_is_never_swept() {
    let mut messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "call-0".to_string(),
            content: "x".repeat(9_000),
        }]),
    }];
    let sweep = sweep_expired_pairs(&mut messages, &HashMap::new(), 10, SweepSchedule::default());
    assert!(sweep.removed.is_empty());
    assert_eq!(messages.len(), 1);
}
