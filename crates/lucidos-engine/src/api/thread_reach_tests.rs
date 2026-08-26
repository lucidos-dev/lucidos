//! Who may archive or cancel whom. Every test here either proves an agent
//! cannot leave its own subtree, or proves the user's device never notices
//! that the ladder exists.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

/// Headers as a Lucidos-spawned subprocess sends them: a thread-bound origin
/// token it cannot re-point, minted over `thread_id`.
///
/// The secret is per-engine-startup and installed first-writer-wins, so each
/// test installs one rather than assuming a booted engine did. Without it,
/// minting returns `None` and every header map comes out empty, which would
/// make a token-bearing caller read as an ordinary untokened one. That is the
/// opposite of what these tests assert.
fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("thread-reach-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
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
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
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
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
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

    assert!(
        authorize_thread_reach(&pool, f.child, f.child, ThreadReachVerb::Cancel)
            .await
            .is_ok()
    );

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
        authorize_thread_reach(&pool, f.root, f.child, ThreadReachVerb::Archive)
            .await
            .is_ok(),
        "a direct child is in reach"
    );
    assert!(
        authorize_thread_reach(&pool, f.root, f.grandchild, ThreadReachVerb::Archive)
            .await
            .is_ok(),
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

    let err = authorize_thread_reach(&pool, f.child, f.sibling, ThreadReachVerb::Archive)
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
    // Actionable, and it names the verb the caller actually tried.
    let msg = err.to_string();
    assert!(msg.contains("archive"), "{msg}");
    assert!(msg.contains("parent thread"), "{msg}");

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
        let err = authorize_thread_reach(&pool, caller, target, ThreadReachVerb::Cancel)
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
        let err = authorize_thread_reach(&pool, f.root, target, ThreadReachVerb::Archive)
            .await
            .expect_err("out of reach");
        assert_eq!(err.reason(), "out_of_reach");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Invariant 2: the user is unaffected. A request with no origin token is the
/// browser, the phone or any other local API client. The gate returns before
/// it looks at the target at all.
#[tokio::test]
async fn a_user_device_reaches_every_thread_exactly_as_before() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;

    for headers in [device_headers(), HeaderMap::new()] {
        for target in [Some(f.root), Some(f.sibling), Some(f.grandchild)] {
            assert!(
                refuse_out_of_reach(&pool, &headers, target, ThreadReachVerb::Archive)
                    .await
                    .is_ok(),
                "an untokened caller keeps the reach it has always had"
            );
        }
        assert!(
            refuse_out_of_reach(&pool, &headers, None, ThreadReachVerb::Cancel)
                .await
                .is_ok(),
            "the global Stop button cancels everything, as it always has"
        );
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
            refuse_out_of_reach(&pool, &headers, Some(target), ThreadReachVerb::Archive)
                .await
                .is_ok()
        );
    }
    for target in [f.root, f.sibling] {
        let err = refuse_out_of_reach(&pool, &headers, Some(target), ThreadReachVerb::Archive)
            .await
            .expect_err("outside its own subtree");
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A subprocess with a token but no thread is a scheduled script. It has no
/// subtree, so there is nothing to scope it to, and it is refused rather than
/// handed the whole workspace. Same answer the event-wait routes give.
#[tokio::test]
async fn a_threadless_subprocess_is_refused_rather_than_given_every_thread() {
    let (pool, db_name) = setup_test_db().await;
    let f = family(&pool).await;
    let headers = agent_headers(None);

    let err = refuse_out_of_reach(&pool, &headers, Some(f.child), ThreadReachVerb::Archive)
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

    let err = refuse_out_of_reach(
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
