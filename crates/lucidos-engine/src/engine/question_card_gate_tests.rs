use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

fn since(last_input: LastInput) -> SinceLastInput {
    SinceLastInput {
        last_input,
        spoke: false,
        unreported_tool_calls: 0,
        refused: false,
        words: String::new(),
        saved_pictures: Vec::new(),
        round_was_notes: false,
    }
}

/// A card that points "above".
const POINTS_ABOVE: &str = "Go with the copy above?";

fn points_above(questions: &serde_json::Value) -> bool {
    says_above(&card_text(questions))
}

fn owes_words(s: SinceLastInput) -> bool {
    should_refuse(&s, "") == Some(Refusal::OwesWords)
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

#[test]
fn a_card_after_a_typed_reply_with_nothing_said_is_refused() {
    // The Claude Code case: the user typed a question into a plan card and got
    // "With that answered: approve the plan?" with no answer and no tool call.
    assert!(owes_words(since(LastInput::Typed)));
}

#[test]
fn a_card_after_unreported_tool_work_is_refused() {
    // The chat case: asked what the next release holds, the agent ran git log
    // and raised "How do you want to continue?".
    for input in [LastInput::Message, LastInput::Picked, LastInput::None] {
        assert!(owes_words(SinceLastInput {
            unreported_tool_calls: 2,
            ..since(input)
        }));
    }
}

#[test]
fn a_clarifying_question_with_no_work_passes() {
    assert!(!owes_words(since(LastInput::Message)));
}

#[test]
fn a_pick_followed_at_once_by_the_next_card_passes() {
    assert!(!owes_words(since(LastInput::Picked)));
}

#[test]
fn words_after_the_work_pass() {
    assert!(!owes_words(SinceLastInput {
        spoke: true,
        ..since(LastInput::Typed)
    }));
}

#[test]
fn a_preamble_does_not_report_the_work_after_it() {
    // "Let me check the log", then git log, then a card: the log went unreported.
    assert!(owes_words(SinceLastInput {
        spoke: true,
        unreported_tool_calls: 1,
        ..since(LastInput::Message)
    }));
}

#[test]
fn one_refusal_per_input() {
    assert!(!owes_words(SinceLastInput {
        refused: true,
        unreported_tool_calls: 3,
        ..since(LastInput::Typed)
    }));
}

#[test]
fn a_typed_answer_carries_text() {
    let typed = |a: serde_json::Value| LastInput::from_answer(&a);
    assert_eq!(
        typed(json!({"kind": "FreeText", "text": "is there a difference?"})),
        LastInput::Typed
    );
    assert_eq!(
        typed(json!({"kind": "MultiSelected", "option_ids": ["opt-0"], "text": "and X"})),
        LastInput::Typed
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
    assert!(CARD_REFUSAL.starts_with(REFUSAL_MARKER));
    assert!(CARD_REFUSAL.contains("never your tool results"));
    assert!(CARD_REFUSAL.contains("unchanged"));
    assert!(!CARD_REFUSAL.contains('\u{2014}'));
}

/// Every agent's notes arrive as text, the coding agents' through the Vertex
/// relay. So the refusal asks for prose and never blames hidden reasoning.
#[test]
fn the_refusal_asks_for_prose() {
    assert!(CARD_REFUSAL.contains("plain prose"));
    assert!(!CARD_REFUSAL.contains("hidden reasoning"));
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
        spoke: !words.trim().is_empty(),
        words: words.to_string(),
        ..since(LastInput::Typed)
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
    assert_eq!(should_refuse(&s, POINTS_ABOVE), Some(Refusal::OwesWords));
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

    assert_eq!(Refusal::OwesWords.text(), CARD_REFUSAL);
}

#[test]
fn a_chat_round_that_spoke_owes_nothing() {
    // Tool work, then this round's words, which are not persisted yet. The
    // words report the work, just as skipping the gate used to.
    let s = SinceLastInput {
        unreported_tool_calls: 3,
        ..since(LastInput::Typed)
    }
    .with_round_text(RoundText::Reply("I found two fixes on main."));
    assert!(s.spoke);
    assert_eq!(s.unreported_tool_calls, 0);
    assert_eq!(should_refuse(&s, ""), None);
}

#[test]
fn a_chat_round_note_still_cannot_hold_what_the_card_points_at() {
    let s = since(LastInput::Message).with_round_text(RoundText::Notes(INCIDENT_NOTE));
    assert!(matches!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::PointsAboveAtNothing { .. })
    ));
}

#[test]
fn a_persisted_round_is_not_counted_twice() {
    let s = SinceLastInput {
        words: format!("Earlier. {INCIDENT_NOTE}"),
        ..since(LastInput::Message)
    }
    .with_round_text(RoundText::Notes(INCIDENT_NOTE));
    assert_eq!(s.words, format!("Earlier. {INCIDENT_NOTE}"));

    // Only a prefix of the round reached the events: the overlap counts once.
    let (saved, _) = INCIDENT_NOTE.split_at(100);
    let partly = SinceLastInput {
        words: format!("Earlier. {saved}"),
        ..since(LastInput::Message)
    }
    .with_round_text(RoundText::Notes(INCIDENT_NOTE));
    assert_eq!(partly.words, format!("Earlier. {INCIDENT_NOTE}"));

    // Nothing of the round is saved yet: it is appended whole.
    let unsaved = SinceLastInput {
        words: "Earlier. ".to_string(),
        ..since(LastInput::Message)
    }
    .with_round_text(RoundText::Notes(INCIDENT_NOTE));
    assert_eq!(unsaved.words, format!("Earlier. {INCIDENT_NOTE}"));

    let blank = since(LastInput::Typed).with_round_text(RoundText::Notes(" \n"));
    assert!(!blank.spoke);
    assert!(blank.words.is_empty());
    assert!(!blank.round_was_notes);
}

// ---------------------------------------------------------------------------
// A typed reply answered only by a progress note
// ---------------------------------------------------------------------------

/// The note the chat agent showed after the user asked where the promised
/// message was. The message itself stayed in its reasoning.
const REFUND_NOTE: &str = "You're right, I hadn't actually written it. Here it is now: steps \
     to cancel your plan and request a refund via support, plus two caveats.";

/// The card that followed it, which points nowhere.
const TAP_WHEN_SENT: &str = "Tap one when you've sent it, and I'll update the notes to match.";

#[test]
fn a_typed_reply_answered_only_by_a_note_is_refused() {
    let s = since(LastInput::Typed).with_round_text(RoundText::Notes(REFUND_NOTE));
    assert_eq!(
        should_refuse(&s, TAP_WHEN_SENT),
        Some(Refusal::OnlyNotes {
            words: REFUND_NOTE.to_string()
        })
    );
}

#[test]
fn a_typed_reply_answered_by_a_real_reply_passes_at_any_length() {
    for reply in ["Done. Next question:", REFUND_NOTE] {
        let s = since(LastInput::Typed).with_round_text(RoundText::Reply(reply));
        assert_eq!(should_refuse(&s, TAP_WHEN_SENT), None, "{reply}");
    }
}

#[test]
fn a_note_after_anything_but_a_typed_reply_passes() {
    for input in [LastInput::Picked, LastInput::Message, LastInput::None] {
        let s = since(input).with_round_text(RoundText::Notes(REFUND_NOTE));
        assert_eq!(should_refuse(&s, TAP_WHEN_SENT), None, "{input:?}");
    }
}

#[test]
fn a_note_after_a_long_earlier_reply_passes() {
    // An earlier round since the input wrote more than any note holds, so it
    // was a reply.
    let s = SinceLastInput {
        words: "An earlier reply with the whole message. ".repeat(20),
        ..since(LastInput::Typed)
    }
    .with_round_text(RoundText::Notes(REFUND_NOTE));
    assert_eq!(should_refuse(&s, TAP_WHEN_SENT), None);
}

#[test]
fn a_coding_agent_short_note_after_a_typed_reply_passes() {
    // No round text: its notes and replies look the same, so only the old
    // rules apply.
    assert_eq!(
        should_refuse(&after_words(REFUND_NOTE), TAP_WHEN_SENT),
        None
    );
}

#[test]
fn only_notes_is_refused_once_per_input() {
    let s = SinceLastInput {
        refused: true,
        ..since(LastInput::Typed)
    }
    .with_round_text(RoundText::Notes(REFUND_NOTE));
    assert_eq!(should_refuse(&s, TAP_WHEN_SENT), None);
}

#[test]
fn only_notes_comes_before_pointing_above() {
    let s = since(LastInput::Typed).with_round_text(RoundText::Notes(REFUND_NOTE));
    assert!(matches!(
        should_refuse(&s, POINTS_ABOVE),
        Some(Refusal::OnlyNotes { .. })
    ));
}

#[test]
fn the_only_notes_refusal_quotes_the_note_and_offers_a_reply() {
    let text = Refusal::OnlyNotes {
        words: format!(" {REFUND_NOTE}\n"),
    }
    .text();
    assert!(text.starts_with(REFUSAL_MARKER));
    assert!(text.contains(&format!("\"{REFUND_NOTE}\"")));
    assert!(text.contains("end your turn with no card"));
    assert!(text.contains("on the card itself"));
    assert!(text.contains("unchanged"));
    assert!(!text.contains('\u{2014}'));
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
        spoke: true,
        words: words.to_string(),
        saved_pictures: vec![MOCKUP.to_string()],
        ..since(LastInput::Message)
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
        Some(Refusal::PictureNotShown {
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
                Some(Refusal::PictureNotShown { .. })
            ),
            "{words}"
        );
    }
}

#[test]
fn a_picture_refusal_is_sent_once_per_input() {
    let s = SinceLastInput {
        refused: true,
        ..saved_mockup(INCIDENT_SUMMARY)
    };
    assert_eq!(should_refuse(&s, ""), None);
}

/// The one refusal per input goes to the picture. Spent on owing words, it
/// let the retry through without the picture: the second incident.
#[test]
fn a_missing_picture_comes_before_owing_words() {
    for s in [
        SinceLastInput {
            unreported_tool_calls: 3,
            ..saved_mockup(INCIDENT_SUMMARY)
        },
        SinceLastInput {
            last_input: LastInput::Typed,
            spoke: false,
            words: String::new(),
            ..saved_mockup("")
        },
    ] {
        assert_eq!(
            should_refuse(&s, ""),
            Some(Refusal::PictureNotShown {
                paths: vec![MOCKUP.to_string()]
            })
        );
    }
}

#[test]
fn a_missing_picture_comes_before_pointing_above() {
    assert!(matches!(
        should_refuse(&saved_mockup(INCIDENT_SUMMARY), POINTS_ABOVE),
        Some(Refusal::PictureNotShown { .. })
    ));
}

#[test]
fn the_picture_refusal_names_the_path_and_the_card() {
    let text = Refusal::PictureNotShown {
        paths: vec![MOCKUP.to_string(), "artifacts/b.png".to_string()],
    }
    .text();
    assert!(text.starts_with(REFUSAL_MARKER));
    assert!(text.contains(&format!("`{MOCKUP}`, `artifacts/b.png`")));
    assert!(text.contains("short summary"));
    assert!(text.contains("ON the card"));
    assert!(text.contains("`preview` if your tool has one"));
    assert!(text.contains("its own picture of only that option"));
    // It outranks owing words, so it carries both of that refusal's asks.
    assert!(text.contains("answer it in plain prose"));
    assert!(text.contains("say what you found"));
    assert!(text.contains("unchanged"));
    assert!(!text.contains('\u{2014}'));
}

#[test]
fn only_pictures_saved_as_artifacts_count() {
    for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.svg", "a.webp"] {
        let path = format!("artifacts/design/{name}");
        assert!(is_shown_picture(&path), "{path}");
    }
    for path in [
        "artifacts/notes.md",
        "artifacts/png-notes.md",
        "artifacts/png",
        "apps/habit-tracker/icon.png",
        "knowhow/diagram.svg",
    ] {
        assert!(!is_shown_picture(path), "{path}");
    }
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
        refuse_card(&pool, t, "toolu_next", &json!([]), None)
            .await
            .is_some(),
        "blank text is not speaking"
    );

    // The refusal comes back as a tool result, so the re-sent card passes.
    insert(
        &pool,
        t,
        "CodingAgentToolResult",
        json!({ "name": "AskUserQuestion", "result": CARD_REFUSAL }),
    )
    .await;
    assert!(
        refuse_card(&pool, t, "toolu_again", &json!([]), None)
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
    assert!(refuse_card(&pool, c, "toolu_chat", &json!([]), None)
        .await
        .is_some());
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
        refuse_card(&pool, c, "toolu_chat", &json!([]), None)
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
        refuse_card(&pool, c, "toolu_chat", &json!([]), None)
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
    assert!(refuse_card(&pool, p, "toolu_chain", &json!([]), None)
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
    cc_tool(&pool, t, "Read").await;
    answered(&pool, t, json!({ "kind": "FreeText", "text": "why?" })).await;
    assert!(refuse_card(&pool, t, "toolu_shown", &json!([]), None)
        .await
        .is_none());

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

    let refusal = refuse_card(&pool, t, "toolu_above", &incident_card(), None).await;
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
        refuse_card(&pool, t, "toolu_above", &incident_card(), None)
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
async fn the_query_finds_the_pictures_this_thread_saved_since_the_input() {
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
    data_file_written(&pool, t, "artifacts/mockups/notes.md").await;
    data_file_written(&pool, Uuid::new_v4(), "artifacts/another-thread.png").await;
    cc_text(&pool, t, INCIDENT_SUMMARY).await;

    let refusal = refuse_card(&pool, t, "toolu_card", &incident_approval_card(None), None).await;
    assert_eq!(
        refusal,
        Some(Refusal::PictureNotShown {
            paths: vec![MOCKUP.to_string()]
        }),
        "only this thread's picture since the input counts"
    );

    let preview = format!("![Mockup]({MOCKUP})");
    assert!(
        refuse_card(
            &pool,
            t,
            "toolu_card",
            &incident_approval_card(Some(&preview)),
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
        refuse_card(&pool, t, "toolu_card", &incident_approval_card(None), None)
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

    let refusal = refuse_card(&pool, t, "toolu_card", &incident_approval_card(None), None).await;
    assert_eq!(
        refusal,
        Some(Refusal::PictureNotShown {
            paths: vec![MOCKUP.to_string()]
        }),
        "the only refusal this input gets must ask for the picture"
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
    assert!(refuse_card(&pool, t, "toolu_x", &json!([]), None)
        .await
        .is_none());
    teardown_test_db(&db).await;
}
