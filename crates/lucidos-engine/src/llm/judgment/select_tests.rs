//! The opt-in gate. Every case that is not an explicit `jev` runs `chat`.

use super::*;
use crate::core::AuthType;
use crate::test_support::{seed_credential, seed_preference, setup_test_db, teardown_test_db};

const A_TIMEOUT: Duration = Duration::from_secs(5);

async fn seed_key(pool: &PgPool) {
    seed_credential(
        pool,
        TYPESAFE_CREDENTIAL_SERVICE,
        "https://api.typesafe.ai/v1",
        AuthType::Bearer,
        "test-key",
    )
    .await;
}

#[test]
fn only_the_word_jev_opts_a_site_in() {
    assert!(wants_jev(Some("jev")));
    assert!(wants_jev(Some("  JEV  ")), "a human types this value");
    assert!(!wants_jev(Some("chat")));
    assert!(!wants_jev(Some("")));
    assert!(!wants_jev(Some("jevvy")));
    assert!(
        !wants_jev(Some("typesafe")),
        "the backend is named by its model"
    );
    assert!(!wants_jev(None), "unset is chat");
}

#[test]
fn the_two_sites_own_two_different_keys() {
    assert_eq!(
        JudgmentSite::CommandGuard.preference_key(),
        "judgment_command_guard"
    );
    assert_eq!(
        JudgmentSite::QueryClassification.preference_key(),
        "judgment_query_classification"
    );
}

/// The promise the whole opt-in exists for: storing a TypeSafe key changes no
/// behavior by itself. Only the preference moves a call site.
#[tokio::test]
async fn a_stored_key_alone_never_switches_a_site_over() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;

    for site in [
        JudgmentSite::CommandGuard,
        JudgmentSite::QueryClassification,
    ] {
        assert!(
            jev_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} has no preference set, so it stays on chat"
        );
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_site_asking_for_jev_with_a_key_gets_a_provider() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(&pool, JudgmentSite::CommandGuard.preference_key(), "jev")
        .await
        .expect("seed the preference");

    assert!(jev_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_some());
    assert!(
        jev_for(&pool, JudgmentSite::QueryClassification, A_TIMEOUT)
            .await
            .is_none(),
        "opting one site in must not move the other"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unrecognized_preference_value_runs_chat() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(&pool, JudgmentSite::CommandGuard.preference_key(), "gpt-5")
        .await
        .expect("seed the preference");

    assert!(jev_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

/// The master switch is a veto over both sites at once. A key is stored and
/// both sites ask for Jev, so only the switch can be what stops them.
#[tokio::test]
async fn the_master_switch_stops_every_site() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    for site in [
        JudgmentSite::CommandGuard,
        JudgmentSite::QueryClassification,
    ] {
        seed_preference(&pool, site.preference_key(), "jev")
            .await
            .expect("seed the site preference");
    }
    seed_preference(&pool, PREF_PROVIDER_ENABLED_TYPESAFE, "false")
        .await
        .expect("seed the master switch");

    for site in [
        JudgmentSite::CommandGuard,
        JudgmentSite::QueryClassification,
    ] {
        assert!(
            jev_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} asked for Jev, but TypeSafe is switched off"
        );
    }
    teardown_test_db(&db).await;
}

/// Absent means on, so the switch shipping changes nothing for a workspace
/// that already runs Jev. An explicit `true` is the same answer, written down.
#[tokio::test]
async fn an_unset_or_true_master_switch_leaves_a_site_on_jev() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(&pool, JudgmentSite::CommandGuard.preference_key(), "jev")
        .await
        .expect("seed the site preference");

    assert!(
        jev_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
            .await
            .is_some(),
        "an unset master switch must behave exactly as before it existed"
    );

    seed_preference(&pool, PREF_PROVIDER_ENABLED_TYPESAFE, "true")
        .await
        .expect("seed the master switch");
    assert!(jev_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_some());
    teardown_test_db(&db).await;
}

/// The `judge` tool needs no third preference. It replaces no existing path,
/// so a configured provider is the whole condition, exactly as a configured
/// image provider is the whole condition for `generate_image`.
///
/// Skipped when the launch environment supplies a key, since the no-key half
/// cannot arise there.
#[tokio::test]
async fn the_judge_tool_rides_on_the_master_switch_and_a_key() {
    if std::env::var(TYPESAFE_API_KEY_ENV).is_ok() {
        return;
    }
    let (pool, db) = setup_test_db().await;
    assert!(
        !judgment_available(&pool).await,
        "no key means no tool, whatever the switch says"
    );

    seed_key(&pool).await;
    assert!(
        judgment_available(&pool).await,
        "an unset master switch means on, so a key alone offers the tool"
    );
    assert!(jev_for_agent(&pool, A_TIMEOUT).await.is_ok());

    seed_preference(&pool, PREF_PROVIDER_ENABLED_TYPESAFE, "false")
        .await
        .expect("seed the master switch");
    assert!(!judgment_available(&pool).await, "the switch is a veto");
    // `JevProvider` has no `Debug`, because it holds the key. So the Ok side is
    // matched out rather than unwrapped.
    let Err(refusal) = jev_for_agent(&pool, A_TIMEOUT).await else {
        panic!("a switched-off provider must refuse");
    };
    assert!(refusal.contains("Settings"), "{refusal}");
    teardown_test_db(&db).await;
}

/// The two neighbours stay on their chat path while the tool is offered. That
/// is the whole point of the split: a capability the agent can reach for is not
/// a decision the engine has moved to a different backend.
#[tokio::test]
async fn offering_the_tool_moves_neither_classification_site() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;

    assert!(judgment_available(&pool).await);
    for site in [
        JudgmentSite::CommandGuard,
        JudgmentSite::QueryClassification,
    ] {
        assert!(
            jev_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} must still run its own path"
        );
    }
    teardown_test_db(&db).await;
}

/// The switch reaches the command guard's backend, and that key is already
/// human-only. A settable master switch would be a back door around it.
#[test]
fn the_master_switch_is_not_agent_settable() {
    use crate::core::preference_catalog::{internal_hint, lookup};

    assert!(lookup(PREF_PROVIDER_ENABLED_TYPESAFE).is_none());
    let hint = internal_hint(PREF_PROVIDER_ENABLED_TYPESAFE).expect("must carry a hint");
    assert!(
        hint.contains("Settings"),
        "the hint must point at the Settings surface, got: {hint}"
    );
}

/// Asking for Jev without a key degrades to chat rather than failing. Skipped
/// when the launch environment supplies a key, because then the case cannot
/// arise here.
#[tokio::test]
async fn asking_for_jev_without_a_key_runs_chat() {
    if std::env::var(TYPESAFE_API_KEY_ENV).is_ok() {
        return;
    }
    let (pool, db) = setup_test_db().await;
    seed_preference(
        &pool,
        JudgmentSite::QueryClassification.preference_key(),
        "jev",
    )
    .await
    .expect("seed the preference");

    assert!(jev_for(&pool, JudgmentSite::QueryClassification, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}
