use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

fn since(last_input: LastInput) -> SinceLastInput {
    SinceLastInput {
        last_input,
        spoke: false,
        unreported_tool_calls: 0,
        refused: false,
    }
}

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

#[test]
fn a_card_after_a_typed_reply_with_nothing_said_is_refused() {
    // The Claude Code case: the user typed a question into a plan card and got
    // "With that answered: approve the plan?" with no answer and no tool call.
    assert!(should_refuse(since(LastInput::Typed)));
}

#[test]
fn a_card_after_unreported_tool_work_is_refused() {
    // The chat case: asked what the next release holds, the agent ran git log
    // and raised "How do you want to continue?".
    for input in [LastInput::Message, LastInput::Picked, LastInput::None] {
        assert!(should_refuse(SinceLastInput {
            unreported_tool_calls: 2,
            ..since(input)
        }));
    }
}

#[test]
fn a_clarifying_question_with_no_work_passes() {
    assert!(!should_refuse(since(LastInput::Message)));
}

#[test]
fn a_pick_followed_at_once_by_the_next_card_passes() {
    assert!(!should_refuse(since(LastInput::Picked)));
}

#[test]
fn words_after_the_work_pass() {
    assert!(!should_refuse(SinceLastInput {
        spoke: true,
        ..since(LastInput::Typed)
    }));
}

#[test]
fn a_preamble_does_not_report_the_work_after_it() {
    // "Let me check the log", then git log, then a card: the log went unreported.
    assert!(should_refuse(SinceLastInput {
        spoke: true,
        unreported_tool_calls: 1,
        ..since(LastInput::Message)
    }));
}

#[test]
fn one_refusal_per_input() {
    assert!(!should_refuse(SinceLastInput {
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
fn the_refusal_starts_with_its_marker_and_offers_the_escape() {
    assert!(CARD_REFUSAL.starts_with(REFUSAL_MARKER));
    assert!(CARD_REFUSAL.contains("never your tool results"));
    assert!(CARD_REFUSAL.contains("unchanged"));
    assert!(!CARD_REFUSAL.contains('\u{2014}'));
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
        refuse_card(&pool, t, "toolu_next").await,
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
        !refuse_card(&pool, t, "toolu_again").await,
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
    assert!(refuse_card(&pool, c, "toolu_chat").await);
    assert!(
        refuse_coding_agent_card(&pool, c, "toolu_chat").await,
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
        !refuse_card(&pool, c, "toolu_chat").await,
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
        refuse_card(&pool, c, "toolu_chat").await,
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
    assert!(!refuse_card(&pool, p, "toolu_chain").await);

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
    assert!(!refuse_card(&pool, t, "toolu_shown").await);

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
    assert!(!refuse_card(&pool, t, "toolu_x").await);
    teardown_test_db(&db).await;
}
