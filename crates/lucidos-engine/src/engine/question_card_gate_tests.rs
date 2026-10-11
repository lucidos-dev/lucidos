use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

fn since(last_input: LastInput) -> SinceLastInput {
    SinceLastInput {
        last_input,
        unreported_tool_calls: 0,
        refused: false,
        artifact_refused: false,
        words: String::new(),
        saved_artifacts: Vec::new(),
        card_has_message: false,
        last_progress_note: None,
    }
}

/// The reply the user typed into a card in the incident.
const TYPED_REPLY: &str = "I need site publisher to be readable on mobike";

fn typed() -> LastInput {
    LastInput::Typed(TYPED_REPLY.to_string())
}

fn message() -> LastInput {
    LastInput::Message("what is in the next release?".to_string())
}

/// The refusal for owing words when there is no input text to quote.
fn card_refusal() -> String {
    Refusal::OwesWords {
        input: LastInput::None,
    }
    .text()
}

/// A card that points "above".
const POINTS_ABOVE: &str = "Go with the copy above?";

fn points_above(questions: &serde_json::Value) -> bool {
    says_above(&card_text(questions))
}

fn owes_words(s: SinceLastInput) -> bool {
    matches!(should_refuse(&s, ""), Some(Refusal::OwesWords { .. }))
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

/// A typed answer may answer the card, ask back or say something else. So
/// the agent may ask its question again at once, as after any message.
#[test]
fn a_re_ask_right_after_a_typed_answer_passes() {
    assert_eq!(should_refuse(&since(typed()), ""), None);
}

#[test]
fn a_card_after_unreported_tool_work_is_refused() {
    // The chat case: asked what the next release holds, the agent ran git log
    // and raised "How do you want to continue?".
    for input in [message(), LastInput::Picked, LastInput::None] {
        assert!(owes_words(SinceLastInput {
            unreported_tool_calls: 2,
            ..since(input)
        }));
    }
}

#[test]
fn a_clarifying_question_with_no_work_passes() {
    assert!(!owes_words(since(message())));
}

#[test]
fn a_pick_followed_at_once_by_the_next_card_passes() {
    assert!(!owes_words(since(LastInput::Picked)));
}

#[test]
fn a_preamble_does_not_report_the_work_after_it() {
    // "Let me check the log", then git log, then a card: the log went unreported.
    assert!(owes_words(SinceLastInput {
        words: "Let me check the log.".to_string(),
        unreported_tool_calls: 1,
        ..since(message())
    }));
}

#[test]
fn one_refusal_per_input() {
    assert!(!owes_words(SinceLastInput {
        refused: true,
        unreported_tool_calls: 3,
        ..since(typed())
    }));
}

#[test]
fn a_typed_answer_carries_text() {
    let typed = |a: serde_json::Value| LastInput::from_answer(&a);
    assert_eq!(
        typed(json!({"kind": "FreeText", "text": "is there a difference?"})),
        LastInput::Typed("is there a difference?".to_string())
    );
    assert_eq!(
        typed(json!({"kind": "MultiSelected", "option_ids": ["opt-0"], "text": "and X"})),
        LastInput::Typed("and X".to_string())
    );
    assert_eq!(
        typed(json!({"kind": "MultiSelected", "option_ids": ["opt-0"]})),
        LastInput::Picked
    );
    assert_eq!(
        typed(json!({"kind": "Selected", "option_id": "opt-0"})),
        LastInput::Picked
    );
    assert_eq!(
        typed(json!({"kind": "FreeText", "text": "  "})),
        LastInput::Picked
    );
}

#[test]
fn the_refusal_starts_with_the_marker_and_offers_the_escape() {
    assert!(card_refusal().starts_with(REFUSAL_MARKER));
    assert!(card_refusal().contains("never your tool results"));
    assert!(card_refusal().contains("unchanged"));
    assert!(!card_refusal().contains('\u{2014}'));
}

/// Every agent's notes arrive as text, the coding agents' through the Vertex
/// relay. So the refusal asks for prose and never blames hidden reasoning.
#[test]
fn the_refusal_asks_for_prose() {
    assert!(card_refusal().contains("plain prose"));
    assert!(!card_refusal().contains("hidden reasoning"));
}

// ---------------------------------------------------------------------------
// A card that points "above" at nothing
// ---------------------------------------------------------------------------

/// The note Claude Code showed in the incident, 224 characters. The copy the
/// card pointed at stayed in the agent's reasoning.
const INCIDENT_NOTE: &str = "I'll rework the card options into user-facing language: \"Keep it \
     plain,\" \"Technical,\" and \"I write software,\" keeping the precise agent-facing \
     instructions separate. The structural question still needs to be resolved next.";

fn incident_card() -> serde_json::Value {
    json!([{
        "question": "Go with three levels and the card copy above?",
        "header": "Levels",
        "multiSelect": false,
        "options": [
            {"label": "Three levels, this copy (Recommended)",
             "description": "Keep it plain / Technical / I write software, with the user-facing descriptions above pinned so the agent can't improvise."},
            {"label": "Four levels, this style of copy",
             "description": "Keep a second level, reworded around a real dividing line."}
        ]
    }])
}

fn after_words(words: &str) -> SinceLastInput {
    SinceLastInput {
        words: words.to_string(),
        ..since(typed())
    }
}

#[test]
fn a_card_pointing_above_at_a_note_is_refused() {
    assert!(points_above(&incident_card()));
    assert_eq!(
        should_refuse(&after_words(INCIDENT_NOTE), POINTS_ABOVE),
        Some(Refusal::PointsAboveAtNothing {
            words: INCIDENT_NOTE.to_string()
        })
    );
}

#[test]
fn a_card_pointing_above_after_nothing_at_all_is_refused() {
    // The chat case: "You use the message above", with no words since the
    // user's message. A picked answer owes no words, so only this rule fires.
    let s = SinceLastInput {
        words: String::new(),
        ..since(LastInput::Picked)
    };
    assert_eq!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::PointsAboveAtNothing {
            words: String::new()
        })
    );
}

#[test]
fn a_card_pointing_above_at_a_real_reply_passes() {
    let reply = "A real reply with the drafted copy in it. ".repeat(20);
    assert!(reply.chars().count() >= NOTE_SIZED_CHARS);
    assert_eq!(should_refuse(&after_words(&reply), POINTS_ABOVE), None);
}

#[test]
fn a_short_reply_passes_when_the_card_points_nowhere() {
    assert_eq!(should_refuse(&after_words(INCIDENT_NOTE), ""), None);
}

#[test]
fn pointing_above_is_refused_once_per_input() {
    let s = SinceLastInput {
        refused: true,
        ..after_words(INCIDENT_NOTE)
    };
    assert_eq!(should_refuse(&s, POINTS_ABOVE), None);
}

#[test]
fn owing_words_keeps_its_own_refusal() {
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..after_words("Let me check.")
    };
    assert!(matches!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::OwesWords { .. })
    ));
}

#[test]
fn above_matches_as_a_whole_word_anywhere_on_the_card() {
    let card = |question: &str, label: &str, description: &str| json!([{ "question": question, "options": [{ "label": label, "description": description }] }]);
    assert!(points_above(&card("Approve the plan Above?", "Yes", "")));
    assert!(points_above(&card("Approve?", "Yes, above.", "")));
    assert!(points_above(&card("Approve?", "Yes", "As listed (above)")));
    assert!(!points_above(&card("Is it aboveboard?", "Yes", "")));
    assert!(!points_above(&card(
        "Which layout?",
        "None of the above",
        ""
    )));
    assert!(!points_above(&card(
        "Which?",
        "All",
        "All OF THE ABOVE apply"
    )));
    assert!(points_above(&card(
        "Which?",
        "None of the above",
        "See the list above"
    )));
    assert!(!points_above(&card("Approve?", "Yes", "No")));
    assert!(!points_above(&serde_json::Value::Null));
}

#[test]
fn the_points_above_refusal_quotes_what_the_user_saw() {
    let text = Refusal::PointsAboveAtNothing {
        words: format!("  {INCIDENT_NOTE}\n\n"),
    }
    .text();
    assert!(text.starts_with(REFUSAL_MARKER));
    assert!(text.contains(&format!("\"{INCIDENT_NOTE}\"")));
    assert!(text.contains("on the card itself"));
    assert!(text.contains("unchanged"));
    assert!(!text.contains('\u{2014}'));

    let nothing = Refusal::PointsAboveAtNothing {
        words: " \n".to_string(),
    }
    .text();
    assert!(nothing.contains("they have read nothing from you"));
    assert!(nothing.starts_with(REFUSAL_MARKER));
}

#[test]
fn a_chat_round_that_spoke_owes_nothing() {
    // Tool work, then this round's words, which are not persisted yet. The
    // words report the work, just as skipping the gate used to.
    let s = SinceLastInput {
        unreported_tool_calls: 3,
        ..since(typed())
    }
    .with_round_text("I found two fixes on main.");
    assert_eq!(s.unreported_tool_calls, 0);
    assert_eq!(should_refuse(&s, ""), None);
}

#[test]
fn a_chat_round_note_still_cannot_hold_what_the_card_points_at() {
    let s = since(message()).with_round_text(INCIDENT_NOTE);
    assert!(matches!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::PointsAboveAtNothing { .. })
    ));
}

#[test]
fn a_persisted_round_is_not_counted_twice() {
    let s = SinceLastInput {
        words: format!("Earlier. {INCIDENT_NOTE}"),
        ..since(message())
    }
    .with_round_text(INCIDENT_NOTE);
    assert_eq!(s.words, format!("Earlier. {INCIDENT_NOTE}"));

    // Only a prefix of the round reached the events: the overlap counts once.
    let (saved, _) = INCIDENT_NOTE.split_at(100);
    let partly = SinceLastInput {
        words: format!("Earlier. {saved}"),
        ..since(message())
    }
    .with_round_text(INCIDENT_NOTE);
    assert_eq!(partly.words, format!("Earlier. {INCIDENT_NOTE}"));

    // Nothing of the round is saved yet: it is appended whole.
    let unsaved = SinceLastInput {
        words: "Earlier. ".to_string(),
        ..since(message())
    }
    .with_round_text(INCIDENT_NOTE);
    assert_eq!(unsaved.words, format!("Earlier. {INCIDENT_NOTE}"));

    let blank = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(typed())
    }
    .with_round_text(" \n");
    assert!(blank.words.is_empty());
    assert_eq!(blank.unreported_tool_calls, 1, "blank text reports nothing");
}

// ---------------------------------------------------------------------------
// A re-ask after a typed answer and a progress note
// ---------------------------------------------------------------------------

/// The incident: the user typed a correction into a card instead of picking.
const CORRECTION: &str = "2. Sry ignored was wrong i mean theres no one to tell it to. I just \
     post into the void";

/// The progress note the chat agent showed before it asked again.
const BORROW_NOTE: &str = "Since you lack an audience, borrowing one is the path forward: get \
     the principal engineer/CTO to post about it, pitch an engineering blog post, lean on \
     Shorts' algorithmic reach, and reply to big accounts.";

/// The card it asked again, which points nowhere.
const DRAFT_EITHER: &str = "Want me to draft either of these?";

#[test]
fn a_re_ask_after_a_typed_answer_and_a_note_passes() {
    let s = since(LastInput::Typed(CORRECTION.to_string())).with_round_text(BORROW_NOTE);
    assert_eq!(should_refuse(&s, DRAFT_EITHER), None);
}

#[test]
fn a_note_after_a_typed_answer_still_cannot_hold_what_the_card_points_at() {
    let s = since(typed()).with_round_text(BORROW_NOTE);
    assert!(matches!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::PointsAboveAtNothing { .. })
    ));
}

#[test]
fn a_long_reply_is_quoted_cut_and_on_one_line() {
    let long = format!("first line\n\n{}", "word ".repeat(100));
    let text = Refusal::OwesWords {
        input: LastInput::Typed(long.clone()),
    }
    .text();
    let quoted = quote(&long);
    assert_eq!(quoted.chars().count(), QUOTED_INPUT_CHARS + "...".len());
    assert!(quoted.starts_with("first line word word"));
    assert!(quoted.ends_with("..."));
    assert!(text.contains(&format!("\"{quoted}\"")));
    assert!(!text.contains(&long));

    let short = "ok \n then";
    assert_eq!(quote(short), "ok then");
    // Cutting counts characters, so a multi-byte reply never splits a char.
    let wide = "\u{e6}".repeat(QUOTED_INPUT_CHARS + 1);
    assert_eq!(
        quote(&wide),
        format!("{}...", "\u{e6}".repeat(QUOTED_INPUT_CHARS))
    );
}

#[test]
fn owing_words_quotes_the_input_it_means() {
    let owes = |input: LastInput| Refusal::OwesWords { input }.text();

    let after_typed = owes(typed());
    assert!(after_typed.starts_with(REFUSAL_MARKER));
    assert!(after_typed.contains(&format!(
        "the text they typed into your card: \"{TYPED_REPLY}\""
    )));
    assert!(!after_typed.contains("reply they typed"));
    assert!(after_typed.contains(NOTHING_NEW));
    assert!(after_typed.contains(OWES_WORDS));

    let after_message = owes(message());
    assert!(after_message.contains("this message: \"what is in the next release?\""));
    assert!(after_message.contains(NOTHING_NEW));

    // Nothing to quote: no claim about an input at all.
    for input in [
        LastInput::Picked,
        LastInput::None,
        LastInput::Message(" ".to_string()),
    ] {
        let text = owes(input);
        assert_eq!(text, format!("{REFUSAL_MARKER} {OWES_WORDS}"));
        assert!(!text.contains(NOTHING_NEW));
    }
    assert!(!after_typed.contains('\u{2014}'));
}

#[test]
fn a_refusal_carries_the_input_it_was_decided_on() {
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(typed())
    };
    assert_eq!(
        should_refuse(&s, ""),
        Some(Refusal::OwesWords { input: typed() })
    );
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(message())
    };
    assert_eq!(
        should_refuse(&s, ""),
        Some(Refusal::OwesWords { input: message() })
    );
}

// ---------------------------------------------------------------------------
// A card after a picture nobody saw
// ---------------------------------------------------------------------------

const MOCKUP: &str = "artifacts/mockups/status-timestamp-dot.png";

/// The summary Claude Code showed in the incident, in place of a reply that
/// held the picture.
const INCIDENT_SUMMARY: &str = "I've generated a mockup comparing today's status line to the \
     proposed one, showing how the dot is replaced by icons or omitted on two-line layouts.";

fn saved_mockup(words: &str) -> SinceLastInput {
    SinceLastInput {
        words: words.to_string(),
        saved_artifacts: vec![MOCKUP.to_string()],
        ..since(message())
    }
}

fn incident_approval_card(preview: Option<&str>) -> serde_json::Value {
    json!([{
        "question": "Go ahead with this look?",
        "header": "Dot",
        "multiSelect": false,
        "options": [
            {"label": "Approve", "description": "Build it as shown in the mockup.", "preview": preview},
            {"label": "Request changes", "description": "Tell me what to adjust first."}
        ]
    }])
}

#[test]
fn a_card_after_a_saved_picture_nobody_saw_is_refused() {
    let card = card_text(&incident_approval_card(None));
    assert_eq!(
        should_refuse(&saved_mockup(INCIDENT_SUMMARY), &card),
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        })
    );
}

#[test]
fn a_picture_on_the_card_passes() {
    let preview = format!("![Mockup]({MOCKUP})");
    let card = card_text(&incident_approval_card(Some(&preview)));
    assert_eq!(should_refuse(&saved_mockup(INCIDENT_SUMMARY), &card), None);

    let in_question = format!("Go ahead with this look?\n![Mockup]({MOCKUP})");
    assert_eq!(
        should_refuse(&saved_mockup(INCIDENT_SUMMARY), &in_question),
        None
    );
}

#[test]
fn a_picture_in_the_words_passes_in_every_form_the_renderer_takes() {
    for shown in [
        format!("Here it is: ![Mockup]({MOCKUP})"),
        format!("Here it is: ![Mockup](<{MOCKUP}>)"),
        format!("Here it is: ![Mockup](data/{MOCKUP})"),
    ] {
        assert_eq!(should_refuse(&saved_mockup(&shown), ""), None, "{shown}");
    }
}

/// `lucidos data write` prints the picture line with an image size hint, and
/// the agent pastes it as printed.
#[test]
fn a_picture_carrying_a_size_hint_passes() {
    for shown in [
        format!("![Mockup]({MOCKUP}#1600x1200)"),
        format!("![Mockup](<{MOCKUP}#1600x1200>)"),
    ] {
        let card = card_text(&incident_approval_card(Some(&shown)));
        assert_eq!(
            should_refuse(&saved_mockup(INCIDENT_SUMMARY), &card),
            None,
            "{shown}"
        );
    }
    let other_file = format!("![Backup]({MOCKUP}x#10x10)");
    assert!(matches!(
        should_refuse(&saved_mockup(&other_file), ""),
        Some(Refusal::ArtifactNotShown { .. })
    ));
}

#[test]
fn a_bare_path_a_link_or_another_file_is_not_the_picture() {
    for words in [
        format!("I saved it to {MOCKUP}."),
        format!("Tap to open: [Mockup]({MOCKUP})"),
        format!("![Old](x.png) and a link: [Mockup]({MOCKUP})"),
        format!("![Backup]({MOCKUP}.bak)"),
    ] {
        assert!(
            matches!(
                should_refuse(&saved_mockup(&words), ""),
                Some(Refusal::ArtifactNotShown { .. })
            ),
            "{words}"
        );
    }
}

#[test]
fn a_picture_refusal_is_sent_once_per_input() {
    let s = SinceLastInput {
        refused: true,
        artifact_refused: true,
        ..saved_mockup(INCIDENT_SUMMARY)
    };
    assert_eq!(should_refuse(&s, ""), None);
}

/// The third incident: the card showed the picture, a refusal for owing
/// words followed, and the agent's retry dropped the picture.
#[test]
fn a_retry_that_drops_a_picture_is_refused_after_another_refusal() {
    let s = SinceLastInput {
        refused: true,
        ..saved_mockup(INCIDENT_SUMMARY)
    };
    assert_eq!(
        should_refuse(&s, &card_text(&incident_approval_card(None))),
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        })
    );
    let preview = format!("![Mockup]({MOCKUP})");
    assert_eq!(
        should_refuse(&s, &card_text(&incident_approval_card(Some(&preview)))),
        None,
        "a retry that keeps the picture passes"
    );
}

/// The first refusal for an input goes to the picture. Spent on owing words,
/// it let the retry through without the picture: the second incident.
#[test]
fn a_missing_picture_comes_before_owing_words() {
    for s in [
        SinceLastInput {
            unreported_tool_calls: 3,
            ..saved_mockup(INCIDENT_SUMMARY)
        },
        SinceLastInput {
            last_input: typed(),
            unreported_tool_calls: 1,
            ..saved_mockup("")
        },
    ] {
        assert_eq!(
            should_refuse(&s, ""),
            Some(Refusal::ArtifactNotShown {
                paths: vec![MOCKUP.to_string()]
            })
        );
    }
}

#[test]
fn a_missing_picture_comes_before_pointing_above() {
    assert!(matches!(
        should_refuse(&saved_mockup(INCIDENT_SUMMARY), POINTS_ABOVE),
        Some(Refusal::ArtifactNotShown { .. })
    ));
}

#[test]
fn the_artifact_refusal_names_the_path_and_the_card() {
    let text = Refusal::ArtifactNotShown {
        paths: vec![MOCKUP.to_string(), "artifacts/b.png".to_string()],
    }
    .text();
    assert!(text.starts_with(REFUSAL_MARKER));
    assert!(text.contains(&format!("`{MOCKUP}`, `artifacts/b.png`")));
    assert!(text.contains("short summary"));
    assert!(text.contains("ON the card"));
    // Claude Code's tool has `preview`, the chat agent's has a description,
    // and Codex's has neither, so it falls back to the question.
    assert!(text.contains("its `preview` or description, whichever your tool has"));
    assert!(text.contains("its own picture of only that option"));
    assert!(text.contains("`[...](...)` link to any other file"));
    assert!(text.contains("an option shows a link as plain text"));
    // It outranks owing words, so it carries both of that refusal's asks.
    assert!(text.contains("answer it in plain prose"));
    assert!(text.contains("say what you found"));
    assert!(text.contains("unchanged"));
    assert!(!text.contains('\u{2014}'));
}

#[test]
fn only_files_saved_as_artifacts_count() {
    for path in [
        "artifacts/design/a.png",
        "artifacts/notes.md",
        "artifacts/explainer.html",
    ] {
        assert!(is_shown_artifact(path), "{path}");
    }
    for path in ["apps/habit-tracker/icon.png", "knowhow/diagram.svg"] {
        assert!(!is_shown_artifact(path), "{path}");
    }
}

#[test]
fn only_picture_extensions_need_the_image_form() {
    for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.svg", "a.webp"] {
        assert!(is_picture(&format!("artifacts/design/{name}")), "{name}");
    }
    for path in [
        "artifacts/notes.md",
        "artifacts/png-notes.md",
        "artifacts/png",
    ] {
        assert!(!is_picture(path), "{path}");
    }
}

// ---------------------------------------------------------------------------
// A card after a page nobody linked
// ---------------------------------------------------------------------------

const EXPLAINER: &str = "artifacts/tree-memory-corrections.html";

/// The summary Claude Code showed in the incident, in place of a reply that
/// held the link to the explainer.
const EXPLAINER_SUMMARY: &str = "I've put together a one-page overview covering the log and \
     tree approach, plus two glossary definitions included below.";

fn saved_explainer(words: &str) -> SinceLastInput {
    SinceLastInput {
        words: words.to_string(),
        saved_artifacts: vec![EXPLAINER.to_string()],
        ..since(typed())
    }
}

fn explainer_link() -> String {
    format!("[tree-memory-corrections.html]({EXPLAINER})")
}

/// The incident's card. `question_link` and `preview` go where named.
fn glossary_card(question_link: &str, preview: Option<&str>) -> serde_json::Value {
    json!([{
        "question": format!("Add this entry to docs/glossary.md?\n\n**Workspace anchor**: a line in a thread memory view.\n{question_link}"),
        "header": "Glossary",
        "multiSelect": false,
        "options": [
            {"label": "Add as proposed", "description": "Goes in beside Memory view.", "preview": preview},
            {"label": "Skip, not a real term", "description": "Describe it inside the entry."}
        ]
    }])
}

#[test]
fn a_card_after_a_saved_page_nobody_linked_is_refused() {
    assert_eq!(
        should_refuse(
            &saved_explainer(EXPLAINER_SUMMARY),
            &card_text(&glossary_card("", None))
        ),
        Some(Refusal::ArtifactNotShown {
            paths: vec![EXPLAINER.to_string()]
        })
    );
}

#[test]
fn a_page_linked_in_the_words_or_the_question_passes() {
    for shown in [explainer_link(), format!("[Explainer](<data/{EXPLAINER}>)")] {
        assert_eq!(should_refuse(&saved_explainer(&shown), ""), None, "{shown}");
    }
    let card = card_text(&glossary_card(&explainer_link(), None));
    assert_eq!(
        should_refuse(&saved_explainer(EXPLAINER_SUMMARY), &card),
        None
    );
}

/// An option renders inside a button, which shows a link as its label alone.
#[test]
fn a_page_linked_only_in_an_option_is_refused() {
    let link = explainer_link();
    let card = card_text(&glossary_card("", Some(&link)));
    assert!(matches!(
        should_refuse(&saved_explainer(EXPLAINER_SUMMARY), &card),
        Some(Refusal::ArtifactNotShown { .. })
    ));
}

#[test]
fn an_option_keeps_its_pictures_and_loses_its_link_targets() {
    assert_eq!(
        without_link_targets("see [the page](artifacts/a.html) and ![it](artifacts/a.png) too"),
        "see [the page] and ![it](artifacts/a.png) too"
    );
    assert_eq!(without_link_targets("[cut](artifacts/a.html"), "[cut]");
    assert_eq!(without_link_targets("no markdown"), "no markdown");
}

/// Markdown ends a bare target at a space, so only the bracketed form opens.
#[test]
fn a_page_with_a_space_needs_the_bracketed_link() {
    let s = |words: &str| SinceLastInput {
        saved_artifacts: vec!["artifacts/quarterly report.html".to_string()],
        ..saved_explainer(words)
    };
    let bracketed = "[report](<artifacts/quarterly report.html>)";
    assert_eq!(should_refuse(&s(bracketed), ""), None);
    let bare = "[report](artifacts/quarterly report.html)";
    assert!(matches!(
        should_refuse(&s(bare), ""),
        Some(Refusal::ArtifactNotShown { .. })
    ));
}

#[test]
fn a_bare_path_or_an_image_line_does_not_link_a_page() {
    for words in [
        format!("I saved it to {EXPLAINER}."),
        format!("[Old](other.html) then {EXPLAINER}"),
        format!("![Explainer]({EXPLAINER})"),
    ] {
        assert!(
            matches!(
                should_refuse(&saved_explainer(&words), ""),
                Some(Refusal::ArtifactNotShown { .. })
            ),
            "{words}"
        );
    }
}

// ---------------------------------------------------------------------------
// A card whose question carries the report
// ---------------------------------------------------------------------------

/// A card question in the incident's shape: the findings, the picture, then
/// the decision. The agent had saved the picture, then asked.
fn reporting_card(picture: &str) -> serde_json::Value {
    let findings = "- The hold opens a different menu from each button. ".repeat(10);
    json!([{
        "question": format!(
            "Here is every state today:\n\n![Today]({picture})\n\nWhat differs:\n{findings}\nShould I build the proposal?"
        ),
        "header": "Unify",
        "multiSelect": false,
        "options": [
            {"label": "Build the proposal", "description": "One mode for every entry."},
            {"label": "Only fix the gaps", "description": "Keep today's UI."}
        ]
    }])
}

/// What the gate decides for `questions`, as `refuse_card` folds it.
fn decide(s: SinceLastInput, questions: &serde_json::Value) -> Option<Refusal> {
    should_refuse(
        &s.with_card_question(&longest_card_question(questions)),
        &card_text(questions),
    )
}

/// The incident: the card held the findings and the picture, and a refusal
/// for owing words made the agent re-send it without either.
#[test]
fn a_card_whose_question_carries_the_report_owes_no_words() {
    let card = reporting_card(MOCKUP);
    assert!(longest_card_question(&card).chars().count() >= NOTE_SIZED_CHARS);
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..saved_mockup("Both render correctly. Storing them so you can see them.")
    };
    assert_eq!(decide(s, &card), None);
}

#[test]
fn a_long_card_question_still_cannot_point_above() {
    let card = json!([{
        "question": format!("{} Go with the copy above?", "Some context. ".repeat(50)),
        "options": [{"label": "Yes"}]
    }]);
    assert!(longest_card_question(&card).chars().count() >= NOTE_SIZED_CHARS);
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..after_words(INCIDENT_NOTE)
    };
    assert!(matches!(
        decide(s, &card),
        Some(Refusal::PointsAboveAtNothing { .. })
    ));
}

#[test]
fn several_short_questions_do_not_add_up_to_a_report() {
    let question = "Which of these approaches should I take for the module? ".repeat(3);
    let card = json!([
        {"question": question, "options": [{"label": "A"}]},
        {"question": question, "options": [{"label": "B"}]},
        {"question": question, "options": [{"label": "C"}]},
        {"question": question, "options": [{"label": "D"}]}
    ]);
    assert!(4 * question.chars().count() >= NOTE_SIZED_CHARS);
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(message())
    };
    assert!(matches!(decide(s, &card), Some(Refusal::OwesWords { .. })));
}

#[test]
fn a_short_card_question_does_not_report_the_work() {
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(message())
    };
    assert!(matches!(
        decide(s, &incident_approval_card(None)),
        Some(Refusal::OwesWords { .. })
    ));
}

/// The first incident's card: "How do you want to continue?", with the
/// answer only in the option text.
#[test]
fn a_report_only_in_the_options_does_not_count() {
    let long = "The release holds two fixes and a new setting. ".repeat(15);
    let card = json!([{
        "question": "How do you want to continue?",
        "options": [{"label": "Ship it", "description": long}]
    }]);
    assert_eq!(longest_card_question(&card), "How do you want to continue?");
    let s = SinceLastInput {
        unreported_tool_calls: 1,
        ..since(message())
    };
    assert!(matches!(decide(s, &card), Some(Refusal::OwesWords { .. })));
}

#[test]
fn a_reporting_card_still_needs_the_picture() {
    let card = reporting_card("artifacts/an-older-picture.png");
    assert_eq!(
        decide(saved_mockup(INCIDENT_SUMMARY), &card),
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        })
    );
}

// ---------------------------------------------------------------------------
// The query, against the event shapes the agents actually write
// ---------------------------------------------------------------------------

async fn insert(pool: &PgPool, thread_id: Uuid, event_type: &str, payload: serde_json::Value) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, $2, $3, $4, 'thread', $4::text)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(payload)
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert event");
}

async fn cc_text(pool: &PgPool, thread_id: Uuid, text: &str) {
    insert(
        pool,
        thread_id,
        "CodingAgentTextStreamed",
        json!({ "text": text }),
    )
    .await;
}

async fn cc_tool(pool: &PgPool, thread_id: Uuid, name: &str) {
    insert(
        pool,
        thread_id,
        "CodingAgentToolCalled",
        json!({ "name": name, "args": {} }),
    )
    .await;
}

async fn answered(pool: &PgPool, thread_id: Uuid, answer: serde_json::Value) {
    insert(
        pool,
        thread_id,
        "UserQuestionAnswered",
        json!({ "tool_use_id": format!("toolu_{}#q0", Uuid::new_v4()), "answer": answer }),
    )
    .await;
}

#[tokio::test]
async fn the_query_reads_what_each_agent_writes() {
    let (pool, db) = setup_test_db().await;

    // Claude Code: a typed reply to a card, then only blank text.
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "audit", "mode": "human" }),
    )
    .await;
    cc_tool(&pool, t, "Grep").await;
    cc_text(&pool, t, "Here is the plan.").await;
    answered(
        &pool,
        t,
        json!({ "kind": "FreeText", "text": "is there a difference?" }),
    )
    .await;
    cc_text(&pool, t, "\n\n").await;
    cc_tool(&pool, t, crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL).await;
    cc_tool(&pool, t, "TodoWrite").await;
    assert!(
        refuse_card(&pool, t, "toolu_next", &json!([]), None, None)
            .await
            .is_none(),
        "a re-ask after a typed answer passes, and question tools are not work"
    );
    cc_tool(&pool, t, "Grep").await;
    cc_text(&pool, t, "\n\n").await;
    assert_eq!(
        refuse_card(&pool, t, "toolu_next", &json!([]), None, None).await,
        Some(Refusal::OwesWords {
            input: LastInput::Typed("is there a difference?".to_string())
        }),
        "blank text reports no work, and the refusal quotes the typed text"
    );

    // The refusal comes back as a tool result, so the re-sent card passes.
    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": card_refusal() }),
    )
    .await;
    assert!(
        refuse_card(&pool, t, "toolu_again", &json!([]), None, None)
            .await
            .is_none(),
        "refused once per input"
    );

    // Chat: a message, tool work, no text.
    let c = Uuid::new_v4();
    insert(
        &pool,
        c,
        "MessageReceived",
        json!({ "text": "what is in .3", "mode": "human" }),
    )
    .await;
    insert(
        &pool,
        c,
        "ToolCalled",
        json!({ "name": "run_bash", "args": {} }),
    )
    .await;
    assert_eq!(
        refuse_card(&pool, c, "toolu_chat", &json!([]), None, None).await,
        Some(Refusal::OwesWords {
            input: LastInput::Message("what is in .3".to_string())
        }),
        "the refusal names the message"
    );
    assert!(
        refuse_coding_agent_card(&pool, c, "toolu_chat", &json!([]))
            .await
            .is_some(),
        "both looks agree"
    );
    insert(
        &pool,
        c,
        "TextStreamed",
        json!({ "text": "Two fixes are on main." }),
    )
    .await;
    assert!(
        refuse_card(&pool, c, "toolu_chat", &json!([]), None, None)
            .await
            .is_none(),
        "the report came first"
    );
    insert(
        &pool,
        c,
        "ToolCalled",
        json!({ "name": "changes", "args": {} }),
    )
    .await;
    assert!(
        refuse_card(&pool, c, "toolu_chat", &json!([]), None, None)
            .await
            .is_some(),
        "work after the words is unreported"
    );

    // A pick, then the next card at once.
    let p = Uuid::new_v4();
    insert(
        &pool,
        p,
        "MessageReceived",
        json!({ "text": "release", "mode": "human" }),
    )
    .await;
    answered(
        &pool,
        p,
        json!({ "kind": "Selected", "option_id": "opt-0" }),
    )
    .await;
    assert!(refuse_card(&pool, p, "toolu_chain", &json!([]), None, None)
        .await
        .is_none());

    // A card already shown is a crash-recovery re-POST and always passes.
    insert(
        &pool,
        t,
        "UserQuestionAsked",
        json!({ "tool_use_id": "toolu_shown#q0", "question": "Approve?", "options": [] }),
    )
    .await;
    answered(&pool, t, json!({ "kind": "FreeText", "text": "why?" })).await;
    cc_tool(&pool, t, "Read").await;
    assert!(refuse_card(&pool, t, "toolu_shown", &json!([]), None, None)
        .await
        .is_none());

    teardown_test_db(&db).await;
}

/// Coding-agent results are stored whole, so a long grep over the gate's own
/// source can quote the marker far into its output. Only the start of a result
/// may stand for "already refused".
#[tokio::test]
async fn a_result_quoting_the_marker_deep_in_its_output_is_not_a_refusal() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "audit", "mode": "human" }),
    )
    .await;
    cc_tool(&pool, t, "Grep").await;
    let earlier_lines = "engine/question_card_gate.rs:1:use sqlx::PgPool;\n".repeat(10);
    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({
            "name": "Grep",
            "result": format!("{earlier_lines}engine/question_card_gate.rs:22:const REFUSAL_MARKER: &str = \"{REFUSAL_MARKER}\";"),
        }),
    )
    .await;
    assert!(
        refuse_card(&pool, t, "toolu_grep", &json!([]), None, None)
            .await
            .is_some(),
        "a grep that quotes the marker must not count as a refusal"
    );
    teardown_test_db(&db).await;
}

/// Codex returns the refusal as an MCP result, which its driver stores as the
/// result's JSON. The marker then sits inside the wrapper, and still counts.
#[tokio::test]
async fn a_codex_refusal_in_its_mcp_wrapper_counts() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "audit", "mode": "human" }),
    )
    .await;
    cc_tool(&pool, t, "Grep").await;
    assert!(refuse_card(&pool, t, "toolu_codex", &json!([]), None, None)
        .await
        .is_some());
    let wrapped =
        json!({ "content": [{ "type": "text", "text": card_refusal() }], "isError": true });
    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "mcp__lucidos__ask_user_question", "result": wrapped.to_string() }),
    )
    .await;
    assert!(
        refuse_card(&pool, t, "toolu_codex", &json!([]), None, None)
            .await
            .is_none(),
        "refused once per input"
    );
    teardown_test_db(&db).await;
}

/// The incident end to end: the words since the typed reply are read back
/// from the events, joined in order, and quoted in the refusal.
#[tokio::test]
async fn the_query_reads_the_words_a_card_points_above_at() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    cc_text(
        &pool,
        t,
        "A long reply from before the answer. ".repeat(30).as_str(),
    )
    .await;
    answered(
        &pool,
        t,
        json!({ "kind": "FreeText", "text": "paths and ids need no explanation?" }),
    )
    .await;
    let (first, rest) = INCIDENT_NOTE.split_at(40);
    cc_text(&pool, t, first).await;
    cc_text(&pool, t, rest).await;

    let refusal = refuse_card(&pool, t, "toolu_above", &incident_card(), None, None).await;
    assert_eq!(
        refusal,
        Some(Refusal::PointsAboveAtNothing {
            words: INCIDENT_NOTE.to_string()
        }),
        "only the words since the answer count, in order"
    );

    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": refusal.unwrap().text() }),
    )
    .await;
    assert!(
        refuse_card(&pool, t, "toolu_above", &incident_card(), None, None)
            .await
            .is_none(),
        "an unchanged re-send is never refused twice"
    );

    teardown_test_db(&db).await;
}

/// The `DataFileWritten` shape `PUT /api/v1/data` emits: no thread_id, the
/// saving thread in the actor.
async fn data_file_written(pool: &PgPool, saved_by: Uuid, path: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
         VALUES ($1, 'DataFileWritten', $2, 'data_file', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({
        "type": "DataFileWritten",
        "data": {
            "path": path,
            "actor": { "kind": "api", "mode": "agent", "source_thread_id": saved_by },
        },
    }))
    .bind(path)
    .execute(pool)
    .await
    .expect("insert DataFileWritten");
}

/// The incident end to end, with a decoy for each save that must not count.
#[tokio::test]
async fn the_query_finds_the_artifacts_this_thread_saved_since_the_input() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    data_file_written(&pool, t, "artifacts/before-the-input.png").await;
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "show me the dot options", "mode": "human" }),
    )
    .await;
    cc_text(&pool, t, "I'll save it so you can see it.").await;
    cc_tool(&pool, t, "Bash").await;
    data_file_written(&pool, t, MOCKUP).await;
    data_file_written(&pool, t, "knowhow/mockups/notes.md").await;
    data_file_written(&pool, Uuid::new_v4(), "artifacts/another-thread.png").await;
    cc_text(&pool, t, INCIDENT_SUMMARY).await;

    let refusal = refuse_card(
        &pool,
        t,
        "toolu_card",
        &incident_approval_card(None),
        None,
        None,
    )
    .await;
    assert_eq!(
        refusal,
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        }),
        "only this thread's artifact since the input counts"
    );

    let preview = format!("![Mockup]({MOCKUP})");
    assert!(
        refuse_card(
            &pool,
            t,
            "toolu_card",
            &incident_approval_card(Some(&preview)),
            None,
            None
        )
        .await
        .is_none(),
        "the picture on the card shows it"
    );

    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": refusal.unwrap().text() }),
    )
    .await;
    assert!(
        refuse_card(
            &pool,
            t,
            "toolu_card",
            &incident_approval_card(None),
            None,
            None
        )
        .await
        .is_none(),
        "an unchanged re-send is never refused twice"
    );

    teardown_test_db(&db).await;
}

/// The second incident: the user typed a complaint into a card, the agent
/// rendered a mockup with tool work it never reported, then asked again.
#[tokio::test]
async fn a_rendered_mockup_after_a_typed_reply_gets_the_picture_refusal() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    answered(
        &pool,
        t,
        json!({ "kind": "FreeText", "text": "ascii art doesn't show me the colour" }),
    )
    .await;
    cc_text(&pool, t, "Fair point. I'll render real mockups.").await;
    cc_tool(&pool, t, "Bash").await;
    cc_tool(&pool, t, "Read").await;
    cc_tool(&pool, t, "Bash").await;
    data_file_written(&pool, t, MOCKUP).await;

    let refusal = refuse_card(
        &pool,
        t,
        "toolu_card",
        &incident_approval_card(None),
        None,
        None,
    )
    .await;
    assert_eq!(
        refusal,
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        }),
        "the only refusal this input gets must ask for the picture"
    );

    teardown_test_db(&db).await;
}

/// The third incident end to end: a short note, the save, then a card whose
/// question held the findings and the picture.
#[tokio::test]
async fn the_query_passes_a_card_that_carries_its_report() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "show me and suggest unification", "mode": "human" }),
    )
    .await;
    cc_tool(&pool, t, "Bash").await;
    cc_text(
        &pool,
        t,
        "Both render correctly. Storing them so you can see them.",
    )
    .await;
    cc_tool(&pool, t, "Bash").await;
    data_file_written(&pool, t, MOCKUP).await;

    assert_eq!(
        refuse_card(&pool, t, "toolu_card", &reporting_card(MOCKUP), None, None).await,
        None
    );
    teardown_test_db(&db).await;
}

/// After a refusal for owing words, the retry dropped the picture the first
/// card showed. It gets one more refusal, and only one.
#[tokio::test]
async fn the_query_refuses_a_retry_that_drops_the_picture_once() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "show me the dot options", "mode": "human" }),
    )
    .await;
    cc_tool(&pool, t, "Bash").await;
    data_file_written(&pool, t, MOCKUP).await;
    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": card_refusal() }),
    )
    .await;
    cc_text(&pool, t, INCIDENT_SUMMARY).await;

    let refusal = refuse_card(
        &pool,
        t,
        "toolu_retry",
        &incident_approval_card(None),
        None,
        None,
    )
    .await;
    assert_eq!(
        refusal,
        Some(Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()]
        })
    );

    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": refusal.unwrap().text() }),
    )
    .await;
    assert!(
        refuse_card(
            &pool,
            t,
            "toolu_retry",
            &incident_approval_card(None),
            None,
            None
        )
        .await
        .is_none(),
        "the picture refusal is never sent twice"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_query_error_shows_the_card() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "hi", "mode": "human" }),
    )
    .await;
    insert(
        &pool,
        t,
        "ToolCalled",
        json!({ "name": "run_bash", "args": {} }),
    )
    .await;
    pool.close().await;
    assert!(refuse_card(&pool, t, "toolu_x", &json!([]), None, None)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

// ---------------------------------------------------------------------------
// A card that carries its own answer in `message`
// ---------------------------------------------------------------------------

/// The steps the incident's agent drafted and never showed, short enough to
/// be note-sized.
const CARD_MESSAGE: &str = "1. Open Web Analytics and press **Manage site**.\n\
     2. Change the setup choice to **Enable**.\n3. Save.";

#[test]
fn a_card_message_reports_the_tool_work_before_it() {
    let s = SinceLastInput {
        unreported_tool_calls: 3,
        ..since(message())
    }
    .with_card_message(CARD_MESSAGE);
    assert_eq!(should_refuse(&s, ""), None);
}

#[test]
fn a_card_may_point_above_at_its_own_message() {
    let s = since(LastInput::Picked).with_card_message(CARD_MESSAGE);
    assert_eq!(should_refuse(&s, POINTS_ABOVE), None);
}

#[test]
fn a_blank_card_message_changes_nothing() {
    for input in [typed(), message(), LastInput::Picked] {
        let base = SinceLastInput {
            unreported_tool_calls: 1,
            ..since(input)
        };
        let with_blank = base.clone().with_card_message(" \n");
        assert_eq!(with_blank, base);
        assert!(should_refuse(&with_blank, POINTS_ABOVE).is_some());
    }
}

#[test]
fn a_card_message_still_has_to_show_a_saved_picture() {
    let s = saved_mockup("").with_card_message(CARD_MESSAGE);
    assert!(matches!(
        should_refuse(&s, ""),
        Some(Refusal::ArtifactNotShown { .. })
    ));
    let shown =
        saved_mockup("").with_card_message(&format!("{CARD_MESSAGE}\n\n![The mockup]({MOCKUP})"));
    assert_eq!(should_refuse(&shown, ""), None);
}

#[test]
fn every_refusal_names_the_message_field() {
    let refusals = [
        Refusal::OwesWords { input: typed() },
        Refusal::PointsAboveAtNothing {
            words: INCIDENT_NOTE.to_string(),
        },
        Refusal::ArtifactNotShown {
            paths: vec![MOCKUP.to_string()],
        },
    ];
    for refusal in refusals {
        let text = refusal.text();
        assert!(text.contains(CARD_MESSAGE_HINT), "{text}");
        assert!(!text.contains('\u{2014}'));
    }
}

// ---------------------------------------------------------------------------
// A card after a progress note
// ---------------------------------------------------------------------------

/// The incident's only visible text: the provider's summary of the steps the
/// agent wrote before its card. The steps never reached the user.
const SUMMARY_NOTE: &str = "I've laid out the cancellation steps and what it affects here: \
     coding agents keep working fine since they already run on Vertex, but you'd lose access \
     to Fable 5/5.1 and the \"Anthropic sub\" button in the provider app would start \
     failing after the paid period ends.";

/// The incident's card, which carried no `message`.
fn cancellation_card() -> serde_json::Value {
    json!([{
        "question": "Should I record the cancellation, so no thread or trigger switches to \
                     the subscription later?",
        "options": [
            { "label": "Record it", "description": "Note the end of the subscription in the \
              provider-switch knowhow and mark the Anthropic mode as dead." },
            { "label": "Not now", "description": "Leave everything as it is." }
        ]
    }])
}

fn cancel_message() -> LastInput {
    LastInput::Message("i need to cancel my anthropic sub, what do i do".to_string())
}

/// The incident round: tool work, then the summary as the round's only text.
fn after_summary() -> SinceLastInput {
    SinceLastInput {
        unreported_tool_calls: 2,
        ..since(cancel_message())
    }
    .with_round(ChatRound {
        text: SUMMARY_NOTE,
        progress_notes: &[SUMMARY_NOTE.to_string()],
    })
}

/// Prose after the note is a reply the user read in full, so the card passes.
#[test]
fn text_after_a_progress_note_passes() {
    let text = format!("{SUMMARY_NOTE}\nCancel at Settings > Billing.");
    let s = since(cancel_message()).with_round(ChatRound {
        text: &text,
        progress_notes: &[SUMMARY_NOTE.to_string()],
    });
    assert_eq!(s.last_progress_note, None);
    assert_eq!(should_refuse(&s, &card_text(&cancellation_card())), None);
}

#[test]
fn the_incident_card_after_a_progress_note_is_refused() {
    let card = card_text(&cancellation_card());
    assert_eq!(
        should_refuse(&after_summary(), &card),
        Some(Refusal::NoteShownAsSummary {
            note: SUMMARY_NOTE.to_string()
        })
    );
}

/// The same words as plain text are a real reply, so the length rule and
/// the new rule both leave the card alone. This is why the old gate missed
/// the incident: only the block type tells the two apart.
#[test]
fn the_same_words_as_text_pass() {
    let s = SinceLastInput {
        unreported_tool_calls: 2,
        ..since(cancel_message())
    }
    .with_round_text(SUMMARY_NOTE);
    assert_eq!(should_refuse(&s, &card_text(&cancellation_card())), None);
}

#[test]
fn a_card_with_a_message_after_a_progress_note_passes() {
    let s = after_summary().with_card_message("## How to cancel\n\n1. Open Settings > Billing.");
    assert_eq!(should_refuse(&s, &card_text(&cancellation_card())), None);
}

#[test]
fn a_second_card_after_a_progress_note_passes() {
    let s = SinceLastInput {
        refused: true,
        ..after_summary()
    };
    assert_eq!(should_refuse(&s, &card_text(&cancellation_card())), None);
}

/// An unseen artifact still outranks the note: its refusal is the one a
/// retry could otherwise drop a picture past.
#[test]
fn an_unseen_artifact_outranks_a_progress_note() {
    let s = SinceLastInput {
        saved_artifacts: vec!["artifacts/steps.png".to_string()],
        ..after_summary()
    };
    assert_eq!(
        should_refuse(&s, &card_text(&cancellation_card())),
        Some(Refusal::ArtifactNotShown {
            paths: vec!["artifacts/steps.png".to_string()]
        })
    );
}

#[test]
fn the_note_refusal_quotes_the_summary_and_names_the_verbatim_places() {
    let text = Refusal::NoteShownAsSummary {
        note: SUMMARY_NOTE.to_string(),
    }
    .text();
    assert!(text.starts_with(REFUSAL_MARKER));
    assert!(text.contains("I've laid out the cancellation steps"));
    assert!(text.contains(CARD_MESSAGE_HINT));
    assert!(text.contains("in the question"));
    assert!(text.contains("will not be refused twice"));
}

#[tokio::test]
async fn the_query_reads_a_claude_code_progress_note() {
    let (pool, db) = setup_test_db().await;
    let t = Uuid::new_v4();
    insert(
        &pool,
        t,
        "MessageReceived",
        json!({ "text": "explain it", "mode": "human" }),
    )
    .await;
    cc_text(&pool, t, "Reading the code first.").await;
    insert(
        &pool,
        t,
        "CodingAgentTextStreamed",
        json!({ "text": "\n\nI've put together a one-page overview.", "progress_note": true }),
    )
    .await;
    assert_eq!(
        refuse_card(&pool, t, "toolu_card", &cancellation_card(), None, None).await,
        Some(Refusal::NoteShownAsSummary {
            note: "I've put together a one-page overview.".to_string()
        }),
        "a progress note as the last words refuses the card"
    );

    // A sub-agent's narration is not the session's reply, so it hides nothing.
    insert(
        &pool,
        t,
        "CodingAgentTextStreamed",
        json!({ "text": "Scanning the tests.", "parent_tool_use_id": "toolu_agent" }),
    )
    .await;
    assert!(
        matches!(
            refuse_card(&pool, t, "toolu_card", &cancellation_card(), None, None).await,
            Some(Refusal::NoteShownAsSummary { .. })
        ),
        "sub-agent text after the note leaves it the last thing the user read"
    );

    // Prose written after the note is what the user read last.
    cc_text(&pool, t, "\n\nThe overview covers the three phases.").await;
    assert!(
        refuse_card(&pool, t, "toolu_card", &cancellation_card(), None, None)
            .await
            .is_none(),
        "text after the note is a reply the user read in full"
    );

    teardown_test_db(&db).await;
}
