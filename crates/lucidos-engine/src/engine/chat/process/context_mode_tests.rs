//! The context mode's assembly half, on both arms.
//!
//! What these tests reach is what the turn builds: the mode read, the sweep,
//! and the guidance the prompt carries. The end-to-end half belongs to
//! `crates/lucidos-eval`'s manipulation check, which reads the panel's section
//! name off real `ContextCaptured` rows.

use super::*;
use crate::llm::{ContentBlock, Message, MessageContent};

const ADDRESS_A: &str = "evt-0123456789abcdef0123456789abcdef";
const ADDRESS_B: &str = "evt-fedcba9876543210fedcba9876543210";

/// One round's pair, in the shape the loop appends it: an assistant message of
/// tool calls, then a user message of results and the standing instruction.
fn pair(id: &str, address: &str, body: &str, with_prose: bool) -> Vec<Message> {
    let mut assistant: Vec<ContentBlock> = Vec::new();
    if with_prose {
        assistant.push(ContentBlock::Text {
            text: "Let me read that.".to_string(),
        });
    }
    assistant.push(ContentBlock::ToolUse {
        id: id.to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({}),
        thought_signature: None,
    });
    vec![
        Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(assistant),
        },
        Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: format!("{body}\n[{address}]"),
                },
                ContentBlock::Text {
                    text: "Results above.".to_string(),
                },
            ]),
        },
    ]
}

/// Put an image where `build_tool_result_blocks` puts one: in the results
/// message, after every result and before the standing instruction.
fn push_image(messages: &mut [Message]) {
    let last = messages.last_mut().expect("a results message");
    let MessageContent::Blocks(blocks) = &mut last.content else {
        panic!("the results message carries blocks");
    };
    let at = blocks.len() - 1;
    blocks.insert(
        at,
        ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
        },
    );
}

fn holds_image(messages: &[Message]) -> bool {
    messages.iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        MessageContent::Text(_) => false,
    })
}

fn request() -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::Text {
            text: "please read the file".to_string(),
        }]),
    }
}

fn seen(pairs: &[(&str, usize)]) -> std::collections::HashMap<String, usize> {
    pairs
        .iter()
        .map(|(address, round)| ((*address).to_string(), *round))
        .collect()
}

/// The bytes a message puts on the wire. `Message` carries no `PartialEq`, and
/// comparing the serialized form is the stricter check for a cache prefix.
fn wire(messages: &[Message]) -> String {
    serde_json::to_string(messages).expect("messages serialize")
}

fn block_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks.len(),
            MessageContent::Text(_) => 1,
        })
        .sum()
}

fn holds_address(messages: &[Message], address: &str) -> bool {
    messages.iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks.iter().any(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.contains(address),
            _ => false,
        }),
        MessageContent::Text(text) => text.contains(address),
    })
}

fn tool_use_ids(messages: &[Message]) -> Vec<String> {
    let mut ids = Vec::new();
    for message in messages {
        if let MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                if let ContentBlock::ToolUse { id, .. } = block {
                    ids.push(id.clone());
                }
            }
        }
    }
    ids
}

// ---- the mode read ----

#[test]
fn the_mode_reads_off_the_turns_capability_snapshot() {
    assert_eq!(
        ContextMode::from_capabilities(&ToolCapabilities::default()),
        ContextMode::Off
    );
    // `all_open` is the WIDEST array, and the mode closes a family rather than
    // opening one, so the mode is off there. It is turned on by hand.
    assert_eq!(
        ContextMode::from_capabilities(&ToolCapabilities::all_open()),
        ContextMode::Off
    );
    assert_eq!(
        ContextMode::from_capabilities(&ToolCapabilities {
            context_mode: true,
            ..ToolCapabilities::all_open()
        }),
        ContextMode::On
    );
}

/// Invariant 1. The control arm adds no prompt section at all, which is what
/// keeps its baseline the one a build without the mode would send.
#[test]
fn off_adds_no_system_prompt_section() {
    assert!(system_prompt_section(ContextMode::Off, SweepSchedule::default()).is_empty());
    assert!(!system_prompt_section(ContextMode::On, SweepSchedule::default()).is_empty());
}

// ---- the schedule ----

/// Invariant 4. On the defaults an item lives 6 to 15 rounds, averaging the ten
/// decision 1 asks for.
#[test]
fn an_item_lives_six_to_fifteen_rounds_on_the_defaults() {
    let schedule = SweepSchedule::default();
    let mut lives: Vec<usize> = Vec::new();
    for arrived in 1..=schedule.sweep_every_rounds {
        let at = schedule.leaves_at(arrived, arrived);
        lives.push(at - arrived);
    }
    assert_eq!(*lives.iter().min().unwrap(), 6);
    assert_eq!(*lives.iter().max().unwrap(), 15);
    let mean = lives.iter().sum::<usize>() as f64 / lives.len() as f64;
    assert!((mean - 10.5).abs() < 0.01, "mean life was {mean}");
}

#[test]
fn the_pass_runs_on_every_tenth_round() {
    let schedule = SweepSchedule::default();
    assert!(!schedule.is_sweep_round(1));
    assert!(!schedule.is_sweep_round(9));
    assert!(schedule.is_sweep_round(10));
    assert!(!schedule.is_sweep_round(11));
    assert!(schedule.is_sweep_round(20));
    assert_eq!(schedule.rounds_to_next_sweep(9), 1);
    assert_eq!(schedule.rounds_to_next_sweep(10), 10);
}

/// A zero interval would divide by zero on every round, so it is clamped rather
/// than trusted.
#[test]
fn a_zero_interval_is_clamped_to_one() {
    let schedule = SweepSchedule::new(0, 0);
    assert_eq!(schedule.sweep_every_rounds, 1);
    assert!(schedule.is_sweep_round(1));
}

/// Invariant 7. Two items of different ages that share a sweep report the same
/// remainder, because one pass takes them together.
#[test]
fn two_items_sharing_a_sweep_report_the_same_remainder() {
    let schedule = SweepSchedule::default();
    assert_eq!(schedule.leaves_in(1, 8), schedule.leaves_in(3, 8));
}

// ---- the sweep ----

/// Invariant 2. Both blocks go, and nothing is written in their place.
#[test]
fn a_pair_leaves_whole_or_not_at_all() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "the file body", true));
    let before = block_count(&messages);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1)]),
        10,
        SweepSchedule::default(),
    );

    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);
    assert!(!holds_address(&messages, ADDRESS_A));
    assert!(tool_use_ids(&messages).is_empty());
    // The pair, plus the instruction block the results message carried for it.
    assert_eq!(block_count(&messages), before - 3, "the pair and its frame");
    let text: String = messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            MessageContent::Text(text) => text.clone(),
        })
        .collect();
    assert!(!text.contains("let go"), "no stub stands in its place");
}

/// Invariant 3. The API requires a `tool_use` to be answered in the same
/// exchange, so age zero always survives.
#[test]
fn the_current_rounds_pair_always_stays() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "just arrived", false));
    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 10)]),
        10,
        SweepSchedule::default(),
    );
    assert!(sweep.removed.is_empty());
    assert!(holds_address(&messages, ADDRESS_A));
}

/// A screenshot is the single most expensive block a round can carry, and it
/// holds no address. Left behind by its result it is unaddressable: the panel
/// cannot list it, the model cannot keep it, and no later sweep reaches it.
#[test]
fn a_swept_result_takes_its_image_with_it() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "the screen", true));
    push_image(&mut messages);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1)]),
        10,
        SweepSchedule::default(),
    );

    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);
    assert!(!holds_image(&messages), "the image had no other owner");
}

/// The image is dropped by the message, not by one result, because nothing
/// pairs an image to the call that produced it. So a message still holding a
/// live result keeps its image.
#[test]
fn an_image_beside_a_surviving_result_stays() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "the screen", true));
    if let MessageContent::Blocks(blocks) = &mut messages[2].content {
        blocks.insert(
            1,
            ContentBlock::ToolResult {
                tool_use_id: "call-b".to_string(),
                content: format!("still wanted\n[{ADDRESS_B}]"),
            },
        );
    }
    push_image(&mut messages);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1), (ADDRESS_B, 10)]),
        10,
        SweepSchedule::default(),
    );

    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);
    assert!(holds_address(&messages, ADDRESS_B));
    assert!(holds_image(&messages), "a live result could still own it");
}

/// Invariant 5. Nothing moves between sweeps: that is what buys the cache back.
#[test]
fn nothing_leaves_on_a_round_that_is_not_a_sweep() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "ancient", true));
    let before = wire(&messages);

    for round in [1, 2, 5, 9, 11, 19] {
        let mut probe = messages.clone();
        let sweep = sweep_expired_pairs(
            &mut probe,
            &seen(&[(ADDRESS_A, 1)]),
            round,
            SweepSchedule::default(),
        );
        assert!(sweep.removed.is_empty(), "round {round} removed something");
        assert_eq!(wire(&probe), before, "round {round} rewrote the array");
    }
}

/// Invariant 4, at the boundary. Past expiry it goes on the sweep, and not
/// before.
#[test]
fn an_item_leaves_at_the_first_sweep_past_expiry() {
    let schedule = SweepSchedule::default();
    // Arrived on round 5, so at round 10 its age is 5, which is not PAST five.
    let mut at_ten = vec![request()];
    at_ten.extend(pair("call-a", ADDRESS_A, "body", true));
    let sweep = sweep_expired_pairs(&mut at_ten, &seen(&[(ADDRESS_A, 5)]), 10, schedule);
    assert!(sweep.removed.is_empty(), "age five is not past five");

    // Arrived on round 4, so at round 10 its age is 6.
    let mut older = vec![request()];
    older.extend(pair("call-a", ADDRESS_A, "body", true));
    let sweep = sweep_expired_pairs(&mut older, &seen(&[(ADDRESS_A, 4)]), 10, schedule);
    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);
}

/// Invariant 6. Both numbers are read, not inlined: a swept arm must be able to
/// move them without a rebuild.
#[test]
fn both_numbers_are_read_and_not_inlined() {
    let schedule = SweepSchedule::new(2, 4);
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "body", true));

    // Round 4 is a sweep at this interval, and age three is past two.
    let sweep = sweep_expired_pairs(&mut messages, &seen(&[(ADDRESS_A, 1)]), 4, schedule);
    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);

    // Round 10 is NOT a sweep at this interval, however old the item is.
    let mut other = vec![request()];
    other.extend(pair("call-b", ADDRESS_B, "body", true));
    let sweep = sweep_expired_pairs(&mut other, &seen(&[(ADDRESS_B, 1)]), 10, schedule);
    assert!(sweep.removed.is_empty());
}

/// Invariant 30. An assistant message of nothing but `tool_use` blocks is empty
/// once the pair goes, and the provider rejects an empty message.
/// `validate_tool_use_pairing` does not see it.
#[test]
fn a_sweep_never_leaves_an_empty_message() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "body", false));
    assert_eq!(messages.len(), 3);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1)]),
        10,
        SweepSchedule::default(),
    );

    // The text-free assistant message, and the results message the pair left
    // holding nothing but the round's instruction.
    assert_eq!(sweep.messages_dropped, 2);
    assert_eq!(messages.len(), 1, "the request, and nothing of the pair");
    for message in &messages {
        match &message.content {
            MessageContent::Blocks(blocks) => assert!(!blocks.is_empty()),
            MessageContent::Text(text) => assert!(!text.is_empty()),
        }
    }
    crate::llm::validate::validate_tool_use_pairing(&mut messages);
    assert_eq!(messages.len(), 1, "pairing validation finds nothing to fix");
}

/// The results message carries the round's trailing instruction beside the
/// bodies. Once the bodies go it says "Results above" above nothing, which is a
/// placeholder in all but name.
#[test]
fn a_sweep_takes_the_instruction_the_results_leave_behind() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "body", true));
    messages.extend(pair("call-b", ADDRESS_B, "body", true));
    assert_eq!(messages.len(), 5);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1)]),
        10,
        SweepSchedule::default(),
    );

    assert_eq!(sweep.removed.len(), 1, "only the aged pair goes");
    assert_eq!(sweep.messages_dropped, 1, "its hollowed results message");
    assert_eq!(messages.len(), 4);
    for message in &messages {
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        let orphan = message.role == "user"
            && blocks
                .iter()
                .all(|b| matches!(b, ContentBlock::Text { .. }))
            && blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text == "Results above."));
        assert!(!orphan, "an instruction survived its results");
    }
    // The drop is a role decision, not a text-only one. The prose beside the
    // swept call is the model's own, and it stays.
    assert!(
        wire(&messages).contains("Let me read that."),
        "the model's own words went with the engine's"
    );
}

/// An assistant message that also carried prose keeps the prose, so the model's
/// own words do not vanish with the call.
#[test]
fn a_sweep_keeps_the_prose_beside_the_call() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "body", true));
    sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1)]),
        10,
        SweepSchedule::default(),
    );
    assert_eq!(messages.len(), 2, "the hollowed results message went");
    assert!(matches!(
        &messages[1].content,
        MessageContent::Blocks(blocks)
            if blocks.len() == 1 && matches!(&blocks[0], ContentBlock::Text { text } if text.contains("Let me read"))
    ));
}

/// Invariant 29. The existing loop bookkeeping assumes contiguous removal from
/// index 1, which a mid-array sweep is not.
#[test]
fn message_index_pins_survive_a_mid_array_removal() {
    let mut messages = vec![request()];
    messages.extend(pair("call-a", ADDRESS_A, "old", false));
    messages.extend(pair("call-b", ADDRESS_B, "new", false));
    // 0 request, 1 assistant(a), 2 result(a), 3 assistant(b), 4 result(b)
    assert_eq!(messages.len(), 5);

    let sweep = sweep_expired_pairs(
        &mut messages,
        &seen(&[(ADDRESS_A, 1), (ADDRESS_B, 9)]),
        10,
        SweepSchedule::default(),
    );

    assert_eq!(sweep.removed, vec![ADDRESS_A.to_string()]);
    assert_eq!(sweep.messages_dropped, 2, "the pair's two hollow messages");
    assert_eq!(sweep.remap(0), 0, "the request stays where it is");
    assert_eq!(sweep.remap(3), 1, "the newer call shifts down by two");
    assert_eq!(sweep.remap(4), 2);
    assert!(holds_address(&messages, ADDRESS_B));
}

// ---- the two text surfaces ----

fn prompt() -> String {
    system_prompt_section(ContextMode::On, SweepSchedule::default())
}

/// Invariant 6. The prompt quotes the values in force, so a swept arm cannot
/// say ten while the pass drops at four.
#[test]
fn the_prompt_quotes_the_values_in_force() {
    let swept = rendered_context_mode_prompt(3, 7);
    assert!(swept.contains("Every 7 rounds"), "{swept}");
    assert!(swept.contains("more than 3 rounds old"), "{swept}");
    assert!(!swept.contains("Every 10 rounds"));
}

/// Invariant 43's mechanism. Two arms at different values render different
/// text, so the eval's guidance hash can tell them apart.
#[test]
fn two_schedules_render_different_prompts() {
    assert_ne!(
        rendered_context_mode_prompt(5, 10),
        rendered_context_mode_prompt(4, 10)
    );
    assert_ne!(
        rendered_context_mode_prompt(5, 10),
        rendered_context_mode_prompt(5, 8)
    );
}

/// Invariant 19. It appeared five times per request plus once per placeholder,
/// and the request was advertising its most expensive move on every line.
#[test]
fn the_recovery_command_appears_once() {
    assert_eq!(prompt().matches(RESULT_RECOVERY).count(), 1);
}

/// Invariant 23. The record's text says writing is free and never says how
/// often, which leaves the old habit standing.
#[test]
fn the_guidance_states_the_write_rhythm() {
    let prompt = prompt();
    assert_eq!(prompt.matches("WHEN TO WRITE").count(), 1);
    assert!(prompt.contains("Add to it on an ordinary round"));
    assert!(prompt.contains("Rewrite it whole on the round the panel says the sweep is\nnext"));
}

/// Invariant 22. Five claims in the shipped text are false under the design,
/// and the load-bearing one is that a note is cheaper than a keep.
#[test]
fn none_of_the_five_false_claims_survives() {
    let prompt = prompt().to_ascii_lowercase();
    for claim in [
        "exactly once",
        "one more round",
        "cheaper",
        "goes stale",
        "each write is the whole document",
        "scratchpad",
        "keep_in_context",
    ] {
        assert!(!prompt.contains(claim), "the prompt still says `{claim}`");
    }
}

/// The three markers, and the three headings, all spelled out where the model
/// reads them.
#[test]
fn the_guidance_names_every_marker_it_asks_for() {
    let prompt = prompt();
    for marker in [
        "[WORKING UNDERSTANDING]",
        "[WORKING UNDERSTANDING: ADD]",
        "[/WORKING UNDERSTANDING]",
        "[CONSTRAINTS]",
        "[TODO]",
        "[KEEP OPEN]",
    ] {
        assert!(prompt.contains(marker), "the prompt never names {marker}");
    }
}

/// The keep is a line in the document, not a tool. A schema is prose billed on
/// every request of every workspace.
#[test]
fn the_guidance_never_offers_a_keep_tool() {
    let prompt = prompt();
    assert!(!prompt.contains("keep_open("));
    assert!(prompt.contains("[KEEP OPEN]"));
}

/// Invariant 17, on the prompt surface.
#[test]
fn no_prompt_surface_shows_a_shortened_address() {
    assert_no_short_addresses(&prompt());
}

/// The refusal sends the model somewhere the guidance already describes.
///
/// A refusal that only says no leaves it holding a list with nowhere to put
/// it. That is how a withdrawn tool turns into a wasted round.
#[test]
fn the_todo_refusal_names_the_heading_the_guidance_names() {
    assert!(TODO_TOOL_REFUSAL.contains("[TODO]"));
    assert!(TODO_TOOL_REFUSAL.contains("[WORKING UNDERSTANDING]"));
    assert!(prompt().contains("[TODO]"));
}

/// The rebuild is what a follow-up turn does, and the prompt used to deny it.
///
/// `build_resume_tool_blocks_with_skip_ids` rebuilds the last few pairs with
/// their addresses. A model told they were gone would price a read-back it did
/// not need, and would not know its ages had reset.
#[test]
fn the_prompt_does_not_deny_the_resume_rebuild() {
    let prompt = prompt();
    assert!(
        !prompt.contains("not sent back to you at all"),
        "the prompt still denies the rebuild"
    );
    assert!(prompt.contains("A NEW TURN REBUILDS YOUR LAST FEW TOOL CALLS"));
}
