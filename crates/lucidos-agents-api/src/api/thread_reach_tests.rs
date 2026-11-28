use super::*;
use crate::core::subprocess_origin::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

const EVERY_VERB: &[Action] = &[
    Action::Archive,
    Action::Cancel,
    Action::Control,
    Action::AskSideQuestion,
    Action::DismissSideQuestion,
    Action::Apply,
    Action::Discard,
    Action::SetAside,
    Action::BringBack,
    Action::Revert,
    Action::AnswerQuestion,
    Action::ResolvePermission,
    Action::Continue,
];

fn owner_buttons() -> impl Iterator<Item = &'static Action> {
    EVERY_VERB.iter().filter(|verb| !verb.answers_a_card())
}

const RETIRED_VERBS: &[Action] = &[
    Action::CreateTopThread,
    Action::DetachChild,
    Action::SpawnSuccessor,
];

const GATED_HANDLERS: &[(&str, &str, &str)] = &[
    ("changes.rs", CHANGES_RS, "apply_change"),
    ("changes.rs", CHANGES_RS, "discard_change"),
    ("changes.rs", CHANGES_RS, "revert_change"),
    ("changes.rs", CHANGES_RS, "apply_all_changes"),
    ("changes.rs", CHANGES_RS, "cancel_apply_all_changes"),
    ("changes.rs", CHANGES_RS, "discard_all_changes"),
    ("changes.rs", CHANGES_RS, "remove_from_landing_lane"),
    ("changes.rs", CHANGES_RS, "cancel_lane_batch"),
    ("chat.rs", CHAT_RS, "cancel_chat"),
    ("chat.rs", CHAT_RS, "chat_settings"),
    ("chat.rs", CHAT_RS, "remove_queued_message"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_stop"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_apply_now"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_discard"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_interrupt"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_control"),
    ("disk_usage.rs", DISK_USAGE_RS, "cleanup_worktree"),
    ("threads/actions.rs", ACTIONS_RS, "answer_thread_question"),
    ("threads/actions.rs", ACTIONS_RS, "continue_thread"),
    ("threads/actions.rs", ACTIONS_RS, "cancel_thread_event_wait"),
    ("threads/archive.rs", ARCHIVE_RS, "archive_thread"),
    ("threads/archive.rs", ARCHIVE_RS, "archive_thread_as_caller"),
    ("engine/tools/mod.rs", TOOLS_RS, "execute_archive_thread"),
    ("threads_compose.rs", THREADS_COMPOSE_RS, "put_compose"),
    ("threads_compose.rs", THREADS_COMPOSE_RS, "delete_thread"),
    ("blobs.rs", BLOBS_RS, "post_blob"),
    (
        "command_checkpoint.rs",
        COMMAND_CHECKPOINT_RS,
        "undo_command_checkpoint",
    ),
    (
        "command_permission.rs",
        COMMAND_PERMISSION_RS,
        "submit_command_consent",
    ),
    (
        "mcp_permission.rs",
        MCP_PERMISSION_RS,
        "submit_mcp_permission_consent",
    ),
    ("mcp.rs", MCP_RS, "submit_mcp_consent"),
    ("changes.rs", CHANGES_RS, "arm_standing_apply"),
    ("changes.rs", CHANGES_RS, "disarm_standing_apply"),
    ("changes.rs", CHANGES_RS, "disarm_all_standing_applies"),
    ("engine/tools/mod.rs", TOOLS_RS, "execute_apply_change"),
    (
        "engine/tools/mod.rs",
        TOOLS_RS,
        "execute_apply_when_settled",
    ),
    (
        "engine/tools/mod.rs",
        TOOLS_RS,
        "execute_apply_as_they_settle",
    ),
    (
        "engine/tools/mod.rs",
        TOOLS_RS,
        "execute_cancel_standing_apply",
    ),
];

const CHANGES_RS: &str = include_str!("changes.rs");
const CHAT_RS: &str = include_str!("chat.rs");
const CLAUDE_CODE_RS: &str = include_str!("claude_code.rs");
const DISK_USAGE_RS: &str = include_str!("disk_usage.rs");
const ACTIONS_RS: &str = include_str!("threads/actions.rs");
const ARCHIVE_RS: &str = include_str!("threads/archive.rs");
const THREADS_COMPOSE_RS: &str = include_str!("threads_compose.rs");
const COMMAND_CHECKPOINT_RS: &str = include_str!("command_checkpoint.rs");
const BLOBS_RS: &str = include_str!("blobs.rs");
const TOOLS_RS: &str = include_str!("../engine/tools/mod.rs");
const COMMAND_PERMISSION_RS: &str = include_str!("command_permission.rs");
const MCP_PERMISSION_RS: &str = include_str!("mcp_permission.rs");
const MCP_RS: &str = include_str!("mcp.rs");
const THREAD_REACH_RS: &str = include_str!("thread_reach.rs");

const GATE_CALLS: &[&str] = &[
    "refuse_without_authority",
    "refuse_thread_without_authority",
    "refuse_change_verb",
    "refuse_batch_change_verb",
    "refuse_tool_without_authority",
    "refuse_permission_answer",
    "authorize_checkpoint_undo",
];

#[test]
fn every_clause_4_route_asks_the_gate() {
    for (path, source, handler) in GATED_HANDLERS {
        let body = handler_body(source, handler)
            .unwrap_or_else(|| panic!("{path}: no handler named {handler}"));
        assert!(
            GATE_CALLS.iter().any(|call| body.contains(call)),
            "{path}: {handler} carries a clause-4 verb and never asks the gate"
        );
    }
    for (source, wrapper, reaches) in [
        (CHANGES_RS, "refuse_change_verb", "refuse_without_authority"),
        (
            CHANGES_RS,
            "refuse_batch_change_verb",
            "refuse_without_authority",
        ),
        (
            TOOLS_RS,
            "refuse_tool_without_authority",
            "refuse_thread_without_authority",
        ),
        (
            THREAD_REACH_RS,
            "refuse_permission_answer",
            "refuse_without_authority",
        ),
        (
            COMMAND_CHECKPOINT_RS,
            "authorize_checkpoint_undo",
            "refuse_without_authority",
        ),
    ] {
        let body =
            handler_body(source, wrapper).unwrap_or_else(|| panic!("no wrapper named {wrapper}"));
        assert!(
            body.contains(reaches),
            "{wrapper} stands in for the gate, so it must reach {reaches}"
        );
    }
    for (source, read_only) in [
        (CHANGES_RS, "list_changes"),
        (TOOLS_RS, "execute_list_changes"),
    ] {
        let body = handler_body(source, read_only).expect("the read handler exists");
        assert!(
            !GATE_CALLS.iter().any(|call| body.contains(call)),
            "the body slicer is over-reading at {read_only}: a read cannot contain the gate"
        );
    }
}

#[test]
fn creating_a_top_thread_asks_no_gate() {
    for (source, handler) in [
        (CHAT_RS, "chat_submit"),
        (
            include_str!("../engine/chat/child_detach.rs"),
            "move_child_to_top_level",
        ),
    ] {
        let body = handler_body(source, handler).expect("the handler exists");
        assert!(
            !GATE_CALLS.iter().any(|call| body.contains(call)),
            "{handler} asks the gate again"
        );
    }
}

fn handler_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let decl = source.find(&format!("async fn {name}("))?;
    let line_start = source[..decl].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..decl]
        .chars()
        .take_while(|c| *c == ' ')
        .collect();
    let rest = &source[line_start..];
    let close = format!("\n{indent}}}\n");
    let end = rest
        .find(&close)
        .map(|i| i + close.len())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

#[test]
fn every_refusal_reads_as_a_sentence() {
    for verb in EVERY_VERB {
        let aimed = ThreadReachError::OutOfReach {
            target: Uuid::nil(),
            verb: *verb,
        }
        .to_string();
        assert!(aimed.contains(verb.word()), "{verb:?}: {aimed}");

        let rootward = ThreadReachError::NoStandingInstruction(*verb).to_string();
        assert!(rootward.contains(verb.act()), "{verb:?}: {rootward}");
        assert!(
            !rootward.contains("this thread"),
            "a root-aimed refusal has no thread to point at: {rootward}"
        );

        let threadless = ThreadReachError::NoCallerThread(*verb).to_string();
        assert!(threadless.contains(verb.act()), "{verb:?}: {threadless}");

        for text in [&aimed, &rootward, &threadless] {
            assert!(
                text.contains("workspace owner"),
                "every refusal names the authority: {text}"
            );
            assert!(
                !text.contains("parent thread"),
                "and never a parent the caller may not have: {text}"
            );
        }
    }
}

fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    agent_headers_for_fire(thread_id, None)
}

fn agent_headers_for_fire(thread_id: Option<Uuid>, emitting_trigger_id: Option<&str>) -> HeaderMap {
    init_agent_origin_secret("thread-reach-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, emitting_trigger_id)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

fn device_headers() -> HeaderMap {
    crate::api::gateway_stamp::test_support::stamped_device_headers("device-abc")
}

fn unattested_headers() -> Vec<HeaderMap> {
    let local = crate::api::local_auth::publish_test_local_token();
    let mut typed = HeaderMap::new();
    typed.insert(
        crate::core::subprocess_origin::HEADER_DEVICE_ID,
        "device-abc".parse().unwrap(),
    );
    let mut token = HeaderMap::new();
    token.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        local.parse().unwrap(),
    );
    vec![typed, token, HeaderMap::new()]
}

async fn seed_thread(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>) {
    seed_thread_opened_by(bus, thread_id, parent, None).await;
}

async fn seed_thread_opened_by(
    bus: &EventBus,
    thread_id: Uuid,
    parent: Option<Uuid>,
    origin: Option<MessageOrigin>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

fn bus_for(pool: &PgPool) -> EventBus {
    EventBus::new(pool.clone()).0
}

#[tokio::test]
async fn saved_draft_submission_needs_owner_authority_even_in_own_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    for target in [f.root, f.child, f.grandchild, f.sibling] {
        assert_eq!(
            authority_held(&pool, Some(f.root), None, Some(target), Action::SubmitDraft)
                .await
                .unwrap(),
            None,
            "a relationship must not authorize sending saved owner content"
        );
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn saved_draft_submission_accepts_a_standing_owner_instruction() {
    let (pool, db_name) = setup_test_db().await;
    let bus = bus_for(&pool);
    let caller = Uuid::new_v4();
    seed_thread_opened_by(&bus, caller, None, Some(owner_device())).await;
    for target in [caller, Uuid::new_v4()] {
        assert_eq!(
            authority_held(&pool, Some(caller), None, Some(target), Action::SubmitDraft)
                .await
                .unwrap(),
            Some(HeldAuthority::StandingInstruction)
        );
    }
    assert!(Action::SubmitDraft.proposable());
    assert_eq!(
        serde_json::to_value(Action::SubmitDraft).unwrap(),
        "submit-draft"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn owner_device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "device-abc".into(),
    }
}

struct Family {
    root: Uuid,
    child: Uuid,
    sibling: Uuid,
    grandchild: Uuid,
}

async fn family(pool: &PgPool) -> Family {
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = Uuid::new_v4();
    let child = Uuid::new_v4();
    let sibling = Uuid::new_v4();
    let grandchild = Uuid::new_v4();
    seed_thread(&bus, root, None).await;
    seed_thread(&bus, child, Some(root)).await;
    seed_thread(&bus, sibling, Some(root)).await;
    seed_thread(&bus, grandchild, Some(child)).await;
    Family {
        root,
        child,
        sibling,
        grandchild,
    }
}

#[tokio::test]
async fn a_thread_reaches_itself() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    assert!(authorize_thread_reach(&pool, f.child, f.child)
        .await
        .unwrap());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_reaches_its_child_and_its_grandchild() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    assert!(
        authorize_thread_reach(&pool, f.root, f.child)
            .await
            .unwrap(),
        "a direct child is in reach"
    );
    assert!(
        authorize_thread_reach(&pool, f.root, f.grandchild)
            .await
            .unwrap(),
        "a grandchild is in reach too, because the cascade reaches it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_cannot_reach_its_sibling() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    assert!(!authorize_thread_reach(&pool, f.child, f.sibling)
        .await
        .unwrap());

    let err = refuse_without_authority(
        &bus_for(&pool),
        &agent_headers(Some(f.child)),
        Some(f.sibling),
        Action::Archive,
    )
    .await
    .expect_err("a sibling is out of reach");
    assert_eq!(
        err,
        ThreadReachError::OutOfReach {
            target: f.sibling,
            verb: Action::Archive
        }
    );
    assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    let msg = err.to_string();
    assert!(msg.contains("archive"), "names the verb tried: {msg}");
    assert!(
        msg.contains("workspace owner"),
        "names the authority: {msg}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_cannot_reach_its_own_parent_or_grandparent() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for (caller, target) in [(f.child, f.root), (f.grandchild, f.root)] {
        let err = refuse_without_authority(
            &bus_for(&pool),
            &agent_headers(Some(caller)),
            Some(target),
            Action::Cancel,
        )
        .await
        .expect_err("an ancestor is out of reach");
        assert_eq!(
            err,
            ThreadReachError::OutOfReach {
                target,
                verb: Action::Cancel
            }
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_reaches_neither_an_unrelated_thread_nor_a_missing_one() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let unrelated = Uuid::new_v4();
    seed_thread(&bus, unrelated, None).await;
    let missing = Uuid::new_v4();

    for target in [unrelated, missing] {
        let err = refuse_without_authority(
            &bus_for(&pool),
            &agent_headers(Some(f.root)),
            Some(target),
            Action::Archive,
        )
        .await
        .expect_err("out of reach");
        assert_eq!(err.reason(), "out_of_reach");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_user_device_reaches_every_thread_exactly_as_before() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for verb in EVERY_VERB {
        for target in [Some(f.root), Some(f.sibling), Some(f.grandchild), None] {
            assert!(
                refuse_without_authority(&bus_for(&pool), &device_headers(), target, *verb)
                    .await
                    .is_ok(),
                "the owner's device keeps the reach it has always had"
            );
            let unattested =
                refuse_without_authority(&bus_for(&pool), &HeaderMap::new(), target, *verb).await;
            let expected = match verb {
                Action::Apply | Action::Discard | Action::Revert => {
                    Err(ThreadReachError::Unattested(UnattestedOutcome::Refused))
                }
                Action::AnswerQuestion | Action::ResolvePermission => {
                    Err(ThreadReachError::Unattested(UnattestedOutcome::CardAnswer))
                }
                _ => Ok(()),
            };
            assert_eq!(unattested, expected, "{verb:?}");
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_tokened_agent_is_bound_to_its_own_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.child));

    for target in [f.child, f.grandchild] {
        assert!(
            refuse_without_authority(&bus_for(&pool), &headers, Some(target), Action::Archive)
                .await
                .is_ok()
        );
    }
    for target in [f.root, f.sibling] {
        let err =
            refuse_without_authority(&bus_for(&pool), &headers, Some(target), Action::Archive)
                .await
                .expect_err("outside its own subtree");
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn every_verb_refuses_outside_the_subtree_with_no_standing_instruction() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.child));

    for verb in EVERY_VERB {
        let Err(err) =
            refuse_without_authority(&bus_for(&pool), &headers, Some(f.sibling), *verb).await
        else {
            panic!("{verb:?} must refuse a sibling");
        };
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN, "{verb:?}");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn every_verb_reaches_the_caller_s_own_subtree_with_no_owner_turn() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.root));

    for verb in owner_buttons() {
        for target in [f.root, f.child, f.grandchild] {
            assert!(
                refuse_without_authority(&bus_for(&pool), &headers, Some(target), *verb)
                    .await
                    .is_ok(),
                "{verb:?} inside the caller's own subtree needs nobody's instruction"
            );
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_retired_verb_passes_with_no_authority() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for verb in RETIRED_VERBS {
        assert!(!verb.needs_authority(), "{verb:?}");
        for caller in [Some(f.child), None] {
            for target in [None, Some(f.child), Some(f.sibling)] {
                assert_eq!(
                    authority_held(&pool, caller, None, target, *verb)
                        .await
                        .unwrap(),
                    Some(HeldAuthority::NoneNeeded),
                    "{verb:?} from {caller:?} at {target:?}"
                );
                assert!(
                    refuse_without_authority(
                        &bus_for(&pool),
                        &agent_headers(caller),
                        target,
                        *verb
                    )
                    .await
                    .is_ok(),
                    "{verb:?} from {caller:?} at {target:?}"
                );
            }
        }
    }
    assert!(EVERY_VERB.iter().all(|verb| verb.needs_authority()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_turn_the_owner_opened_reaches_past_the_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let orchestrator = Uuid::new_v4();
    seed_thread_opened_by(&bus, orchestrator, None, Some(owner_device())).await;
    let headers = agent_headers(Some(orchestrator));

    for verb in owner_buttons() {
        assert!(
            refuse_without_authority(&bus_for(&pool), &headers, Some(f.sibling), *verb)
                .await
                .is_ok(),
            "{verb:?} rides the owner's standing instruction"
        );
    }
    assert!(
        refuse_without_authority(&bus_for(&pool), &headers, None, Action::Apply)
            .await
            .is_ok(),
        "so does an act aimed at the workspace, which is nobody's subtree"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_trigger_fire_reaches_its_own_subtree_and_wider() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let trigger_thread = Uuid::new_v4();
    let its_child = Uuid::new_v4();
    bus.emit(BusEvent::System(
        crate::engine::event_bus::SystemEvent::TriggerCreated {
            trigger_id: "nightly".into(),
            payload: serde_json::json!({ "trigger_id": "nightly" }),
            actor: Some(owner_device()),
        },
    ))
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: trigger_thread,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "nightly".into(),
            trigger_name: Some("Nightly release".into()),
            prompt: None,
            invocation: None,
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
            continue_key: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    seed_thread(&bus, its_child, Some(trigger_thread)).await;
    let headers = agent_headers(Some(trigger_thread));

    assert!(
        refuse_without_authority(&bus_for(&pool), &headers, Some(its_child), Action::Apply)
            .await
            .is_ok(),
        "its own subtree, which clause 3 already covered"
    );
    assert!(
        refuse_without_authority(&bus_for(&pool), &headers, Some(f.sibling), Action::Apply)
            .await
            .is_ok(),
        "and wider, on the firing the owner authorized"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_script_trigger_fire_carries_the_instruction_with_no_thread() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    let (bus, _rx) = EventBus::new(pool.clone());
    bus.emit(BusEvent::System(
        crate::engine::event_bus::SystemEvent::TriggerCreated {
            trigger_id: "nightly".into(),
            payload: serde_json::json!({ "trigger_id": "nightly" }),
            actor: Some(owner_device()),
        },
    ))
    .await
    .unwrap();

    assert!(refuse_without_authority(
        &bus_for(&pool),
        &agent_headers_for_fire(None, Some("nightly")),
        Some(f.sibling),
        Action::Apply,
    )
    .await
    .is_ok());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_threadless_subprocess_is_refused_rather_than_given_every_thread() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(None);

    let err = refuse_without_authority(&bus_for(&pool), &headers, Some(f.child), Action::Archive)
        .await
        .expect_err("no thread of its own");
    assert_eq!(err, ThreadReachError::NoCallerThread(Action::Archive));
    assert_eq!(err.status_code(), StatusCode::FORBIDDEN);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_tokened_agent_cannot_cancel_the_whole_workspace() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    let err = refuse_without_authority(
        &bus_for(&pool),
        &agent_headers(Some(f.child)),
        None,
        Action::Cancel,
    )
    .await
    .expect_err("an unscoped cancel is not an agent's to make");
    assert_eq!(err, ThreadReachError::UnscopedCancel);
    assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    assert!(err.to_string().contains("Name a thread"), "{err}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn no_refusal_sends_a_top_thread_to_a_parent_it_does_not_have() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let top = Uuid::new_v4();
    seed_thread(&bus, top, None).await;
    let headers = agent_headers(Some(top));

    for verb in EVERY_VERB {
        for target in [Some(f.child), None] {
            let Err(err) = refuse_without_authority(&bus_for(&pool), &headers, target, *verb).await
            else {
                panic!("{verb:?} at {target:?} must refuse");
            };
            let msg = err.to_string();
            assert!(!msg.contains("parent thread"), "{msg}");
            assert!(msg.contains("workspace owner"), "{msg}");
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_forged_token_does_not_authenticate_as_the_thread_it_names() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    let mine = agent_headers(Some(f.child));
    let mac = mine
        .get(HEADER_AGENT_ORIGIN_TOKEN)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.rsplit_once('.').map(|(_, m)| m.to_string()))
        .expect("minted token has a mac");
    let mut forged = HeaderMap::new();
    forged.insert(
        HEADER_AGENT_ORIGIN_TOKEN,
        format!("{}.{mac}", f.root).parse().unwrap(),
    );

    assert_eq!(
        crate::core::subprocess_origin::subprocess_origin(&forged),
        SubprocessOrigin::NotSubprocess,
        "a spliced prefix must not authenticate as the thread it names"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_home_thread_follows_up_any_thread_and_a_sibling_does_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let home_child = Uuid::new_v4();
    seed_thread(&bus, home_child, Some(home)).await;

    let reach = |caller, parent| follow_up_reach(&pool, caller, parent);
    assert_eq!(
        reach(home, Some(home)).await.unwrap(),
        Some(FollowUpReach::OwnChild)
    );
    for parent in [None, Some(f.root), Some(f.child)] {
        assert_eq!(
            reach(home, parent).await.unwrap(),
            Some(FollowUpReach::Home)
        );
    }
    assert_eq!(
        reach(f.child, Some(f.child)).await.unwrap(),
        Some(FollowUpReach::OwnChild)
    );
    assert_eq!(reach(f.child, Some(f.root)).await.unwrap(), None);
    assert_eq!(reach(f.sibling, Some(home)).await.unwrap(), None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_home_thread_gains_reach_but_not_authority() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let headers = agent_headers(Some(home));

    seed_thread_opened_by(&bus, home, None, None).await;
    for verb in EVERY_VERB {
        let Err(err) =
            refuse_without_authority(&bus_for(&pool), &headers, Some(f.sibling), *verb).await
        else {
            panic!("{verb:?} must refuse with no owner words in the turn");
        };
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN, "{verb:?}");
    }

    seed_thread_opened_by(&bus, home, None, Some(owner_device())).await;
    for verb in owner_buttons() {
        assert!(
            refuse_without_authority(&bus_for(&pool), &headers, Some(f.sibling), *verb)
                .await
                .is_ok(),
            "{verb:?} rides the owner's words in this turn"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn an_agent_never_presses_an_always_allow_grant() {
    use crate::engine::thread_events::AllowScope;
    let agent = agent_headers(Some(Uuid::new_v4()));
    for scope in [AllowScope::Narrow, AllowScope::Broad] {
        let err = require_owner_for_always_allow(&agent, Some(scope)).unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN, "{scope:?}");
        assert!(require_owner_for_always_allow(&device_headers(), Some(scope)).is_ok());
        for h in unattested_headers() {
            let err = require_owner_for_always_allow(&h, Some(scope)).unwrap_err();
            assert_eq!(
                err.message,
                crate::api::actor::NOT_ATTESTED_AS_OWNER,
                "{h:?}"
            );
        }
    }
    for scope in [None, Some(AllowScope::Session)] {
        assert!(
            require_owner_for_always_allow(&agent, scope).is_ok(),
            "{scope:?}"
        );
    }
}

#[test]
fn an_agent_never_writes_a_permission_grant_through_settings() {
    let err = require_owner_for_permission_grant(&agent_headers(Some(Uuid::new_v4()))).unwrap_err();
    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert!(require_owner_for_permission_grant(&device_headers()).is_ok());
    for h in unattested_headers() {
        let err = require_owner_for_permission_grant(&h).unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN, "{h:?}");
    }

    for (path, source, handler, write) in [
        (
            "settings.rs",
            include_str!("settings.rs"),
            "write_grant_lane",
            "tool_grants::change_standing(",
        ),
        (
            "mcp.rs",
            include_str!("mcp.rs"),
            "set_mcp_auto_approve",
            ".set_auto_approve(",
        ),
    ] {
        let body = handler_body(source, handler)
            .unwrap_or_else(|| panic!("{path}: no handler named {handler}"));
        let gate = body
            .find("require_owner_for_permission_grant")
            .unwrap_or_else(|| panic!("{path}: {handler} never refuses an agent"));
        let mutation = body.find(write).expect("the handler writes the grant");
        assert!(
            gate < mutation,
            "{path}: the refusal must come before the write"
        );
    }
}

#[test]
fn an_agent_never_writes_a_human_only_preference() {
    use crate::api::settings::require_owner_for_human_only_preference;
    use crate::core::prefs;
    let refuse = |h: &HeaderMap, key: &str| require_owner_for_human_only_preference(h, key);
    let agent = agent_headers(Some(Uuid::new_v4()));
    let internal: Vec<&str> = prefs::internal_specs().map(|s| s.key).collect();
    let named = [
        prefs::COMMAND_GUARD.key(),
        prefs::MAX_TOOL_CALLS.key(),
        prefs::LOCAL_BASE_URL.key(),
        prefs::CLAUDE_GATEWAY_URL.key(),
    ];
    let switches = crate::llm::provider_preferences::PROVIDER_ENABLED_SWITCHES.map(|s| s.key());
    for key in named.iter().chain(switches.iter()) {
        assert!(internal.contains(key), "{key} must be human-only");
    }
    for key in internal {
        let err = refuse(&agent, key).unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN, "{key}");
        assert!(err.message.contains(key), "{key}: {}", err.message);
        assert!(refuse(&device_headers(), key).is_ok(), "{key}");
        for h in unattested_headers() {
            assert!(refuse(&h, key).is_err(), "{key}: {h:?}");
        }
    }
    for key in [prefs::TIMEZONE.key(), "habit_tracker_goal"] {
        assert!(refuse(&agent, key).is_ok(), "{key}");
    }
}

#[test]
fn an_agent_never_turns_off_email_send_confirmation() {
    use crate::api::settings::lowers_send_confirmation as lowers;
    assert!(lowers(false, Ok(Some(true))));
    assert!(lowers(false, Ok(None)));
    assert!(lowers(false, Err(sqlx::Error::PoolTimedOut)));
    assert!(!lowers(false, Ok(Some(false))));
    assert!(!lowers(true, Ok(Some(true))));
    assert!(!lowers(true, Ok(Some(false))));
}

#[test]
fn every_human_only_route_refuses_an_agent_before_it_acts() {
    let settings = include_str!("settings.rs");
    let reveal = include_str!("credential_reveal.rs");
    let backup = include_str!("backup.rs");
    let gateway = include_str!("claude_gateway.rs");
    let pref_gate = "require_owner_for_human_only_preference(";
    let agent_gate = "require_attested_owner(";
    for (path, source, handler, gate, act) in [
        (
            "settings.rs",
            settings,
            "set_preference",
            pref_gate,
            ".apply_preference_write(",
        ),
        (
            "settings.rs",
            settings,
            "delete_preference",
            pref_gate,
            ".apply_preference_reset(",
        ),
        (
            "settings.rs",
            settings,
            "send_email_confirmed",
            agent_gate,
            "send_confirmed_draft(",
        ),
        (
            "settings.rs",
            settings,
            "update_credential",
            agent_gate,
            "CredentialStore::update(",
        ),
        (
            "claude_gateway.rs",
            gateway,
            "sign_in",
            pref_gate,
            ".apply_preference_write(",
        ),
        (
            "credential_reveal.rs",
            reveal,
            "mint_reveal_token",
            agent_gate,
            ".mint(",
        ),
        (
            "credential_reveal.rs",
            reveal,
            "get_credential_value",
            agent_gate,
            ".redeem(",
        ),
        (
            "backup.rs",
            backup,
            "mint_backup_key_reveal_token",
            agent_gate,
            ".mint(",
        ),
        (
            "backup.rs",
            backup,
            "check_key_access",
            agent_gate,
            ".redeem(",
        ),
    ] {
        let body = handler_body(source, handler)
            .unwrap_or_else(|| panic!("{path}: no handler named {handler}"));
        let refusal = body
            .find(gate)
            .unwrap_or_else(|| panic!("{path}: {handler} never refuses an agent"));
        let action = body
            .find(act)
            .unwrap_or_else(|| panic!("{path}: {handler} no longer calls {act}"));
        assert!(
            refusal < action,
            "{path}: {handler} must refuse an agent before {act}"
        );
    }
}

#[test]
fn every_consent_route_asks_the_gate_before_it_answers() {
    for (path, source, handler) in [
        (
            "command_permission.rs",
            include_str!("command_permission.rs"),
            "submit_command_consent",
        ),
        (
            "mcp_permission.rs",
            include_str!("mcp_permission.rs"),
            "submit_mcp_permission_consent",
        ),
        ("mcp.rs", include_str!("mcp.rs"), "submit_mcp_consent"),
    ] {
        let body = handler_body(source, handler)
            .unwrap_or_else(|| panic!("{path}: no handler named {handler}"));
        let gate = body
            .find("refuse_permission_answer")
            .unwrap_or_else(|| panic!("{path}: {handler} never asks the gate"));
        let resolve = body.find("resolve_").expect("the handler resolves a card");
        assert!(
            gate < resolve,
            "{path}: the gate must run before the card resolves"
        );
    }
}

#[test]
fn removing_a_queued_message_asks_the_control_gate_first() {
    let body = handler_body(CHAT_RS, "remove_queued_message").expect("the handler exists");
    let gate = body
        .find("refuse_without_authority")
        .expect("remove_queued_message never asks the gate");
    assert!(
        body[gate..].contains("Action::Control"),
        "taking back a queued message is a Control act"
    );
    let first_read = body
        .find("was_removed")
        .expect("the handler checks for an earlier removal");
    assert!(gate < first_read, "the gate must run before any lookup");
}

#[test]
fn draft_and_undo_routes_ask_the_gate_before_they_write() {
    for (source, handler, gate, verb, writes) in [
        (
            THREADS_COMPOSE_RS,
            "put_compose",
            "refuse_without_authority",
            "COMPOSE_VERB",
            &["upload_legacy_compose_images", "compose_update_sql()"][..],
        ),
        (
            BLOBS_RS,
            "post_blob",
            "refuse_without_authority",
            "COMPOSE_VERB",
            &["SELECT state", "write_blob(", "ImageUploaded"][..],
        ),
        (
            THREADS_COMPOSE_RS,
            "delete_thread",
            "refuse_without_authority",
            "DISCARD_DRAFT_VERB",
            &["SELECT state", "ThreadDiscarded"][..],
        ),
        (
            COMMAND_CHECKPOINT_RS,
            "authorize_checkpoint_undo",
            "refuse_without_authority",
            "Action::Revert",
            &[][..],
        ),
        (
            COMMAND_CHECKPOINT_RS,
            "undo_command_checkpoint",
            "authorize_checkpoint_undo",
            "authorize_checkpoint_undo",
            &[".undo_command_checkpoint("][..],
        ),
    ] {
        let body = handler_body(source, handler).expect("the handler exists");
        let at = body
            .find(gate)
            .unwrap_or_else(|| panic!("{handler} never asks the gate"));
        assert!(
            body[at..].contains(verb),
            "{handler} must ask the gate as {verb}"
        );
        for write in writes {
            let write_at = body
                .find(write)
                .unwrap_or_else(|| panic!("{handler} no longer does {write}"));
            assert!(at < write_at, "{handler} must ask the gate before {write}");
        }
    }
}

#[tokio::test]
async fn a_queued_message_is_taken_back_only_inside_the_callers_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let remove = |headers: HeaderMap, target: Uuid| {
        let pool = pool.clone();
        async move {
            refuse_without_authority(&bus_for(&pool), &headers, Some(target), Action::Control).await
        }
    };

    let child = agent_headers(Some(f.child));
    assert!(remove(child.clone(), f.child).await.is_ok());
    assert!(remove(child.clone(), f.grandchild).await.is_ok());
    for target in [f.sibling, f.root] {
        let err = remove(child.clone(), target).await.unwrap_err();
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.reason(), "out_of_reach");
    }
    assert!(remove(device_headers(), f.sibling).await.is_ok());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn cancelling_apply_all_asks_the_apply_gate_first() {
    let body = handler_body(CHANGES_RS, "cancel_apply_all_changes").expect("the handler exists");
    let gate = body
        .find("refuse_without_authority")
        .expect("cancel_apply_all_changes never asks the gate");
    assert!(
        body[gate..].contains("Action::Apply"),
        "cancelling Apply All takes back an Apply"
    );
    for touch in ["cancel_apply_all_batches", "drop_standing_applies"] {
        let at = body.find(touch).expect("the handler stops the batch");
        assert!(gate < at, "the gate must run before {touch}");
    }
}

#[tokio::test]
async fn only_the_owner_cancels_apply_all() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let cancel = |headers: HeaderMap| {
        let pool = pool.clone();
        async move { refuse_without_authority(&bus_for(&pool), &headers, None, Action::Apply).await }
    };

    for caller in [f.root, f.child, f.sibling] {
        let err = cancel(agent_headers(Some(caller))).await.unwrap_err();
        assert_eq!(err, ThreadReachError::NoStandingInstruction(Action::Apply));
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    }
    assert!(cancel(device_headers()).await.is_ok());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn no_thread_answers_a_permission_card() {
    use crate::engine::thread_events::AllowScope;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let answer = |headers: HeaderMap, card: Uuid, allowed: bool, scope: Option<AllowScope>| {
        let pool = pool.clone();
        async move {
            refuse_permission_answer(&bus_for(&pool), &headers, Some(card), allowed, scope).await
        }
    };

    let child = agent_headers(Some(f.child));
    for allowed in [true, false] {
        let err = answer(child.clone(), f.grandchild, allowed, None)
            .await
            .unwrap_err();
        assert_eq!(
            err.status,
            StatusCode::FORBIDDEN,
            "its own sub-thread's card"
        );
    }
    assert!(
        answer(device_headers(), f.sibling, true, Some(AllowScope::Broad))
            .await
            .is_ok()
    );

    seed_thread_opened_by(&bus, home, None, Some(owner_device())).await;
    let from_home = agent_headers(Some(home));
    for (allowed, scope) in [
        (true, Some(AllowScope::Session)),
        (false, Some(AllowScope::Narrow)),
    ] {
        let err = answer(from_home.clone(), f.sibling, allowed, scope)
            .await
            .unwrap_err();
        assert_eq!(
            err.status,
            StatusCode::FORBIDDEN,
            "the owner's words relay nothing"
        );
    }
    let err = answer(from_home, f.sibling, true, Some(AllowScope::Narrow))
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::FORBIDDEN);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_card_answer_from_a_thread_needs_a_batch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let orchestrator = Uuid::new_v4();
    seed_thread_opened_by(&bus, orchestrator, None, Some(owner_device())).await;
    let callers = [
        (agent_headers(Some(f.root)), Some(f.root), "its own thread"),
        (agent_headers(Some(f.root)), Some(f.child), "its own child"),
        (
            agent_headers(Some(orchestrator)),
            Some(f.sibling),
            "a turn the owner opened",
        ),
        (
            agent_headers_for_fire(None, Some("nightly")),
            Some(f.child),
            "a trigger firing",
        ),
    ];

    for verb in [Action::AnswerQuestion, Action::ResolvePermission] {
        assert!(verb.answers_a_card());
        for (headers, target, how) in &callers {
            let refused = refuse_without_authority(&bus_for(&pool), headers, *target, verb).await;
            let Err(err) = refused else {
                panic!("{verb:?} on {how} must refuse");
            };
            assert_eq!(
                err,
                ThreadReachError::CardAnswerNeedsBatch,
                "{verb:?} on {how}"
            );
            assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
            let msg = err.to_string();
            assert!(msg.contains("lucidos permissions batch"), "{msg}");
            assert!(msg.contains("workspace owner"), "{msg}");
        }
        assert!(
            refuse_without_authority(&bus_for(&pool), &device_headers(), Some(f.child), verb)
                .await
                .is_ok(),
            "the owner's device still answers"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn allow(bus: &EventBus, caller: Uuid, verb: Action, target: Uuid, label: &str) {
    use crate::api::permission_requests::{tests::grant, thread_ask};
    let ask = thread_ask(bus.pool(), caller, verb, Some(target), "I need it.")
        .await
        .unwrap();
    grant(bus, caller, ask, label).await;
}

#[tokio::test]
async fn allow_once_lets_one_act_out_of_reach_through_once() {
    use crate::engine::permission::menu::ALLOW_ONCE_LABEL;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let press = |verb| refuse_thread_without_authority(&bus, f.child, Some(f.sibling), verb);

    assert!(matches!(
        press(Action::Archive).await,
        Err(ThreadReachError::OutOfReach { .. })
    ));
    allow(&bus, f.child, Action::Archive, f.sibling, ALLOW_ONCE_LABEL).await;
    assert!(press(Action::Cancel).await.is_err(), "one verb only");
    assert!(
        refuse_thread_without_authority(&bus, f.child, Some(f.root), Action::Archive)
            .await
            .is_err(),
        "one target only"
    );
    assert!(
        refuse_thread_without_authority(&bus, f.grandchild, Some(f.sibling), Action::Archive)
            .await
            .is_err(),
        "the asking thread only"
    );
    assert!(press(Action::Archive).await.is_ok());
    assert!(press(Action::Archive).await.is_err(), "spent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn two_racing_acts_spend_one_allow_once_once() {
    use crate::engine::permission::menu::ALLOW_ONCE_LABEL;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    allow(&bus, f.child, Action::Continue, f.sibling, ALLOW_ONCE_LABEL).await;

    let press =
        || refuse_thread_without_authority(&bus, f.child, Some(f.sibling), Action::Continue);
    let (a, b) = tokio::join!(press(), press());
    assert!(a.is_ok() ^ b.is_ok(), "{a:?} {b:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn always_allow_lets_any_thread_press_the_verb_on_its_target() {
    use crate::engine::permission::menu::{ALWAYS_ALLOW_LABEL, DONT_ALLOW_LABEL};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    allow(&bus, f.child, Action::Archive, f.root, ALWAYS_ALLOW_LABEL).await;

    for caller in [f.child, f.sibling, f.child] {
        refuse_thread_without_authority(&bus, caller, Some(f.root), Action::Archive)
            .await
            .unwrap();
    }
    assert_eq!(
        refuse_without_authority(&bus, &agent_headers(None), Some(f.root), Action::Archive).await,
        Err(ThreadReachError::NoCallerThread(Action::Archive))
    );

    use crate::api::permission_requests::{prepare, tests::ask_prepared, thread_ask};
    let ask = thread_ask(&pool, f.child, Action::Apply, Some(f.root), "Ship it.")
        .await
        .unwrap();
    let request_id = prepare(&bus, f.child, ask).await.unwrap();
    let card = ask_prepared(&bus, f.child, &request_id).await;
    assert!(!card.labels().contains(&ALWAYS_ALLOW_LABEL));
    assert!(card.labels().contains(&DONT_ALLOW_LABEL));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_gate_leaves_a_draft_sends_grant_to_its_acceptance() {
    use crate::engine::permission::thread_user;
    use crate::engine::permission::{live_permission, menu::ALLOW_ONCE_LABEL};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    allow(
        &bus,
        f.child,
        Action::SubmitDraft,
        f.sibling,
        ALLOW_ONCE_LABEL,
    )
    .await;

    assert!(
        refuse_thread_without_authority(&bus, f.child, Some(f.sibling), Action::SubmitDraft)
            .await
            .is_err()
    );
    let target = f.sibling.to_string();
    assert!(live_permission(
        &pool,
        &thread_user(f.child),
        Action::SubmitDraft,
        Some(&target)
    )
    .await
    .unwrap()
    .is_some());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn archive_asks_the_reach_gate_before_the_cascade() {
    for (source, handler, gate, archive) in [
        (
            ARCHIVE_RS,
            "archive_thread",
            "refuse_without_authority",
            "archive_family(",
        ),
        (
            ARCHIVE_RS,
            "archive_thread_as_caller",
            "refuse_without_authority",
            "archive_as_agent(",
        ),
        (
            TOOLS_RS,
            "execute_archive_thread",
            "refuse_tool_without_authority",
            "archive_as_agent(",
        ),
    ] {
        let body = handler_body(source, handler).expect("the handler exists");
        let gate_at = body.find(gate).expect("the handler asks the gate");
        let archive_at = body.find(archive).expect("the handler archives the target");
        assert!(gate_at < archive_at, "{handler} must check authority first");
        assert!(
            body[gate_at..archive_at].contains("Action::Archive"),
            "{handler} must check the archive permission"
        );
    }
}

#[tokio::test]
async fn a_snapshot_restore_is_never_covered_by_reach_or_a_standing_instruction() {
    let (pool, db_name) = setup_test_db().await;
    let bus = bus_for(&pool);
    let f = family(&pool).await;
    let owner_turn = Uuid::new_v4();
    seed_thread_opened_by(&bus, owner_turn, None, Some(owner_device())).await;
    for (caller, target) in [
        (f.root, f.child),
        (f.root, f.root),
        (owner_turn, f.sibling),
        (owner_turn, owner_turn),
    ] {
        assert_eq!(
            authority_held(
                &pool,
                Some(caller),
                None,
                Some(target),
                Action::RestoreSnapshot
            )
            .await
            .unwrap(),
            None,
            "{caller} -> {target}"
        );
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unreadable_ladder_is_an_error_never_a_false() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    pool.close().await;
    assert!(matches!(
        authorize_thread_reach(&pool, f.root, f.child).await,
        Err(ThreadReachError::Unverifiable(_))
    ));
    assert!(matches!(
        hold_reach(&pool, f.root, f.child).await,
        Err(ThreadReachError::Unverifiable(_))
    ));
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_parent_a_sibling_and_a_missing_thread_refuse_with_the_same_words() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let missing = Uuid::new_v4();
    let mut refusals = Vec::new();
    for (caller, target) in [(f.child, f.root), (f.child, f.sibling), (f.child, missing)] {
        let err = refuse_without_authority(
            &bus_for(&pool),
            &agent_headers(Some(caller)),
            Some(target),
            Action::Archive,
        )
        .await
        .expect_err("out of reach");
        refusals.push((
            err.reason(),
            err.status_code(),
            err.to_string().replace(&target.to_string(), "<target>"),
        ));
    }
    assert!(refusals.windows(2).all(|w| w[0] == w[1]), "{refusals:?}");
    pool.close().await;
    teardown_test_db(&db_name).await;
}
