//! Who may press which button. Every test here proves one of three things: an
//! agent cannot leave its own subtree, the owner's standing instruction lets it,
//! or the user's own device never notices that either rule exists.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

/// Every clause-4 verb, so a test that must hold "per verb" says so by
/// iterating rather than by picking one and hoping.
const EVERY_VERB: &[ThreadReachVerb] = &[
    ThreadReachVerb::Archive,
    ThreadReachVerb::Cancel,
    ThreadReachVerb::Apply,
    ThreadReachVerb::Discard,
    ThreadReachVerb::AnswerQuestion,
    ThreadReachVerb::Continue,
    ThreadReachVerb::CreateTopThread,
];

/// Every surface carrying a clause-4 verb, and the file its handler lives in.
///
/// Fourteen HTTP routes for seven verbs, since Apply, Discard and cancel each
/// arrive by more than one path. Plus the three LLM tools that press Apply
/// in-process. Hand-written, which is its weakness: a route added elsewhere is
/// invisible here until somebody adds the row, and that is how arming an apply
/// shipped ungated. Review a new change route against this list.
const GATED_HANDLERS: &[(&str, &str, &str)] = &[
    ("changes.rs", CHANGES_RS, "apply_change"),
    ("changes.rs", CHANGES_RS, "discard_change"),
    ("changes.rs", CHANGES_RS, "apply_all_changes"),
    ("changes.rs", CHANGES_RS, "discard_all_changes"),
    ("chat.rs", CHAT_RS, "chat_submit"),
    ("chat.rs", CHAT_RS, "cancel_chat"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_stop"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_apply_now"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_discard"),
    ("claude_code.rs", CLAUDE_CODE_RS, "claude_code_interrupt"),
    ("threads/actions.rs", ACTIONS_RS, "answer_thread_question"),
    ("threads/actions.rs", ACTIONS_RS, "continue_thread"),
    ("threads/archive.rs", ARCHIVE_RS, "archive_thread"),
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
const ACTIONS_RS: &str = include_str!("threads/actions.rs");
const ARCHIVE_RS: &str = include_str!("threads/archive.rs");
const TOOLS_RS: &str = include_str!("../engine/tools/mod.rs");

/// The gate under any of its names: the header form a route asks, the
/// thread form an in-process tool asks, and the three local wrappers. Each
/// wrapper is asserted to reach the real one by the test below, so naming them
/// here is not a hole.
const GATE_CALLS: &[&str] = &[
    "refuse_without_authority",
    "refuse_thread_without_authority",
    "refuse_change_verb",
    "refuse_batch_change_verb",
    "refuse_tool_without_authority",
];

/// The plan's own verification for "the standing instruction has exactly one
/// definition": a source scan asserting every clause-4 verb asks it rather than
/// re-deriving the check.
///
/// A behavioral test cannot cover this. Both ADRs diagnose one failure: a verb
/// gated on the route somebody remembered and forgotten on its second. A route
/// that never calls the gate has no behavior to assert against.
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
    ] {
        let body =
            handler_body(source, wrapper).unwrap_or_else(|| panic!("no wrapper named {wrapper}"));
        assert!(
            body.contains(reaches),
            "{wrapper} stands in for the gate, so it must reach {reaches}"
        );
    }
    // The controls: two reads that ask no gate, one a free function and one a
    // method. A slicer running past a body would pick the gate up from a
    // neighbour, and every assertion above would be passing on nothing.
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

/// One function or method body, as source text.
///
/// The end is the closing brace at the same column the `async fn` sits on. A
/// method inside an `impl` therefore stops at its own brace, rather than
/// running to the end of the block and borrowing a neighbour's gate. Crude on
/// purpose: it needs to find a call, not parse Rust.
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

/// The refusal is the sentence an agent acts on, so it has to read as one.
///
/// Two ways it stopped reading once the verb set grew past archive and cancel.
/// A frame that inflected the verb produced "a thread applys itself". A
/// root-aimed refusal pointed at "this thread", which names nothing, since that
/// refusal exists precisely when there is no thread.
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

/// Headers as a Lucidos-spawned subprocess sends them: a thread-bound origin
/// token it cannot re-point, minted over `thread_id`.
///
/// The secret is per-engine-startup and installed first-writer-wins, so each
/// test installs one rather than assuming a booted engine did. Without it,
/// minting returns `None` and every header map comes out empty, which would
/// make a token-bearing caller read as an ordinary untokened one. That is the
/// opposite of what these tests assert.
fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    agent_headers_for_fire(thread_id, None)
}

/// The same, for a subprocess that IS a trigger's fire. The trigger id rides
/// the signed prefix, so it is authenticated exactly as the thread id is.
fn agent_headers_for_fire(thread_id: Option<Uuid>, emitting_trigger_id: Option<&str>) -> HeaderMap {
    init_agent_origin_secret("thread-reach-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, emitting_trigger_id)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

/// The user's browser or phone: a device id and no origin token.
fn device_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        crate::api::actor::HEADER_DEVICE_ID,
        "device-abc".parse().unwrap(),
    );
    h
}

async fn seed_thread(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>) {
    seed_thread_opened_by(bus, thread_id, parent, None).await;
}

/// Seed a thread whose turn `origin` opened. `None` is the shape every ladder
/// test wants: a thread nobody's device spoke into, so it carries no standing
/// instruction and the ladder is the only thing answering.
async fn seed_thread_opened_by(
    bus: &EventBus,
    thread_id: Uuid,
    parent: Option<Uuid>,
    origin: Option<MessageOrigin>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

fn owner_device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "device-abc".into(),
        label: "My MacBook".into(),
    }
}

/// A root with two children, and a grandchild under the first child. Enough
/// shape to tell a descendant from a sibling, and a direct child from a
/// deeper one.
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

/// A thread ends its own work whenever it likes. This is the case that costs
/// no query at all, which is why it is asserted before the tree cases.
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

/// Descendants, not just direct children. The archive route cascades to the
/// whole family, so a ladder that stopped at one level would authorize less
/// than the verb actually does.
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

/// The defect this whole change exists to close: ADR 0083's star topology says
/// nothing points sideways, and archive said otherwise.
#[tokio::test]
async fn a_thread_cannot_reach_its_sibling() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    assert!(!authorize_thread_reach(&pool, f.child, f.sibling)
        .await
        .unwrap());

    let err = refuse_without_authority(
        &pool,
        &agent_headers(Some(f.child)),
        Some(f.sibling),
        ThreadReachVerb::Archive,
    )
    .await
    .expect_err("a sibling is out of reach");
    assert_eq!(
        err,
        ThreadReachError::OutOfReach {
            target: f.sibling,
            verb: ThreadReachVerb::Archive
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

/// Upward is refused as firmly as sideways. A child that thinks its parent has
/// gone wrong reports to the parent; it does not stop it.
#[tokio::test]
async fn a_thread_cannot_reach_its_own_parent_or_grandparent() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for (caller, target) in [(f.child, f.root), (f.grandchild, f.root)] {
        let err = refuse_without_authority(
            &pool,
            &agent_headers(Some(caller)),
            Some(target),
            ThreadReachVerb::Cancel,
        )
        .await
        .expect_err("an ancestor is out of reach");
        assert_eq!(
            err,
            ThreadReachError::OutOfReach {
                target,
                verb: ThreadReachVerb::Cancel
            }
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A top-level thread in another part of the workspace, and a uuid with no row
/// at all. Both are out of reach, and both refuse identically: the caller
/// learns the thread is not its own and nothing about whether it exists.
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
            &pool,
            &agent_headers(Some(f.root)),
            Some(target),
            ThreadReachVerb::Archive,
        )
        .await
        .expect_err("out of reach");
        assert_eq!(err.reason(), "out_of_reach");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant: the user is unaffected. A request with no origin token is the
/// browser, the phone or any other local API client. The gate returns before
/// it looks at the target at all, for every verb.
#[tokio::test]
async fn a_user_device_reaches_every_thread_exactly_as_before() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for headers in [device_headers(), HeaderMap::new()] {
        for verb in EVERY_VERB {
            for target in [Some(f.root), Some(f.sibling), Some(f.grandchild), None] {
                assert!(
                    refuse_without_authority(&pool, &headers, target, *verb)
                        .await
                        .is_ok(),
                    "an untokened caller keeps the reach it has always had"
                );
            }
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The same ladder, reached the way a real request reaches it: through the
/// token rather than through a caller id someone passed in.
#[tokio::test]
async fn a_tokened_agent_is_bound_to_its_own_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.child));

    for target in [f.child, f.grandchild] {
        assert!(
            refuse_without_authority(&pool, &headers, Some(target), ThreadReachVerb::Archive)
                .await
                .is_ok()
        );
    }
    for target in [f.root, f.sibling] {
        let err = refuse_without_authority(&pool, &headers, Some(target), ThreadReachVerb::Archive)
            .await
            .expect_err("outside its own subtree");
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant: a clause-4 verb aimed outside the caller's own subtree refuses
/// when no standing instruction is present. Per verb, because the gate is per
/// verb: a caller carrying a token and no owner turn gets the same answer
/// whichever button it reaches for.
#[tokio::test]
async fn every_verb_refuses_outside_the_subtree_with_no_standing_instruction() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.child));

    for verb in EVERY_VERB {
        let Err(err) = refuse_without_authority(&pool, &headers, Some(f.sibling), *verb).await
        else {
            panic!("{verb:?} must refuse a sibling");
        };
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN, "{verb:?}");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant: in-subtree action needs no standing instruction. The 264 measured
/// in-lane applies are this test, and a parent that could no longer apply its
/// own child's work is the failure it guards.
#[tokio::test]
async fn every_verb_reaches_the_caller_s_own_subtree_with_no_owner_turn() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(Some(f.root));

    for verb in EVERY_VERB {
        for target in [f.root, f.child, f.grandchild] {
            assert!(
                refuse_without_authority(&pool, &headers, Some(target), *verb)
                    .await
                    .is_ok(),
                "{verb:?} inside the caller's own subtree needs nobody's instruction"
            );
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Clause 5: the owner opened this turn, so their words in it are the press.
/// The same caller that was refused above now reaches its sibling.
#[tokio::test]
async fn a_turn_the_owner_opened_reaches_past_the_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let orchestrator = Uuid::new_v4();
    seed_thread_opened_by(&bus, orchestrator, None, Some(owner_device())).await;
    let headers = agent_headers(Some(orchestrator));

    for verb in EVERY_VERB {
        assert!(
            refuse_without_authority(&pool, &headers, Some(f.sibling), *verb)
                .await
                .is_ok(),
            "{verb:?} rides the owner's standing instruction"
        );
    }
    assert!(
        refuse_without_authority(&pool, &headers, None, ThreadReachVerb::CreateTopThread)
            .await
            .is_ok(),
        "so does an act aimed at the root, which is nobody's subtree"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant: a trigger firing counts as a standing instruction, and a trigger
/// thread still reaches its own subtree without one. Both sides, because the
/// failure signal is a nightly pipeline that stops applying at 3am.
#[tokio::test]
async fn a_trigger_fire_reaches_its_own_subtree_and_wider() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let f = family(&pool).await;
    let trigger_thread = Uuid::new_v4();
    let its_child = Uuid::new_v4();
    // The owner wrote the trigger, which is what makes its fire theirs.
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
            trigger_id: "nightly".into(),
            trigger_name: Some("Nightly release".into()),
            prompt: None,
            invocation: None,
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
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
        refuse_without_authority(&pool, &headers, Some(its_child), ThreadReachVerb::Apply)
            .await
            .is_ok(),
        "its own subtree, which clause 3 already covered"
    );
    assert!(
        refuse_without_authority(&pool, &headers, Some(f.sibling), ThreadReachVerb::Apply)
            .await
            .is_ok(),
        "and wider, on the firing the owner authorized"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A script trigger has no thread to read a turn from, and is the same standing
/// instruction. Its fire rides the token, so it reaches what an intent
/// trigger's thread reaches.
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
        &pool,
        &agent_headers_for_fire(None, Some("nightly")),
        Some(f.sibling),
        ThreadReachVerb::Apply,
    )
    .await
    .is_ok());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A subprocess with a token, no thread and no fire behind it. It has no
/// subtree and no turn, so it is refused rather than handed the whole
/// workspace. Same answer the event-wait routes give.
#[tokio::test]
async fn a_threadless_subprocess_is_refused_rather_than_given_every_thread() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(None);

    let err = refuse_without_authority(&pool, &headers, Some(f.child), ThreadReachVerb::Archive)
        .await
        .expect_err("no thread of its own");
    assert_eq!(
        err,
        ThreadReachError::NoCallerThread(ThreadReachVerb::Archive)
    );
    assert_eq!(err.status_code(), StatusCode::FORBIDDEN);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `POST /api/v1/chat/cancel` with no `thread_id` stops every thread in the
/// workspace, which is the user's global Stop. A thread-bound caller is
/// refused explicitly rather than quietly reread as "cancel yourself".
#[tokio::test]
async fn a_tokened_agent_cannot_cancel_the_whole_workspace() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    let err = refuse_without_authority(
        &pool,
        &agent_headers(Some(f.child)),
        None,
        ThreadReachVerb::Cancel,
    )
    .await
    .expect_err("an unscoped cancel is not an agent's to make");
    assert_eq!(err, ThreadReachError::UnscopedCancel);
    assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    assert!(err.to_string().contains("Name a thread"), "{err}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant: a refusal never names a parent the caller does not have. This
/// caller is a top-thread, so the old "say so to your parent thread" pointed at
/// nothing. Asserted over every verb and both target shapes, since the sentence
/// is what an agent acts on.
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
            let Err(err) = refuse_without_authority(&pool, &headers, target, *verb).await else {
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

/// A token re-pointed at another thread does not verify, so it reads as no
/// token at all. Re-asserted here because this ladder is only as strong as
/// that binding: `api::actor` owns the property, and this is what leans on it.
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
        crate::api::actor::subprocess_origin(&forged),
        SubprocessOrigin::NotSubprocess,
        "a spliced prefix must not authenticate as the thread it names"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
