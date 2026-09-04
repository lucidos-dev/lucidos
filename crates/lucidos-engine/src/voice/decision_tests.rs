//! What is open on a thread, and the ids that settle it.
//!
//! The reads run against a real database. The choice sets are pure, so they are
//! stated without one.

use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::ThreadEvent;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

fn option(id: &str, label: &str, description: Option<&str>) -> QuestionOption {
    QuestionOption {
        id: id.to_string(),
        label: label.to_string(),
        description: description.map(str::to_string),
    }
}

fn two_options() -> Vec<QuestionOption> {
    vec![
        option("opt-0", "Run the tail now", Some("Chunks 25-33")),
        option("opt-1", "Leave it for tonight", None),
    ]
}

fn labels(decision: &OpenDecision) -> Vec<&str> {
    decision.choices.iter().map(|c| c.label.as_str()).collect()
}

fn ids(open: &[OpenDecision]) -> Vec<String> {
    open.iter()
        .flat_map(|d| d.choices.iter().map(|c| c.id.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// The choice sets
// ---------------------------------------------------------------------------

/// The card's own options, plus the one the screen's prompt textarea stands in
/// for. A free-text answer needs a choice, because the tool takes only an id.
#[test]
fn a_question_offers_its_options_and_one_for_their_own_words() {
    let decision = OpenDecision::question("toolu_q0", "Do something now?", &two_options(), false);
    assert_eq!(
        labels(&decision),
        vec![
            "Run the tail now",
            "Leave it for tonight",
            THEIR_WORDS_LABEL
        ]
    );
    assert_eq!(decision.kind, DecisionKind::Question);
    assert_eq!(decision.prompt, "Do something now?");
}

/// A question with no options is answered by the caller's own words, and by
/// nothing else. One choice, not zero: a decision nothing can settle is not
/// worth reading out.
#[test]
fn a_free_text_question_offers_only_their_own_words() {
    let decision = OpenDecision::question("toolu_q1", "What should I call it?", &[], false);
    assert_eq!(labels(&decision), vec![THEIR_WORDS_LABEL]);
}

/// Every choice id starts with its decision's, so an id says which card it
/// belongs to before anything looks it up.
#[test]
fn every_choice_id_is_rooted_in_its_decisions_id() {
    let decision = OpenDecision::question("toolu_q0", "Now?", &two_options(), false);
    assert_eq!(decision.id, "question:toolu_q0");
    for choice in &decision.choices {
        assert!(choice.id.starts_with("question:toolu_q0#"), "{}", choice.id);
    }
}

/// The option's OWN id is what the answer carries, and the choice id numbers
/// by position. An agent writing `#` into an option id cannot then collide two
/// choices of one card.
#[test]
fn a_choice_id_numbers_by_position_and_the_answer_carries_the_real_option_id() {
    let awkward = vec![option("weird#id", "Ship it", None)];
    let decision = OpenDecision::question("toolu_q0", "Ready?", &awkward, false);
    assert_eq!(decision.choices[0].id, "question:toolu_q0#opt0");
    assert_eq!(
        decision.choices[0].act,
        Act::Answer {
            tool_use_id: "toolu_q0".to_string(),
            answer: AnswerKind::Selected {
                option_id: "weird#id".to_string()
            },
        }
    );
}

/// A multi-select card is answered one option at a time out loud, because the
/// tool takes one id. Its answer still carries the shape that card accepts.
#[test]
fn a_multi_select_option_answers_in_the_shape_that_card_takes() {
    let decision = OpenDecision::question("toolu_q0", "Which?", &two_options(), true);
    assert_eq!(
        decision.choices[0].act,
        Act::Answer {
            tool_use_id: "toolu_q0".to_string(),
            answer: AnswerKind::MultiSelected {
                option_ids: vec!["opt-0".to_string()],
                text: None,
            },
        }
    );
}

/// Picking more than one is what the caller's own words are for, and the
/// choice says so. Nothing promises a second id.
#[test]
fn picking_more_than_one_is_routed_through_their_own_words() {
    let decision = OpenDecision::question("toolu_q0", "Which?", &two_options(), true);
    let their_words = decision.choices.last().expect("the last choice");
    assert_eq!(
        their_words.act,
        Act::TheirWords {
            tool_use_id: "toolu_q0".to_string()
        }
    );
    let detail = their_words.description.as_deref().unwrap_or_default();
    assert!(detail.contains("more than one"), "{}", detail);
}

/// The question is never cut, because a truncated question is a different
/// question. An option's description is, like everything else read aloud.
#[test]
fn the_question_survives_whole_and_a_long_description_does_not() {
    let long = "x".repeat(READ_ALOUD_CHARS * 2);
    let decision = OpenDecision::question(
        "toolu_q0",
        &long,
        &[option("opt-0", "Go", Some(&long))],
        false,
    );
    assert_eq!(decision.prompt, long);
    assert!(decision.choices[0]
        .description
        .as_deref()
        .expect("a description")
        .ends_with('…'));
}

/// Decision 7 of the plan, in each of the three lanes: Allow once, Allow for
/// this thread, Deny. Both Always-allow scopes stay on screen, because they
/// widen what every future agent session may do.
#[test]
fn a_permission_offers_three_scopes_and_never_an_always_allow() {
    let lanes = [
        OpenDecision::command_permission("r1", "run_bash", "rm -rf build", "Deletes files."),
        OpenDecision::mcp_permission("r2", "example-server", "Example", "post_message", "{}"),
        OpenDecision::coding_agent_permission(
            "r3",
            "Bash",
            &serde_json::json!({ "command": "git push" }),
            "Bash git push",
        ),
    ];
    for decision in lanes {
        assert_eq!(
            labels(&decision),
            vec!["Allow once", "Allow for this thread", "Deny"],
            "{:?}",
            decision.kind
        );
        for choice in &decision.choices {
            assert!(
                !choice.label.to_lowercase().contains("always"),
                "{:?} offered {}",
                decision.kind,
                choice.label
            );
        }
    }
}

/// The scopes a caller reaches are exactly what the engine will accept.
#[test]
fn the_permission_scopes_are_allow_once_session_and_deny() {
    let decision = OpenDecision::command_permission("r1", "run_bash", "rm -rf b", "Deletes.");
    let acts: Vec<&Act> = decision.choices.iter().map(|c| &c.act).collect();
    assert_eq!(
        acts,
        vec![
            &Act::Permit {
                request_id: "r1".to_string(),
                allowed: true,
                scope: None
            },
            &Act::Permit {
                request_id: "r1".to_string(),
                allowed: true,
                scope: Some(AllowScope::Session)
            },
            &Act::Permit {
                request_id: "r1".to_string(),
                allowed: false,
                scope: None
            },
        ]
    );
}

/// A card whose "Allow for this thread" would record nothing does not offer
/// it. The screen hides that button for the same reason, and a caller cannot
/// see which buttons are there. `file_change` is the case: every one of its
/// approvals renders its own card.
#[test]
fn a_card_that_cannot_be_granted_for_the_thread_does_not_offer_it() {
    let decision = OpenDecision::coding_agent_permission(
        "r3",
        "file_change",
        &serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
        "file_change /tmp/x",
    );
    assert_eq!(labels(&decision), vec!["Allow once", "Deny"]);
}

/// Two lanes cannot issue one string, even for the same underlying id. The
/// lane is the first half of every id.
#[test]
fn two_lanes_never_issue_the_same_id() {
    let command = OpenDecision::command_permission("same", "run_bash", "ls", "Runs.");
    let mcp = OpenDecision::mcp_permission("same", "s", "S", "t", "{}");
    assert_ne!(command.id, mcp.id);
    for a in &command.choices {
        for b in &mcp.choices {
            assert_ne!(a.id, b.id);
        }
    }
}

// ---------------------------------------------------------------------------
// Reading a thread
// ---------------------------------------------------------------------------

async fn a_chat_thread(pool: &PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("create the thread");
    thread_id
}

/// A coding-agent thread, the only kind its permission lane fires on.
///
/// The lifecycle validator refuses a `CodingAgentPermissionRequest` anywhere
/// else. A call is refused on one of these at admission (ADR 0165), so voice
/// meets that lane only after a destination flip mid-call.
async fn a_coding_agent_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: "claude-code/test".to_string(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: crate::engine::thread_events::EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .expect("SessionStarted emit")
    .expect("SessionStarted persisted");
    thread_id
}

fn asks_the_caller(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: tool_use_id.to_string(),
        cc_session_id: String::new(),
        question: "Do something now?".to_string(),
        options: two_options(),
        worktree_path: None,
        multi_select: false,
    }
}

fn asks_to_run(request_id: &str) -> ThreadEvent {
    ThreadEvent::CommandPermissionRequested {
        request_id: request_id.to_string(),
        tool_use_id: "toolu_b0".to_string(),
        tool_name: "run_bash".to_string(),
        command: "gh release delete v1".to_string(),
        summary: "Deletes a published release.".to_string(),
    }
}

fn asks_to_call_mcp(request_id: &str) -> ThreadEvent {
    ThreadEvent::McpPermissionRequested {
        request_id: request_id.to_string(),
        tool_use_id: "toolu_m0".to_string(),
        server_id: "example-server".to_string(),
        server_name: "Example Server".to_string(),
        tool_name: "post_message".to_string(),
        arguments_summary: "{\"channel\":\"general\"}".to_string(),
    }
}

fn asks_the_coding_agent(request_id: &str) -> ThreadEvent {
    ThreadEvent::CodingAgentPermissionRequest {
        request_id: request_id.to_string(),
        tool_use_id: "toolu_c0".to_string(),
        tool_name: "Bash".to_string(),
        input: serde_json::json!({ "command": "git push" }),
        summary: "Bash git push".to_string(),
    }
}

fn a_command_answer(request_id: &str) -> ThreadEvent {
    ThreadEvent::CommandPermissionResolved {
        request_id: request_id.to_string(),
        allowed: true,
        reason: None,
        persist_scope: None,
    }
}

/// A turn on this thread, so the events below are legal to emit.
async fn a_turn_starts(bus: &EventBus, thread_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "clean up the old releases".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
}

/// The chat surfaces at once, in lane order, each carrying the id that settles
/// it. A thread rarely holds them all, and the reader must not care.
#[tokio::test]
async fn the_reader_covers_every_chat_lane_at_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_the_caller("toolu_q0")).await;
    seed_thread_event(&bus, thread_id, asks_to_run("req-cmd")).await;
    seed_thread_event(&bus, thread_id, asks_to_call_mcp("req-mcp")).await;

    let open = open_on(&pool, thread_id).await;
    assert_eq!(
        open.iter().map(|d| d.kind).collect::<Vec<_>>(),
        vec![
            DecisionKind::Question,
            DecisionKind::CommandPermission,
            DecisionKind::McpPermission,
        ]
    );
    assert_eq!(
        open.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["question:toolu_q0", "command:req-cmd", "mcp:req-mcp"]
    );

    teardown_test_db(&db_name).await;
}

/// The fourth lane, on the only thread kind it fires on.
#[tokio::test]
async fn the_reader_covers_the_coding_agent_lane_too() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_coding_agent_thread(&bus).await;

    seed_thread_event(&bus, thread_id, asks_the_coding_agent("req-agent")).await;

    let open = open_on(&pool, thread_id).await;
    assert_eq!(
        open.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["agent:req-agent"]
    );
    assert_eq!(open[0].kind, DecisionKind::CodingAgentPermission);
    assert!(doer_is_parked(&pool, thread_id).await);

    teardown_test_db(&db_name).await;
}

/// A resolved card is not waiting on anybody, so it is not read.
#[tokio::test]
async fn a_resolved_card_stops_being_open() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_to_run("req-cmd")).await;
    assert_eq!(open_on(&pool, thread_id).await.len(), 1);

    seed_thread_event(&bus, thread_id, a_command_answer("req-cmd")).await;
    assert!(open_on(&pool, thread_id).await.is_empty());

    teardown_test_db(&db_name).await;
}

/// The invariant the whole id scheme exists for. Read across a settled card and
/// no id repeats. An id the talker still holds can then never resolve a
/// decision it was not issued for.
#[tokio::test]
async fn no_id_is_ever_reused_across_a_settled_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_to_run("req-first")).await;
    let first = ids(&open_on(&pool, thread_id).await);
    assert!(!first.is_empty());

    seed_thread_event(&bus, thread_id, a_command_answer("req-first")).await;
    seed_thread_event(&bus, thread_id, asks_to_run("req-second")).await;
    let second = ids(&open_on(&pool, thread_id).await);
    assert!(!second.is_empty());

    for id in &second {
        assert!(
            !first.contains(id),
            "{} was issued for both cards, so the first card's answer would \
             land on the second",
            id
        );
    }

    teardown_test_db(&db_name).await;
}

/// Reading the same live card twice issues the same ids. A talker holding one
/// from the opening block can still use it a minute later.
#[tokio::test]
async fn reading_one_live_card_twice_issues_the_same_ids() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_the_caller("toolu_q0")).await;
    assert_eq!(
        ids(&open_on(&pool, thread_id).await),
        ids(&open_on(&pool, thread_id).await)
    );

    teardown_test_db(&db_name).await;
}

/// Both refusals, in one rule. An id nobody issued is not among what is open,
/// and neither is one whose card has settled.
#[tokio::test]
async fn an_id_the_engine_never_issued_and_a_spent_one_both_find_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_to_run("req-cmd")).await;
    let open = open_on(&pool, thread_id).await;
    let issued = open[0].choices[0].id.clone();
    assert!(choice_in(&open, &issued).is_some());

    // A string the talker could have composed.
    assert!(choice_in(&open, "command:req-cmd#always").is_none());
    assert!(choice_in(&open, "allow").is_none());

    // The same id, once the card has settled.
    seed_thread_event(&bus, thread_id, a_command_answer("req-cmd")).await;
    let after = open_on(&pool, thread_id).await;
    assert!(choice_in(&after, &issued).is_none());

    teardown_test_db(&db_name).await;
}

/// A question with no `tool_use_id` cannot be answered by anybody, so reading
/// it aloud would offer the caller a dead end.
#[tokio::test]
async fn a_question_nothing_could_answer_is_not_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_the_caller("")).await;
    assert!(open_on(&pool, thread_id).await.is_empty());

    teardown_test_db(&db_name).await;
}

/// The refusal's own question, in one read. Every lane parks the doer, and a
/// thread with nothing open does not.
#[tokio::test]
async fn a_thread_is_parked_by_any_lane_and_by_nothing_else() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let quiet = a_chat_thread(&pool).await;
    a_turn_starts(&bus, quiet).await;
    assert!(!doer_is_parked(&pool, quiet).await);

    // The coding-agent lane has its own case above: its event is legal on a
    // coding-agent thread and nowhere else.
    for (index, card) in [
        asks_the_caller("toolu_q0"),
        asks_to_run("req-cmd"),
        asks_to_call_mcp("req-mcp"),
    ]
    .into_iter()
    .enumerate()
    {
        let thread_id = a_chat_thread(&pool).await;
        a_turn_starts(&bus, thread_id).await;
        assert!(!doer_is_parked(&pool, thread_id).await, "lane {}", index);
        seed_thread_event(&bus, thread_id, card).await;
        assert!(doer_is_parked(&pool, thread_id).await, "lane {}", index);
    }

    teardown_test_db(&db_name).await;
}

/// A card answered on screen frees the doer, so the next delegation runs.
#[tokio::test]
async fn answering_the_card_unparks_the_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_to_run("req-cmd")).await;
    assert!(doer_is_parked(&pool, thread_id).await);
    seed_thread_event(&bus, thread_id, a_command_answer("req-cmd")).await;
    assert!(!doer_is_parked(&pool, thread_id).await);

    teardown_test_db(&db_name).await;
}

/// Nothing on another thread reaches this one. The reader takes a thread id and
/// reads only that thread.
#[tokio::test]
async fn another_threads_card_is_not_this_threads_decision() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mine = a_chat_thread(&pool).await;
    let theirs = a_chat_thread(&pool).await;
    a_turn_starts(&bus, mine).await;
    a_turn_starts(&bus, theirs).await;

    seed_thread_event(&bus, theirs, asks_to_run("req-cmd")).await;

    assert!(open_on(&pool, mine).await.is_empty());
    assert!(!doer_is_parked(&pool, mine).await);
    assert_eq!(open_on(&pool, theirs).await.len(), 1);

    teardown_test_db(&db_name).await;
}
