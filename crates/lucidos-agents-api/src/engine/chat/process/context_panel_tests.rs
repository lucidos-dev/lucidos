use super::*;
use crate::llm::{ContentBlock, Message, MessageContent};

fn address(byte: u8) -> String {
    format!("evt-{}", format!("{byte:02x}").repeat(16))
}

fn item(byte: u8, chars: usize, age: usize) -> PanelItem {
    PanelItem {
        address: address(byte),
        label: "tool: read_file".to_string(),
        chars,
        original_chars: None,
        age_rounds: age,
        leaves_in: 3,
        stubbed: false,
    }
}

fn panel_of(body: &str) -> String {
    format!(
        "[CONTEXT PANEL]\n{body}\n{}\n[END CONTEXT PANEL]",
        "x".repeat(400)
    )
}

fn document_of(body: &str) -> String {
    format!(
        "{}\n{body}\n{}\n{}",
        working_understanding::OPEN_REPLACE,
        "y".repeat(400),
        working_understanding::CLOSE
    )
}

fn fixed() -> FixedRegions {
    FixedRegions {
        system_chars: 60_000,
        tool_defs_chars: 80_000,
    }
}

static NOTHING_HELD: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);

fn user(text: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Text(text.to_string()),
    }
}

fn assistant(text: &str) -> Message {
    Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::Text {
            text: text.to_string(),
        }]),
    }
}

fn wire(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|m| serde_json::to_string(m).expect("a message serializes"))
        .collect()
}

fn tool_pair(id: &str, name: &str, result: String) -> Vec<Message> {
    vec![
        Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
                thought_signature: None,
            }]),
        },
        Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: result,
            }]),
        },
    ]
}

fn one_result() -> Vec<Message> {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "read_file",
        format!("body\n[{}]", address(1)),
    ));
    messages
}

fn view<'a>(items: &'a [PanelItem], held_open: &'a HashSet<String>) -> PanelView<'a> {
    PanelView {
        items,
        fixed: fixed(),
        held_open,
        budget_chars: 500_000,
        round: 1,
        schedule: SweepSchedule::default(),
    }
}

fn panel(items: &[PanelItem], other: usize) -> String {
    view(items, &NOTHING_HELD).render(other)
}

#[test]
fn the_stated_total_counts_the_panel_itself() {
    let items = vec![item(1, 4_000, 0)];
    let other = 100_000;
    let rendered = panel(&items, other);
    let expected =
        crate::engine::context::estimate_tokens_from_chars(other + rendered.chars().count());
    assert!(
        rendered.contains(&format!("You are holding {}", field(expected))),
        "the panel must state the whole request, panel included:\n{rendered}"
    );
}

#[test]
fn both_render_passes_are_the_same_length() {
    let mut stubbed = item(2, 700, 12);
    stubbed.stubbed = true;
    stubbed.original_chars = Some(9_000);
    let items = vec![item(1, 900_000, 0), stubbed];
    let empty = view(&items, &NOTHING_HELD).render_at(0);
    let full = view(&items, &NOTHING_HELD).render_at(987_654_321);
    assert_eq!(empty.chars().count(), full.chars().count());
}

#[test]
fn every_row_states_its_own_size_age_and_remainder() {
    let items = vec![item(1, 40_000, 0), item(2, 2_500, 7)];
    let rendered = panel(&items, 1_000);
    for entry in &items {
        let tokens = crate::engine::context::estimate_tokens_from_chars(entry.chars);
        let row = rendered
            .lines()
            .find(|line| line.contains(&entry.address))
            .unwrap_or_else(|| panic!("no row for {}:\n{rendered}", entry.address));
        assert!(row.contains(&grouped(tokens)), "size missing from {row}");
    }
    assert!(rendered.contains("    7 "), "age 7 missing:\n{rendered}");
}

#[test]
fn the_panel_states_the_sweep_countdown_once() {
    let rendered = panel(&[item(1, 4_000, 0)], 1_000);
    assert_eq!(rendered.matches("The next sweep is in").count(), 1);
    assert!(
        rendered.contains("The next sweep is in 9 round(s)"),
        "{rendered}"
    );
    assert!(rendered.contains("more than 5 rounds old"), "{rendered}");
}

#[test]
fn rows_sharing_a_sweep_report_the_same_remainder() {
    let schedule = SweepSchedule::default();
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "read_file",
        format!("a\n[{}]", address(1)),
    ));
    messages.extend(tool_pair("call-2", "bash", format!("b\n[{}]", address(2))));
    let first_seen: HashMap<String, usize> =
        [(address(1), 1), (address(2), 3)].into_iter().collect();
    let items = tool_result_items(&messages, &first_seen, 8, schedule);
    assert_ne!(items[0].age_rounds, items[1].age_rounds);
    assert_eq!(items[0].leaves_in, items[1].leaves_in);
}

#[test]
fn the_round_before_a_sweep_names_what_goes() {
    let mut leaving = item(1, 4_000, 8);
    leaving.leaves_in = 1;
    let staying = item(2, 4_000, 0);
    let items = [leaving.clone(), staying.clone()];
    let rendered = PanelView {
        round: 9,
        ..view(&items, &NOTHING_HELD)
    }
    .render(1_000);
    assert!(rendered.contains("NEXT ROUND IS A SWEEP"), "{rendered}");
    assert!(rendered.contains(&leaving.address), "{rendered}");
    assert!(
        !rendered
            .lines()
            .find(|l| l.starts_with("NEXT ROUND IS A SWEEP"))
            .expect("the warning line")
            .contains(&staying.address),
        "only what actually goes is named"
    );
}

#[test]
fn an_ordinary_round_carries_no_sweep_warning() {
    let rendered = panel(&[item(1, 4_000, 0)], 1_000);
    assert!(!rendered.contains("NEXT ROUND IS A SWEEP"), "{rendered}");
}

#[test]
fn the_panel_states_what_is_held_open() {
    let held: HashSet<String> = [address(1)].into_iter().collect();
    let items = [item(1, 40_000, 3), item(2, 4_000, 0)];
    let rendered = view(&items, &held).render(1_000);
    let line = rendered
        .lines()
        .find(|line| line.starts_with("Held open:"))
        .unwrap_or_else(|| panic!("no held-open line:\n{rendered}"));
    assert!(line.contains("1 items"), "{line}");
    assert!(line.contains("% of the room"), "{line}");
    assert_eq!(rendered.matches("Held open:").count(), 1);
}

#[test]
fn nothing_held_open_states_nothing() {
    let rendered = panel(&[item(1, 40_000, 3)], 1_000);
    assert!(!rendered.contains("Held open:"), "{rendered}");
}

#[test]
fn the_percent_is_the_char_ratio() {
    let rendered = panel(&[], 249_000);
    let line = rendered
        .lines()
        .find(|line| line.contains("%)"))
        .unwrap_or_else(|| panic!("no budget line:\n{rendered}"));
    let expected = (249_000 + rendered.chars().count()) * 100 / 500_000;
    assert!(line.contains(&format!("{expected}%)")), "{line}");
}

#[test]
fn appending_the_panel_touches_no_earlier_block() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "read_file",
        format!("a result\n[{}]", address(1)),
    ));
    let before = wire(&messages);
    append_to_tail(
        &mut messages,
        "[CONTEXT PANEL]\nx\n[END CONTEXT PANEL]".to_string(),
    );
    let after = wire(&messages);
    assert_eq!(after.len(), before.len());
    assert_eq!(after[..after.len() - 1], before[..before.len() - 1]);
}

#[test]
fn a_second_panel_does_not_disturb_the_first() {
    let mut messages = vec![user("the request")];
    append_to_tail(&mut messages, "panel one".to_string());
    let after_first = wire(&messages);
    messages.extend(tool_pair(
        "call-1",
        "bash",
        format!("out\n[{}]", address(2)),
    ));
    append_to_tail(&mut messages, "panel two".to_string());
    assert_eq!(wire(&messages)[0], after_first[0]);
}

#[test]
fn a_text_message_gains_a_block_rather_than_a_suffix() {
    let mut messages = vec![user("the request")];
    append_to_tail(&mut messages, "the panel".to_string());
    let MessageContent::Blocks(blocks) = &messages[0].content else {
        panic!("expected blocks");
    };
    assert_eq!(blocks.len(), 2);
}

#[test]
fn every_addressable_tool_result_becomes_an_item() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "read_file",
        format!("{}\n[{}]", "x".repeat(4_000), address(1)),
    ));
    messages.extend(tool_pair(
        "call-2",
        "bash",
        format!("{}\n[{}]", "y".repeat(600), address(2)),
    ));
    let items = tool_result_items(&messages, &HashMap::new(), 1, SweepSchedule::default());
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "tool: read_file");
    assert_eq!(items[1].label, "tool: bash");
}

#[test]
fn a_result_with_no_address_is_not_an_item() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair("call-1", "read_file", "no trailer".to_string()));
    assert!(tool_result_items(&messages, &HashMap::new(), 1, SweepSchedule::default()).is_empty());
}

#[test]
fn a_budget_cut_result_reads_as_already_let_go() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "bash",
        crate::engine::context::budget_stub(
            &format!("{}\n[{}]", "z".repeat(9_000), address(2)),
            crate::engine::context::RecoveryClause::State,
        ),
    ));
    let items = tool_result_items(&messages, &HashMap::new(), 1, SweepSchedule::default());
    assert!(items[0].stubbed);
    let rendered = panel(&items, 1_000);
    assert!(rendered.contains("Already let go:"), "{rendered}");
}

#[test]
fn age_counts_rounds_since_the_item_arrived() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair(
        "call-1",
        "read_file",
        format!("body\n[{}]", address(1)),
    ));
    let mut first_seen = HashMap::new();
    let round_one = tool_result_items(&messages, &first_seen, 1, SweepSchedule::default());
    note_first_seen(&mut first_seen, &messages, 1);
    assert_eq!(round_one[0].age_rounds, 0);
    let round_five = tool_result_items(&messages, &first_seen, 5, SweepSchedule::default());
    assert_eq!(round_five[0].age_rounds, 4);
}

#[test]
fn a_later_round_does_not_reset_a_known_age() {
    let messages = one_result();
    let mut first_seen = HashMap::new();
    note_first_seen(&mut first_seen, &messages, 2);
    note_first_seen(&mut first_seen, &messages, 9);
    assert_eq!(first_seen.get(&address(1)), Some(&2));
}

#[test]
fn a_result_with_no_address_gets_no_entry() {
    let mut messages = vec![user("the request")];
    messages.extend(tool_pair("call-1", "read_file", "no trailer".to_string()));
    let mut first_seen = HashMap::new();
    note_first_seen(&mut first_seen, &messages, 1);
    assert!(first_seen.is_empty());
}

#[test]
fn a_keep_overwrites_the_first_seen_round() {
    let mut first_seen = HashMap::new();
    note_first_seen(&mut first_seen, &one_result(), 2);
    hold_open(&mut first_seen, &address(1), 9);
    assert_eq!(first_seen.get(&address(1)), Some(&9));
}

#[test]
fn elided_rows_are_counted_out_loud() {
    let items: Vec<PanelItem> = (0..30).map(|i| item(i as u8, 100, 0)).collect();
    let rendered = panel(&items, 1_000);
    assert!(
        rendered.contains(&format!("30 items under {ROW_MIN_CHARS} chars")),
        "{rendered}"
    );
}

#[test]
fn the_biggest_items_get_the_rows() {
    let mut items: Vec<PanelItem> = (0..MAX_ROWS + 5)
        .map(|i| item(i as u8, 1_000 + i * 10, 0))
        .collect();
    items.push(item(200, 900_000, 0));
    let rendered = panel(&items, 1_000);
    assert!(
        rendered.contains(&address(200)),
        "the largest item is missing"
    );
    assert!(rendered.contains(&address(24)), "{rendered}");
    assert!(
        !rendered.contains(&address(0)),
        "smallest rows must be elided"
    );
}

#[test]
fn the_panel_names_the_half_no_keep_reaches() {
    let rendered = panel(&[], 1_000);
    assert!(rendered.contains("system instructions"), "{rendered}");
    assert!(rendered.contains("tool definitions"), "{rendered}");
}

#[test]
fn an_empty_panel_still_states_the_budget() {
    let rendered = panel(&[], 1_000);
    assert!(rendered.contains("Nothing addressable is in the prompt yet"));
    assert!(rendered.contains("You are holding"));
}

#[test]
fn the_capture_row_reports_the_panel_size() {
    let rendered = panel(&[item(1, 4_000, 0)], 1_000);
    let section = panel_section(&rendered);
    assert_eq!(section.name, PANEL_SECTION);
    assert_eq!(section.budget_delta_chars, rendered.chars().count());
    assert_eq!(
        section.content_chars,
        Some(rendered.chars().count()),
        "nothing else counts the panel, so the two sizes agree"
    );
}

#[test]
fn thousands_are_grouped_from_the_right() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(1_234_567), "1,234,567");
}

#[test]
fn the_panel_never_repeats_the_recovery_command() {
    let rendered = panel(&[item(1, 4_000, 0)], 1_000);
    assert!(
        !rendered.contains(crate::engine::chat::process::context_mode::RESULT_RECOVERY),
        "{rendered}"
    );
}

#[test]
fn the_panel_quotes_both_numbers() {
    let rendered = PanelView {
        schedule: SweepSchedule::new(3, 7),
        ..view(&[], &NOTHING_HELD)
    }
    .render(1_000);
    assert!(rendered.contains("more than 3 rounds old"), "{rendered}");
    assert!(rendered.contains("One runs every 7 rounds"), "{rendered}");
}

#[test]
fn the_footer_names_the_keep_and_nothing_more() {
    let rendered = panel(&[], 1_000);
    assert!(rendered.contains("[KEEP OPEN]"), "{rendered}");
    assert!(rendered.contains("leaves-in column"), "{rendered}");
    assert!(
        !rendered.contains("exempt"),
        "the wall's rule is in the standing instructions:\n{rendered}"
    );
}

#[test]
fn no_panel_surface_shows_a_shortened_address() {
    crate::engine::chat::process::context_mode::assert_no_short_addresses(&panel(
        &[item(1, 4_000, 0)],
        1_000,
    ));
}

#[test]
fn the_round_collapses_what_it_is_about_to_replace() {
    let mut messages = vec![user("the request")];
    append_to_tail(&mut messages, document_of("older notes"));
    append_to_tail(&mut messages, panel_of("round one"));
    messages.extend(tool_pair(
        "call-1",
        "bash",
        format!("out\n[{}]", address(2)),
    ));

    assert_eq!(collapse_tail_blocks(&mut messages, false), 2);
    append_to_tail(&mut messages, document_of("current notes"));
    append_to_tail(&mut messages, panel_of("round two"));

    let rendered = wire(&messages).join("\n");
    assert!(
        !rendered.contains("older notes"),
        "the old document must go"
    );
    assert!(!rendered.contains("round one"), "the old panel must go");
    assert!(rendered.contains("current notes"), "the live one must stay");
    assert!(rendered.contains("round two"), "the live panel must stay");
    assert!(rendered.contains("a context panel from an earlier round"));
    assert!(rendered.contains("an earlier rendering of your working understanding"));
}

#[test]
fn ten_rounds_leave_one_live_panel_and_one_live_document() {
    let mut messages = vec![user("the request")];
    for round in 0..10 {
        collapse_tail_blocks(&mut messages, false);
        append_to_tail(&mut messages, document_of(&format!("notes {round}")));
        append_to_tail(&mut messages, panel_of(&format!("panel {round}")));
        messages.extend(tool_pair(
            &format!("call-{round}"),
            "bash",
            format!("out\n[{}]", address(round as u8)),
        ));
    }
    let rendered = wire(&messages).join("\n");
    assert_eq!(rendered.matches("[CONTEXT PANEL]").count(), 1);
    assert_eq!(
        rendered
            .matches(working_understanding::OPEN_REPLACE)
            .count(),
        1
    );
    assert!(rendered.contains("panel 9") && rendered.contains("notes 9"));
}

#[test]
fn a_user_message_opening_with_the_panel_heading_survives() {
    let mut messages = vec![user(&panel_of("what does this row mean?"))];
    let before = wire(&messages);

    assert_eq!(
        collapse_tail_blocks(&mut messages, true),
        0,
        "the user wrote this block, so nothing here is superseded"
    );
    assert_eq!(wire(&messages), before);
}

#[test]
fn collapsing_twice_changes_nothing_the_second_time() {
    let mut messages = vec![user("the request")];
    append_to_tail(&mut messages, panel_of("round one"));
    assert_eq!(collapse_tail_blocks(&mut messages, false), 1);
    assert_eq!(collapse_tail_blocks(&mut messages, false), 0);
}

#[test]
fn assistant_spans_fold_only_on_a_sweep_round() {
    let reply = format!(
        "Reading it now.\n{}\nprivate notes\n{}\n{}\nBack shortly.",
        working_understanding::OPEN_REPLACE,
        "and a good deal more of them. ".repeat(20),
        working_understanding::CLOSE
    );
    let mut ordinary = vec![user("the request"), assistant(&reply)];
    let before = wire(&ordinary);
    assert_eq!(collapse_tail_blocks(&mut ordinary, false), 0);
    assert_eq!(wire(&ordinary), before, "an ordinary round touches nothing");

    let mut sweeping = vec![user("the request"), assistant(&reply)];
    assert_eq!(collapse_tail_blocks(&mut sweeping, true), 1);
    let folded = wire(&sweeping).join("\n");
    assert!(!folded.contains("private notes"));
    assert!(folded.contains("Reading it now."));
    assert!(folded.contains("Back shortly."));
}

#[test]
fn an_engine_block_is_overwritten_and_a_reply_is_spliced() {
    let span = format!(
        "prose\n{}\nnotes\n{}\n{}\nmore prose",
        working_understanding::OPEN_REPLACE,
        "and a good deal more of them. ".repeat(20),
        working_understanding::CLOSE
    );
    let mut messages = vec![user("the request"), assistant(&span), user("results")];
    append_to_tail(&mut messages, document_of("engine rendering"));

    collapse_tail_blocks(&mut messages, true);
    let rendered = wire(&messages).join("\n");
    assert!(rendered.contains("prose"), "the model's words survive");
    assert!(rendered.contains("more prose"));
    assert!(rendered.contains("an earlier version of your working understanding"));
    assert!(rendered.contains("an earlier rendering of your working understanding"));
    assert!(!rendered.contains("engine rendering"));
}

#[test]
fn a_row_capped_item_is_not_called_too_small() {
    let items: Vec<PanelItem> = (0..MAX_ROWS + 6).map(|i| item(i as u8, 5_000, 0)).collect();
    let rendered = panel(&items, 1_000);
    assert!(
        rendered.contains("did not fit this table"),
        "a capped item must be named as capped:\n{rendered}"
    );
    assert!(
        !rendered.contains(&format!("items under {ROW_MIN_CHARS} chars")),
        "nothing here is under the floor:\n{rendered}"
    );
}

#[test]
fn the_two_elision_reasons_are_counted_separately() {
    let mut items: Vec<PanelItem> = (0..MAX_ROWS + 3).map(|i| item(i as u8, 5_000, 0)).collect();
    items.push(item(250, 100, 0));
    let rendered = panel(&items, 1_000);
    assert!(
        rendered.contains(&format!("1 items under {ROW_MIN_CHARS} chars")),
        "{rendered}"
    );
    assert!(rendered.contains("3 more items did not fit"), "{rendered}");
}

#[test]
fn the_sweep_round_itself_carries_no_sweep_warning() {
    let mut leaving = item(1, 4_000, 8);
    leaving.leaves_in = 1;
    let items = [leaving];
    let rendered = PanelView {
        round: SweepSchedule::default().sweep_every_rounds,
        ..view(&items, &NOTHING_HELD)
    }
    .render(1_000);
    assert!(!rendered.contains("NEXT ROUND IS A SWEEP"), "{rendered}");
}

#[test]
fn the_capture_row_name_is_the_one_the_eval_reads() {
    assert_eq!(panel_section("[CONTEXT PANEL]").name, "Context Panel");
}
