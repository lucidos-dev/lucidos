use super::*;

const ADDRESS: &str = "evt-0123456789abcdef0123456789abcdef";
const OTHER_ADDRESS: &str = "evt-fedcba9876543210fedcba9876543210";

fn doc(body: &str, constraints: &str) -> WorkingUnderstanding {
    WorkingUnderstanding {
        body: body.to_string(),
        constraints: constraints.to_string(),
    }
}

fn apply(document: &WorkingUnderstanding, reply: &str) -> (WorkingUnderstanding, Applied) {
    apply_spans(document, &parse_reply(reply))
}

#[test]
fn the_marker_prefix_opens_both_forms() {
    assert!(OPEN_REPLACE.starts_with(MARKER_PREFIX));
    assert!(OPEN_ADD.starts_with(MARKER_PREFIX));
    assert!(
        !CLOSE.starts_with(MARKER_PREFIX),
        "the close is not an open"
    );
}

#[test]
fn an_unclosed_marker_captures_to_the_end_of_the_reply() {
    let parsed = parse_reply("before\n[WORKING UNDERSTANDING]\nthe body\nand more");
    assert_eq!(parsed.spans.len(), 1);
    assert_eq!(parsed.spans[0].body, "the body\nand more");
    assert!(
        parsed.faults.iter().any(|f| f.contains("ran to the end")),
        "the engine says when it closed a span: {:?}",
        parsed.faults
    );
}

#[test]
fn silence_keeps_the_document() {
    let before = doc("what I know", "do not touch the frontend");
    let (after, applied) = apply(&before, "Just a sentence for the user. Nothing marked.");
    assert_eq!(after, before);
    assert!(!parse_reply("nothing marked here").wrote_something());
    assert!(applied.todo.is_none());
}

#[test]
fn a_replace_then_an_add_apply_in_that_order() {
    let (after, _) = apply(
        &doc("stale", ""),
        "[WORKING UNDERSTANDING]\nfresh\n[/WORKING UNDERSTANDING]\n\
         mid-reply prose\n\
         [WORKING UNDERSTANDING: ADD]\nand one more line\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(after.body, "fresh\nand one more line");
}

#[test]
fn an_add_then_a_replace_apply_in_that_order() {
    let (after, _) = apply(
        &doc("stale", ""),
        "[WORKING UNDERSTANDING: ADD]\nextra\n[/WORKING UNDERSTANDING]\n\
         [WORKING UNDERSTANDING]\nthe whole thing\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(after.body, "the whole thing");
}

#[test]
fn the_engines_own_rendering_is_never_read_as_a_write() {
    let rendered = render(
        &doc("what I know", "call it X"),
        &[],
        &RoundNotices::default(),
    );
    assert!(rendered.starts_with(OPEN_REPLACE), "{rendered}");
    assert!(
        parse_message("user", &rendered).spans.is_empty(),
        "a user-role block is the engine's own rendering"
    );
    assert!(
        parse_message(ASSISTANT_ROLE, &rendered).wrote_something(),
        "the same text from the model IS a write"
    );
}

#[test]
fn a_stop_cut_fragment_cannot_rewrite_the_document() {
    let before = doc("weeks of notes", "call it X");
    let reply = "Working on it.\n[WORKING UNDERSTANDING]\nthree words";
    let mut parsed = parse_reply(reply);
    assert!(!parsed.spans[0].closed);

    parsed.drop_unclosed();
    let (after, _) = apply_spans(&before, &parsed);
    assert_eq!(after, before);
    assert_eq!(
        strip_faulted_markup(&splice_spans_out(reply, &parsed.spans)),
        "Working on it."
    );

    let mut mixed = parse_reply(
        "[WORKING UNDERSTANDING]\nfinished\n[/WORKING UNDERSTANDING]\n\
         [WORKING UNDERSTANDING: ADD]\ncut off",
    );
    mixed.drop_unclosed();
    assert_eq!(mixed.spans.len(), 1);
    assert_eq!(mixed.spans[0].body, "finished");
}

#[test]
fn folding_a_superseded_span_keeps_everything_around_it() {
    let reply = format!(
        "Here is what I found.\n{OPEN_REPLACE}\nprivate notes\n{}\n{CLOSE}\nNow running the tests.",
        "and a good deal more of them. ".repeat(20)
    );
    let folded = fold_spans(&reply).expect("a span folds");
    assert!(folded.contains("Here is what I found."));
    assert!(folded.contains("Now running the tests."));
    assert!(folded.contains(SUPERSEDED_SPAN));
    assert!(!folded.contains("private notes"));
}

#[test]
fn a_reply_with_no_span_does_not_fold() {
    assert!(fold_spans("ordinary prose, no markers").is_none());
}

#[test]
fn a_span_smaller_than_its_own_marker_is_left_alone() {
    let reply = format!("{OPEN_REPLACE}\nshort\n{CLOSE}");
    assert!(reply.len() < SUPERSEDED_SPAN.len());
    assert!(fold_spans(&reply).is_none());
}

#[test]
fn splicing_leaves_only_what_was_addressed_to_the_user() {
    let reply = "Reading the file now.\n\
                 [WORKING UNDERSTANDING: ADD]\nthe file is 400 lines\n[/WORKING UNDERSTANDING]\n\
                 I will report back.";
    let parsed = parse_reply(reply);
    let visible = splice_spans_out(reply, &parsed.spans);
    assert_eq!(visible, "Reading the file now.\n\nI will report back.");
    assert!(!visible.contains(MARKER_PREFIX));
}

#[test]
fn a_reply_that_is_only_the_document_splices_to_nothing() {
    let reply = "[WORKING UNDERSTANDING]\nnotes only\n[/WORKING UNDERSTANDING]";
    let parsed = parse_reply(reply);
    assert!(splice_spans_out(reply, &parsed.spans).is_empty());
}

#[test]
fn a_malformed_opening_marker_does_not_reach_the_user() {
    let reply = "Here is the plan.\n\
                 [WORKING UNDERSTANDING: REPLACE]\nthe body\n[/WORKING UNDERSTANDING]\n\
                 Starting now.";
    let parsed = parse_reply(reply);
    assert!(
        parsed.spans.is_empty(),
        "an unreadable opening writes nothing"
    );
    assert!(!parsed.faults.is_empty(), "and it is reported");
    let visible = strip_faulted_markup(&splice_spans_out(reply, &parsed.spans));
    assert_eq!(visible, "Here is the plan.\n\nStarting now.");
}

#[test]
fn a_malformed_opening_with_no_close_takes_the_rest() {
    let visible = strip_faulted_markup("Working on it.\n[WORKING UNDERSTANDING: NEW]\nthe body");
    assert_eq!(visible, "Working on it.");
}

#[test]
fn stripping_leaves_text_that_carries_no_markup_alone() {
    let text = "Nothing here mentions the block at all.";
    assert_eq!(strip_faulted_markup(text), text);
}

#[test]
fn an_add_accumulates_the_constraints() {
    let (after, _) = apply(
        &doc("body", "do not touch the frontend"),
        "[WORKING UNDERSTANDING: ADD]\nmore body\n[CONSTRAINTS]\ncall it X, not Y\n\
         [/WORKING UNDERSTANDING]",
    );
    assert_eq!(after.body, "body\nmore body");
    assert_eq!(
        after.constraints,
        "do not touch the frontend\ncall it X, not Y"
    );
}

#[test]
fn an_add_replaces_the_checklist() {
    let (_, applied) = apply(
        &doc("body", ""),
        "[WORKING UNDERSTANDING: ADD]\nprogress\n[TODO]\n- [x] read the file\n\
         [/WORKING UNDERSTANDING]",
    );
    let items = applied.todo.expect("the span carried a checklist");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, TodoStatus::Completed);
    assert_eq!(items[0].content, "read the file");
}

#[test]
fn a_span_with_no_checklist_writes_none() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\njust the body\n[/WORKING UNDERSTANDING]",
    );
    assert!(applied.todo.is_none());
}

#[test]
fn a_replace_with_no_body_keeps_the_document() {
    let before = doc("twenty rounds of notes", "do not touch the frontend");
    let (after, applied) = apply(
        &before,
        &format!("[WORKING UNDERSTANDING]\n[KEEP OPEN]\n{ADDRESS}\n[/WORKING UNDERSTANDING]"),
    );
    assert_eq!(after, before, "a bare keep block is not a rewrite");
    assert_eq!(applied.keep_open, vec![ADDRESS.to_string()]);
}

#[test]
fn a_bodyless_replace_replaces_the_constraints_it_carries() {
    let before = doc("", "do not touch the frontend");
    let span = "[WORKING UNDERSTANDING]\n[CONSTRAINTS]\ndo not touch the frontend\n\
                [/WORKING UNDERSTANDING]";
    let (once, _) = apply(&before, span);
    let (twice, _) = apply(&once, span);
    assert_eq!(twice.constraints, "do not touch the frontend");
    assert_eq!(twice.body, "", "the body is still not its business");
}

#[test]
fn a_replace_carrying_only_a_checklist_keeps_the_document() {
    let before = doc("twenty rounds of notes", "call it X");
    let (after, applied) = apply(
        &before,
        "[WORKING UNDERSTANDING]\n[TODO]\n- [x] ship it\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(after, before);
    assert_eq!(applied.todo.as_ref().map(Vec::len), Some(1));
}

#[test]
fn a_replace_that_omits_the_constraints_keeps_them() {
    let (after, _) = apply(
        &doc("old body", "old constraint"),
        "[WORKING UNDERSTANDING]\nnew body\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(after.body, "new body");
    assert_eq!(after.constraints, "old constraint");
}

#[test]
fn an_empty_constraints_heading_clears_them() {
    let (after, _) = apply(
        &doc("old body", "old constraint"),
        "[WORKING UNDERSTANDING]\nnew body\n[CONSTRAINTS]\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(after.body, "new body");
    assert_eq!(after.constraints, "");
}

#[test]
fn the_checklist_and_the_keeps_are_never_stored() {
    let (after, applied) = apply(
        &doc("", ""),
        &format!(
            "[WORKING UNDERSTANDING]\nthe body\n[TODO]\n- [>] work\n[KEEP OPEN]\n{ADDRESS}\n\
             [/WORKING UNDERSTANDING]"
        ),
    );
    let stored = after.to_document();
    assert_eq!(stored, "the body");
    assert!(!stored.contains("[TODO]"));
    assert!(!stored.contains("[KEEP OPEN]"));
    assert!(applied.todo.is_some());
    assert_eq!(applied.keep_open, vec![ADDRESS.to_string()]);
}

#[test]
fn replaying_the_stored_document_asserts_no_keep() {
    let (after, _) = apply(
        &doc("", ""),
        &format!("[WORKING UNDERSTANDING]\nbody\n[KEEP OPEN]\n{ADDRESS}\n[/WORKING UNDERSTANDING]"),
    );
    let replay = format!(
        "[WORKING UNDERSTANDING]\n{}\n[/WORKING UNDERSTANDING]",
        after.to_document()
    );
    let (_, again) = apply(&after, &replay);
    assert!(again.keep_open.is_empty());
}

#[test]
fn the_document_round_trips_through_its_stored_form() {
    let before = doc("line one\nline two", "call it X");
    assert_eq!(
        WorkingUnderstanding::from_document(&before.to_document()),
        before
    );
}

#[test]
fn prose_about_the_headings_is_not_a_heading() {
    let (after, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\n\
         The todo list and the constraints both live in this block.\n\
         I should keep open the big read.\n[/WORKING UNDERSTANDING]",
    );
    assert!(applied.todo.is_none());
    assert_eq!(after.constraints, "");
    assert!(after.body.contains("The todo list and the constraints"));
}

#[test]
fn a_bare_heading_is_named_rather_than_read() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\nbody\nTODO\n- [ ] a thing\n[/WORKING UNDERSTANDING]",
    );
    assert!(applied.todo.is_none(), "a bare word opens no section");
    assert!(
        applied.faults.iter().any(|f| f.contains("[TODO]")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn a_decorated_heading_still_counts() {
    for heading in ["## [TODO]", "**[TODO]**", "### [TODO]:"] {
        let (_, applied) = apply(
            &doc("", ""),
            &format!(
                "[WORKING UNDERSTANDING]\nbody\n{heading}\n- [ ] a thing\n[/WORKING UNDERSTANDING]"
            ),
        );
        assert!(applied.todo.is_some(), "{heading} should open the section");
    }
}

#[test]
fn every_checklist_mark_round_trips() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\n[TODO]\n\
         - [ ] pending\n- [>] running\n- [x] done\n- [waiting] parked\n- [abandoned] dropped\n\
         [/WORKING UNDERSTANDING]",
    );
    let items = applied.todo.expect("a checklist");
    let statuses: Vec<TodoStatus> = items.iter().map(|i| i.status).collect();
    assert_eq!(
        statuses,
        vec![
            TodoStatus::Pending,
            TodoStatus::InProgress,
            TodoStatus::Completed,
            TodoStatus::Waiting,
            TodoStatus::Abandoned,
        ]
    );
}

#[test]
fn an_unreadable_checklist_line_is_named() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\n[TODO]\n- [?] nonsense\n- [ ] fine\n[/WORKING UNDERSTANDING]",
    );
    assert_eq!(applied.todo.as_ref().map(Vec::len), Some(1));
    assert!(
        applied.faults.iter().any(|f| f.contains("could not read")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn a_second_item_in_progress_is_named_and_demoted() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\n[TODO]\n- [>] one\n- [>] two\n[/WORKING UNDERSTANDING]",
    );
    let items = applied.todo.as_ref().expect("a checklist");
    assert_eq!(items.len(), 2, "nothing is dropped");
    assert_eq!(items[0].status, TodoStatus::InProgress);
    assert_eq!(items[1].status, TodoStatus::Pending);
    assert!(
        applied.faults.iter().any(|f| f.contains("in progress")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn a_checklist_past_the_cap_is_named() {
    let lines: String = (0..=MAX_TODO_ITEMS)
        .map(|i| format!("- [ ] item {i}\n"))
        .collect();
    let (_, applied) = apply(
        &doc("", ""),
        &format!("[WORKING UNDERSTANDING]\n[TODO]\n{lines}[/WORKING UNDERSTANDING]"),
    );
    assert_eq!(applied.todo.as_ref().map(Vec::len), Some(MAX_TODO_ITEMS));
    assert!(
        applied.faults.iter().any(|f| f.contains("cap is")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn an_address_that_is_not_an_address_is_reported() {
    let (_, applied) = apply(
        &doc("", ""),
        "[WORKING UNDERSTANDING]\n[KEEP OPEN]\nevt-7f3a\n[/WORKING UNDERSTANDING]",
    );
    assert!(applied.keep_open.is_empty());
    assert!(
        applied.faults.iter().any(|f| f.contains("not an address")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn a_trailing_duration_is_ignored_and_reported() {
    let (_, applied) = apply(
        &doc("", ""),
        &format!(
            "[WORKING UNDERSTANDING]\n[KEEP OPEN]\n{ADDRESS} for 20 rounds\n\
             [/WORKING UNDERSTANDING]"
        ),
    );
    assert_eq!(applied.keep_open, vec![ADDRESS.to_string()]);
    assert!(
        applied.faults.iter().any(|f| f.contains("no duration")),
        "{:?}",
        applied.faults
    );
}

#[test]
fn keeps_accumulate_across_spans_and_deduplicate() {
    let (_, applied) = apply(
        &doc("", ""),
        &format!(
            "[WORKING UNDERSTANDING: ADD]\n[KEEP OPEN]\n- {ADDRESS}\n[/WORKING UNDERSTANDING]\n\
             [WORKING UNDERSTANDING: ADD]\n[KEEP OPEN]\n{OTHER_ADDRESS}\n{ADDRESS}\n\
             [/WORKING UNDERSTANDING]"
        ),
    );
    assert_eq!(
        applied.keep_open,
        vec![ADDRESS.to_string(), OTHER_ADDRESS.to_string()]
    );
}

#[test]
fn a_decorated_address_is_still_an_address() {
    for written in [
        format!("`{ADDRESS}`"),
        format!("- `{ADDRESS}`"),
        format!("**{ADDRESS}**"),
        format!("[{ADDRESS}]"),
        format!("- {ADDRESS},"),
    ] {
        let (_, applied) = apply(
            &doc("", ""),
            &format!(
                "[WORKING UNDERSTANDING: ADD]\n[KEEP OPEN]\n{written}\n[/WORKING UNDERSTANDING]"
            ),
        );
        assert_eq!(
            applied.keep_open,
            vec![ADDRESS.to_string()],
            "`{written}` was not read as a keep"
        );
        assert!(applied.faults.is_empty(), "`{written}` raised a fault");
    }
}

#[test]
fn nothing_is_refused_on_size() {
    let long = "x".repeat(ASK_TO_REWRITE_ABOVE_CHARS + 1);
    let block = render(&doc(&long, ""), &[], &RoundNotices::default());
    assert!(block.contains("Rewrite it whole"), "{}", &block[..200]);
    assert!(block.contains(&long), "the document survives intact");
    assert!(!block.contains("Error"));
}

#[test]
fn the_constraints_heading_always_renders() {
    let block = render(
        &WorkingUnderstanding::default(),
        &[],
        &RoundNotices::default(),
    );
    assert!(block.contains("[CONSTRAINTS]"));
    assert!(block.contains("Nothing stated yet."));
    assert!(block.starts_with(OPEN_REPLACE));
    assert!(block.ends_with(CLOSE));
}

#[test]
fn the_render_carries_the_engine_written_statuses() {
    let items = vec![
        TodoItem {
            content: "parked on an event".to_string(),
            active_form: "parked on an event".to_string(),
            status: TodoStatus::Waiting,
        },
        TodoItem {
            content: "walked away".to_string(),
            active_form: "walked away".to_string(),
            status: TodoStatus::Abandoned,
        },
    ];
    let block = render(&doc("body", ""), &items, &RoundNotices::default());
    assert!(block.contains("- [waiting] parked on an event"));
    assert!(block.contains("- [abandoned] walked away"));
}

#[test]
fn the_framing_names_the_faults_and_the_holds() {
    let notices = RoundNotices {
        faults: vec!["a checklist line I could not read".to_string()],
        held_open: vec![ADDRESS.to_string()],
    };
    let block = render(&doc("body", ""), &[], &notices);
    assert_eq!(block.matches("Could not read:").count(), 1);
    assert!(block.contains(&format!("Held open this round: {ADDRESS}")));
}

#[test]
fn no_surface_shows_a_shortened_address() {
    let notices = RoundNotices {
        faults: vec![format!("`{OTHER_ADDRESS}` is not an address I could read.")],
        held_open: vec![ADDRESS.to_string()],
    };
    let block = render(&doc(&format!("read {ADDRESS}"), ""), &[], &notices);
    assert!(block.contains(ADDRESS) && block.contains(OTHER_ADDRESS));
    super::super::context_mode::assert_no_short_addresses(&block);
}
