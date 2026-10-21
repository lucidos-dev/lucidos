//! The standing consent to hand off under a parent: the build chain's (ADR
//! 0503) and a carried *handoff consent* (ADR 0507). One verb, one target,
//! and only when the handoff estimate pays.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::EventBus;
use crate::test_support::{
    seed_chat_thread, seed_coding_thread, seed_consented_handoff, seed_handoff, setup_test_db,
    teardown_test_db,
};

/// Headers as the subprocess of `thread_id` sends them.
fn agent_headers(thread_id: Uuid) -> HeaderMap {
    init_agent_origin_secret("thread-reach-test-secret".to_string());
    let token = mint_agent_origin_token(Some(thread_id), 0, None).expect("the secret is installed");
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    headers
}

/// A context of `tokens` on a priced model. With no history, the estimate
/// reads the fallbacks: 400k pays, and 100k sits at the floor.
async fn context(pool: &sqlx::PgPool, thread_id: Uuid, tokens: u64) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
    )
    .bind(serde_json::json!({"model": "claude-opus-5-5", "usage": {"input_tokens": tokens}}))
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap();
}

fn pre_harden() -> CheckpointRun {
    CheckpointRun::parse("pre-harden", None).unwrap()
}

/// I4, I7: a chain member hands off under its planner when the estimate for
/// the named checkpoint pays, and needs authority otherwise.
#[tokio::test]
async fn the_chain_consents_only_when_the_named_moment_pays() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let planner = seed_coding_thread(&bus, None).await;
    let builder = seed_handoff(&bus, planner, Some(planner), true).await;
    let next = seed_handoff(&bus, builder, Some(planner), false).await;

    context(&pool, next, 100_000).await;
    assert!(standing_consent(&pool, next, planner, Some(&pre_harden()))
        .await
        .unwrap()
        .is_none());
    context(&pool, next, 400_000).await;
    assert_eq!(
        standing_consent(&pool, next, planner, Some(&pre_harden()))
            .await
            .unwrap(),
        Some(StandingConsent::BuildChain)
    );
    assert!(
        standing_consent(&pool, next, planner, None)
            .await
            .unwrap()
            .is_none(),
        "a spawn that names no checkpoint gets no consent"
    );
    assert!(
        hands_off_without_card(&pool, &agent_headers(next), next, planner, &pre_harden())
            .await
            .unwrap()
    );

    teardown_test_db(&db).await;
}

/// I7, I8: the consent covers only the chain's own planner, and a child
/// outside a chain still needs `spawn-successor` authority.
#[tokio::test]
async fn the_consent_reaches_no_other_target_and_no_other_child() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let planner = seed_coding_thread(&bus, None).await;
    let builder = seed_handoff(&bus, planner, Some(planner), true).await;
    context(&pool, builder, 400_000).await;
    let elsewhere = seed_chat_thread(&bus, "elsewhere").await;
    assert!(
        standing_consent(&pool, builder, elsewhere, Some(&pre_harden()))
            .await
            .unwrap()
            .is_none()
    );

    let parent = seed_chat_thread(&bus, "parent").await;
    let child = seed_coding_thread(&bus, Some(parent)).await;
    context(&pool, child, 400_000).await;
    assert!(standing_consent(&pool, child, parent, Some(&pre_harden()))
        .await
        .unwrap()
        .is_none());
    assert!(
        !hands_off_without_card(&pool, &agent_headers(child), child, parent, &pre_harden())
            .await
            .unwrap(),
        "a child outside a chain needs a card in a turn the owner did not open"
    );

    teardown_test_db(&db).await;
}

/// I8: chain membership lends no other verb on the planner.
#[tokio::test]
async fn chain_membership_lends_no_other_verb() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let planner = seed_coding_thread(&bus, None).await;
    let builder = seed_handoff(&bus, planner, Some(planner), true).await;
    context(&pool, builder, 400_000).await;
    for verb in [
        ThreadReachVerb::Archive,
        ThreadReachVerb::Apply,
        ThreadReachVerb::Cancel,
        ThreadReachVerb::SpawnSuccessor,
    ] {
        assert_eq!(
            authority_held(&pool, Some(builder), None, Some(planner), verb)
                .await
                .unwrap(),
            None,
            "{verb:?}"
        );
    }

    teardown_test_db(&db).await;
}

/// Whether the context reading would let `thread_id` hand off with no card,
/// as its own agent asks at the pre-harden checkpoint.
async fn without_card(pool: &sqlx::PgPool, thread_id: Uuid, parent: Uuid) -> bool {
    hands_off_without_card(
        pool,
        &agent_headers(thread_id),
        thread_id,
        parent,
        &pre_harden(),
    )
    .await
    .unwrap()
}

/// ADR 0507, I1 and I5: a successor whose predecessor recorded a consent
/// hands off under the shared parent only at a paying automatic checkpoint.
/// The reading and the spawn's predicate agree in every case.
#[tokio::test]
async fn a_carried_consent_holds_only_at_a_paying_checkpoint() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = seed_chat_thread(&bus, "parent").await;
    let first = seed_coding_thread(&bus, Some(parent)).await;
    let next = seed_consented_handoff(
        &bus,
        first,
        Some(parent),
        false,
        Some(HandoffConsent::OwnerApproval),
    )
    .await;

    context(&pool, next, 100_000).await;
    assert!(standing_consent(&pool, next, parent, Some(&pre_harden()))
        .await
        .unwrap()
        .is_none());
    assert!(!without_card(&pool, next, parent).await);

    context(&pool, next, 400_000).await;
    assert_eq!(
        standing_consent(&pool, next, parent, Some(&pre_harden()))
            .await
            .unwrap(),
        Some(StandingConsent::Carried)
    );
    assert!(without_card(&pool, next, parent).await);
    assert!(
        standing_consent(&pool, next, parent, None)
            .await
            .unwrap()
            .is_none(),
        "a spawn that names no checkpoint gets no consent"
    );

    teardown_test_db(&db).await;
}

/// ADR 0507, I2: no recorded consent, or another parent, and the handoff
/// needs a card.
#[tokio::test]
async fn a_carried_consent_reaches_no_other_parent_or_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = seed_chat_thread(&bus, "parent").await;
    let first = seed_coding_thread(&bus, Some(parent)).await;
    let unconsented = seed_handoff(&bus, first, Some(parent), false).await;
    context(&pool, unconsented, 400_000).await;
    assert!(
        standing_consent(&pool, unconsented, parent, Some(&pre_harden()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(!without_card(&pool, unconsented, parent).await);

    let other = seed_coding_thread(&bus, Some(parent)).await;
    let consented = seed_consented_handoff(
        &bus,
        other,
        Some(parent),
        false,
        Some(HandoffConsent::Owner),
    )
    .await;
    context(&pool, consented, 400_000).await;
    let elsewhere = seed_chat_thread(&bus, "elsewhere").await;
    assert!(
        standing_consent(&pool, consented, elsewhere, Some(&pre_harden()))
            .await
            .unwrap()
            .is_none()
    );

    teardown_test_db(&db).await;
}

/// ADR 0507, I3: the recorded consent names the authority that passed. A
/// machine caller and a thread's own subtree record none.
#[test]
fn the_recorded_consent_names_the_authority_that_passed() {
    let device = MessageOrigin::Device {
        device_id: "my-phone".into(),
    };
    let machine = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Engine,
        source_thread_id: None,
    };
    for (passed, actor, expected) in [
        (
            PassedAuthority::Untokened,
            Some(&device),
            Some(HandoffConsent::Owner),
        ),
        (PassedAuthority::Untokened, Some(&machine), None),
        (PassedAuthority::Untokened, None, None),
        (
            PassedAuthority::Held(HeldAuthority::StandingInstruction),
            None,
            Some(HandoffConsent::Owner),
        ),
        (PassedAuthority::Held(HeldAuthority::OwnSubtree), None, None),
        (
            PassedAuthority::OwnerApproval,
            None,
            Some(HandoffConsent::OwnerApproval),
        ),
    ] {
        assert_eq!(recorded_consent(passed, actor), expected, "{passed:?}");
    }
}

/// A successor started at a checkpoint is told to skip it. A flow saying
/// "run it at first entry" then does not send it back there.
#[test]
fn a_checkpoint_successor_is_told_to_skip_that_checkpoint() {
    let opening = narrative_opening(
        "[Old](thread:x)",
        Some(&pre_harden()),
        "Next: review.",
        None,
    );
    assert!(
        opening.contains("`pre-harden` checkpoint, so skip that checkpoint"),
        "{opening}"
    );
    assert!(opening.ends_with("Next: review."), "{opening}");

    let plain = narrative_opening("[Old](thread:x)", None, "Next: review.", None);
    assert!(!plain.contains("skip"), "{plain}");
}

/// I3: the request the user typed at the card follows the narrative, so the
/// successor reads it last.
#[test]
fn a_narrative_successor_ends_on_the_users_request() {
    let opening = narrative_opening(
        "[Old](thread:x)",
        None,
        "Next: review.",
        Some("Add a dark-mode toggle"),
    );
    assert!(
        opening.ends_with("The user's first request for you:\n\nAdd a dark-mode toggle"),
        "{opening}"
    );
}

/// ADR 0507, I8: a handoff that spent an owner approval records it, and the
/// successor it starts carries it at its own paying checkpoint. So does the
/// successor after that one, on the consent it carried.
#[tokio::test]
async fn an_approved_handoff_passes_its_consent_down_the_lineage() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = seed_chat_thread(&bus, "parent").await;
    let first = seed_coding_thread(&bus, Some(parent)).await;
    let approved = recorded_consent(PassedAuthority::OwnerApproval, None);
    let second = seed_consented_handoff(&bus, first, Some(parent), false, approved).await;
    context(&pool, second, 400_000).await;
    assert!(without_card(&pool, second, parent).await);

    let carried = match standing_consent(&pool, second, parent, Some(&pre_harden()))
        .await
        .unwrap()
    {
        Some(StandingConsent::Carried) => Some(HandoffConsent::Carried),
        other => panic!("the second successor carries the approval, got {other:?}"),
    };
    let third = seed_consented_handoff(&bus, second, Some(parent), false, carried).await;
    context(&pool, third, 400_000).await;
    assert!(without_card(&pool, third, parent).await);

    teardown_test_db(&db).await;
}
