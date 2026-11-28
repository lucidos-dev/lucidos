use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::legacy::{LegacyCard, LegacyLane, LegacyPick};
use crate::engine::permission::menu::{ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
use crate::engine::thread_events::GrantOffer;
use crate::engine::thread_events::ThreadEvent;
use crate::test_support::{seed_pre_gate_card, seed_thread_event, setup_test_db, teardown_test_db};

async fn is_parked(pool: &PgPool, thread_id: Uuid) -> bool {
    parked_on(pool, thread_id).await.is_some()
}

fn option(id: &str, label: &str, description: Option<&str>) -> QuestionOption {
    QuestionOption {
        id: id.to_string(),
        label: label.to_string(),
        description: description.map(str::to_string),
        preview: None,
        widget: None,
        quiet: false,
        quiet_reply: None,
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

#[test]
fn a_permission_request_is_read_back_but_never_picked() {
    let request = OpenRequest {
        awaited: false,
        words_answer: true,
    };
    let decision = OpenDecision::permission_request(
        "toolu_p0",
        "Continue hardening?",
        &two_options(),
        request,
    );
    let raised = DecisionKind::PermissionRequest {
        awaited: false,
        words_answer: true,
    };
    assert_eq!(decision.kind, raised);
    assert!(!decision.kind.parks_the_doer());
    assert_eq!(
        labels(&decision),
        vec!["Run the tail now", "Leave it for tonight"]
    );
    assert_eq!(decision.their_words_choice(), None);
    let open = [decision];
    let (kind, act) = choice_in(&open, "permission:toolu_p0#opt0").expect("its id is issued");
    assert_eq!(kind, raised);
    assert_eq!(act, Act::Heard);
}

#[test]
fn a_prepared_permission_request_parks_the_doer_and_is_never_picked() {
    let options = vec![
        option("opt-0", ALLOW_ONCE_LABEL, None),
        option("opt-1", DONT_ALLOW_LABEL, None),
    ];
    let request = OpenRequest {
        awaited: true,
        words_answer: false,
    };
    let decision = OpenDecision::permission_request(
        "toolu_p1",
        "Let it create a workspace?",
        &options,
        request,
    );
    assert!(decision.kind.parks_the_doer());
    assert!(decision.choices.iter().all(|c| c.act == Act::Heard));
    assert_eq!(decision.their_words_choice(), None);
}

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

#[test]
fn a_free_text_question_offers_only_their_own_words() {
    let decision = OpenDecision::question("toolu_q1", "What should I call it?", &[], false);
    assert_eq!(labels(&decision), vec![THEIR_WORDS_LABEL]);
}

#[test]
fn every_choice_id_is_rooted_in_its_decisions_id() {
    let decision = OpenDecision::question("toolu_q0", "Now?", &two_options(), false);
    assert_eq!(decision.id, "question:toolu_q0");
    for choice in &decision.choices {
        assert!(choice.id.starts_with("question:toolu_q0#"), "{}", choice.id);
    }
}

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
                image_hashes: vec![],
            },
        }
    );
}

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

#[test]
fn a_permission_offers_every_button_its_card_draws() {
    let lanes = [
        stamped_command("r1", "git push origin main", "Pushes."),
        LegacyCard::mcp("r2", "example-server", "Example", "post_message", "{}"),
        LegacyCard::coding_agent("r3", Some(git_offer()), "Bash git push"),
    ];
    let expected = [
        ["Always allow `Bash(git:*)`", "Always allow `Bash`"],
        [
            "Always allow `Mcp(example-server:post_message)`",
            "Always allow `Mcp(example-server:*)`",
        ],
        ["Always allow `Bash(git:*)`", "Always allow `Bash`"],
    ];
    for (card, [narrow, broad]) in lanes.iter().zip(expected) {
        let decision = OpenDecision::tool_permission(card);
        assert_eq!(
            labels(&decision),
            vec!["Allow once", "Allow for this thread", narrow, broad, "Deny"],
            "{:?}",
            decision.kind
        );
    }
}

#[test]
fn an_always_allow_is_heard_and_never_picked() {
    let decision = OpenDecision::tool_permission(&stamped_command("r1", "git push", "Pushes."));
    let permit = |pick| Act::Permit {
        lane: LegacyLane::Command,
        request_id: "r1".to_string(),
        pick,
    };
    let acts: Vec<&Act> = decision.choices.iter().map(|c| &c.act).collect();
    assert_eq!(
        acts,
        vec![
            &permit(LegacyPick::Once),
            &permit(LegacyPick::Session),
            &Act::Heard,
            &Act::Heard,
            &permit(LegacyPick::Deny),
        ]
    );
    assert_eq!(decision.choices[2].id, "command:r1#always-narrow");
    assert_eq!(decision.their_words_choice(), None);
}

#[test]
fn a_cards_risk_phrase_stays_whole_and_its_command_is_cut() {
    let long = "x".repeat(READ_ALOUD_CHARS * 2);
    let decision =
        OpenDecision::tool_permission(&LegacyCard::command("r1", "run_bash", &long, "Runs."));
    let (head, rest) = decision.prompt.split_once('\n').expect("two lines");
    assert_eq!(head, "Runs.");
    assert!(rest.ends_with('…'), "{rest}");
}

#[test]
fn a_card_that_cannot_be_granted_for_the_thread_does_not_offer_it() {
    let decision = OpenDecision::tool_permission(&LegacyCard::coding_agent(
        "r3",
        Some(GrantOffer::default()),
        "file_change /tmp/x",
    ));
    assert_eq!(labels(&decision), vec!["Allow once", "Deny"]);
}

#[test]
fn two_lanes_never_issue_the_same_id() {
    let command =
        OpenDecision::tool_permission(&LegacyCard::command("same", "run_bash", "ls", "Runs."));
    let mcp = OpenDecision::tool_permission(&LegacyCard::mcp("same", "s", "S", "t", "{}"));
    assert_ne!(command.id, mcp.id);
    for a in &command.choices {
        for b in &mcp.choices {
            assert_ne!(a.id, b.id);
        }
    }
}

async fn a_chat_thread(pool: &PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("create the thread");
    thread_id
}

async fn a_coding_agent_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: "claude-code/test".to_string(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
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
        owner_approval: None,
        permission_request: None,
    }
}

fn asks_to_run(request_id: &str) -> ThreadEvent {
    ThreadEvent::CommandPermissionRequested {
        decision_trace: None,
        request_id: request_id.to_string(),
        tool_use_id: "toolu_b0".to_string(),
        tool_name: "run_bash".to_string(),
        command: "gh release delete v1".to_string(),
        summary: "Deletes a published release.".to_string(),
        grant_head: None,
        grant_offer: None,
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
        grant_offer: Some(git_offer()),
    }
}

fn git_offer() -> GrantOffer {
    GrantOffer {
        deny: None,
        broad: Some("Bash".to_string()),
        narrow: Some("Bash(git:*)".to_string()),
        session: Some("Bash(git:*)".to_string()),
        withheld: None,
    }
}

fn stamped_command(request_id: &str, command: &str, summary: &str) -> LegacyCard {
    let mut card = LegacyCard::command(request_id, "run_bash", command, summary);
    card.offer = git_offer();
    card
}

fn a_command_answer(request_id: &str) -> ThreadEvent {
    ThreadEvent::CommandPermissionResolved {
        standing_deny: None,
        request_id: request_id.to_string(),
        allowed: true,
        reason: None,
        persist_scope: None,
    }
}

async fn a_turn_starts(bus: &EventBus, thread_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "clean up the old releases".to_string(),
            user_image_hashes: vec![],
            device_id: None,
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

#[tokio::test]
async fn the_reader_covers_every_chat_lane_at_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_the_caller("toolu_q0")).await;
    seed_pre_gate_card(&pool, thread_id, asks_to_run("req-cmd")).await;
    seed_pre_gate_card(&pool, thread_id, asks_to_call_mcp("req-mcp")).await;

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
    assert!(is_parked(&pool, thread_id).await);

    teardown_test_db(&db_name).await;
}

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

    assert!(choice_in(&open, "command:req-cmd#always").is_none());
    assert!(choice_in(&open, "allow").is_none());

    seed_thread_event(&bus, thread_id, a_command_answer("req-cmd")).await;
    let after = open_on(&pool, thread_id).await;
    assert!(choice_in(&after, &issued).is_none());

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn a_thread_is_parked_by_any_lane_and_by_nothing_else() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let quiet = a_chat_thread(&pool).await;
    a_turn_starts(&bus, quiet).await;
    assert!(!is_parked(&pool, quiet).await);

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
        assert!(!is_parked(&pool, thread_id).await, "lane {}", index);
        seed_thread_event(&bus, thread_id, card).await;
        assert!(is_parked(&pool, thread_id).await, "lane {}", index);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn answering_the_card_unparks_the_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_chat_thread(&pool).await;
    a_turn_starts(&bus, thread_id).await;

    seed_thread_event(&bus, thread_id, asks_to_run("req-cmd")).await;
    assert!(is_parked(&pool, thread_id).await);
    seed_thread_event(&bus, thread_id, a_command_answer("req-cmd")).await;
    assert!(!is_parked(&pool, thread_id).await);

    teardown_test_db(&db_name).await;
}

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
    assert!(!is_parked(&pool, mine).await);
    assert_eq!(open_on(&pool, theirs).await.len(), 1);

    teardown_test_db(&db_name).await;
}
