//! The opt-in gate. Every case that is not an explicit, configured System One
//! pick runs `chat`.

use super::*;
use crate::core::AuthType;
use crate::llm::judgment::endpoint::{
    CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE, TYPESAFE_API_KEY_ENV, TYPESAFE_CREDENTIAL_SERVICE,
};
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

async fn seed_cloudflare_token(pool: &PgPool) {
    seed_credential(
        pool,
        CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
        "https://api.cloudflare.com/client/v4/accounts/abc123/ai",
        AuthType::Bearer,
        "cf-token",
    )
    .await;
}

/// Every credential a System One row can read, stored at once.
async fn seed_every_key(pool: &PgPool) {
    seed_key(pool).await;
    seed_cloudflare_token(pool).await;
    seed_preference(
        pool,
        prefs::SYSTEM_ONE_CUSTOM_URL.key(),
        "http://localhost:8080/v1/systemone",
    )
    .await
    .expect("seed the custom URL");
    seed_preference(pool, prefs::SYSTEM_ONE_CUSTOM_MODEL.key(), "kev")
        .await
        .expect("seed the custom model");
}

#[test]
fn only_a_row_id_picks_a_system_one_endpoint() {
    assert_eq!(picked_endpoint(Some("jev")), Some(SystemOneEndpoint::Jev));
    assert_eq!(
        picked_endpoint(Some("  JEV  ")),
        Some(SystemOneEndpoint::Jev),
        "a human types this value"
    );
    assert_eq!(picked_endpoint(Some("clef")), Some(SystemOneEndpoint::Clef));
    assert_eq!(
        picked_endpoint(Some("clef-flash")),
        Some(SystemOneEndpoint::ClefFlash)
    );
    assert_eq!(
        picked_endpoint(Some("custom")),
        Some(SystemOneEndpoint::Custom)
    );
    assert_eq!(picked_endpoint(Some("chat")), None);
    assert_eq!(picked_endpoint(Some("")), None);
    assert_eq!(picked_endpoint(Some("jevvy")), None);
    assert_eq!(
        picked_endpoint(Some("typesafe")),
        None,
        "the backend is named by its model"
    );
    assert_eq!(picked_endpoint(None), None, "unset is chat");
}

#[test]
fn every_site_owns_a_different_key() {
    let keys: std::collections::HashSet<&str> = JudgmentSite::ALL
        .into_iter()
        .map(|site| site.preference_key().key())
        .collect();
    assert_eq!(keys.len(), JudgmentSite::ALL.len(), "{keys:?}");
}

/// I13 and I14: storing a credential changes no site by itself. Every key a
/// System One row reads is present, and still every site runs on chat.
#[tokio::test]
async fn a_stored_key_alone_never_switches_a_site_over() {
    let (pool, db) = setup_test_db().await;
    seed_every_key(&pool).await;

    for site in JudgmentSite::ALL {
        assert!(
            system_one_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} has no preference set, so it stays on chat"
        );
    }
    teardown_test_db(&db).await;
}

/// I14 for the site that will send summary tree lines: an untouched workspace
/// sends `find` to its chat model, never to a third party.
#[tokio::test]
async fn an_untouched_workspace_runs_memory_find_on_chat() {
    let (pool, db) = setup_test_db().await;
    assert!(system_one_for(&pool, JudgmentSite::MemoryFind, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_site_picking_jev_with_a_key_gets_a_provider() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "jev",
    )
    .await
    .expect("seed the preference");

    assert!(system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_some());
    for other in [JudgmentSite::QueryClassification, JudgmentSite::MemoryFind] {
        assert!(
            system_one_for(&pool, other, A_TIMEOUT).await.is_none(),
            "picking for one site must not move {other:?}"
        );
    }
    teardown_test_db(&db).await;
}

/// Each seeded row and the custom one resolve once their own provider is
/// configured.
#[tokio::test]
async fn every_configured_row_resolves() {
    let (pool, db) = setup_test_db().await;
    seed_every_key(&pool).await;

    for endpoint in SystemOneEndpoint::ALL {
        seed_preference(
            &pool,
            JudgmentSite::MemoryFind.preference_key().key(),
            endpoint.id(),
        )
        .await
        .expect("seed the preference");
        assert!(
            system_one_for(&pool, JudgmentSite::MemoryFind, A_TIMEOUT)
                .await
                .is_some(),
            "{endpoint:?} is configured"
        );
    }
    teardown_test_db(&db).await;
}

/// A Clef pick needs a Cloudflare token. A TypeSafe key is not one.
#[tokio::test]
async fn picking_clef_without_a_cloudflare_token_runs_chat() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(
        &pool,
        JudgmentSite::QueryClassification.preference_key().key(),
        "clef",
    )
    .await
    .expect("seed the preference");

    assert!(
        system_one_for(&pool, JudgmentSite::QueryClassification, A_TIMEOUT)
            .await
            .is_none()
    );
    teardown_test_db(&db).await;
}

/// A custom row with no URL has nowhere to send the state.
#[tokio::test]
async fn picking_custom_without_a_url_runs_chat() {
    let (pool, db) = setup_test_db().await;
    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "custom",
    )
    .await
    .expect("seed the preference");

    assert!(system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unrecognized_preference_value_runs_chat() {
    let (pool, db) = setup_test_db().await;
    seed_every_key(&pool).await;
    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "gpt-5",
    )
    .await
    .expect("seed the preference");

    assert!(system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

/// A master switch is a veto over every site on its provider, and only those.
#[tokio::test]
async fn a_master_switch_stops_every_site_on_its_provider() {
    let (pool, db) = setup_test_db().await;
    seed_every_key(&pool).await;
    for site in JudgmentSite::ALL {
        seed_preference(&pool, site.preference_key().key(), "jev")
            .await
            .expect("seed the site preference");
    }
    seed_preference(&pool, prefs::PROVIDER_ENABLED_TYPESAFE.key(), "false")
        .await
        .expect("seed the master switch");

    for site in JudgmentSite::ALL {
        assert!(
            system_one_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} picked Jev, but TypeSafe is switched off"
        );
    }

    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "clef-flash",
    )
    .await
    .expect("seed the site preference");
    assert!(
        system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
            .await
            .is_some(),
        "TypeSafe's switch says nothing about Cloudflare"
    );

    seed_preference(
        &pool,
        SystemOneEndpoint::ClefFlash.switch_key().key(),
        "off",
    )
    .await
    .expect("seed the Cloudflare switch");
    assert!(system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
        .await
        .is_none());
    teardown_test_db(&db).await;
}

/// Absent means on, so the switch shipping changes nothing for a workspace
/// that already runs Jev. An explicit `true` is the same answer, written down.
#[tokio::test]
async fn an_unset_or_true_master_switch_leaves_a_site_on_jev() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;
    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "jev",
    )
    .await
    .expect("seed the site preference");

    assert!(
        system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
            .await
            .is_some(),
        "an unset master switch must behave exactly as before it existed"
    );

    seed_preference(&pool, prefs::PROVIDER_ENABLED_TYPESAFE.key(), "true")
        .await
        .expect("seed the master switch");
    assert!(system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
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

    seed_preference(&pool, prefs::PROVIDER_ENABLED_TYPESAFE.key(), "false")
        .await
        .expect("seed the master switch");
    assert!(!judgment_available(&pool).await, "the switch is a veto");
    // `SystemOneProvider` has no `Debug`, because it holds the key. So the Ok
    // side is matched out rather than unwrapped.
    let Err(refusal) = jev_for_agent(&pool, A_TIMEOUT).await else {
        panic!("a switched-off provider must refuse");
    };
    assert!(refusal.contains("Settings"), "{refusal}");
    teardown_test_db(&db).await;
}

/// The sites stay on their chat model while the tool is offered. That is the
/// whole point of the split: a capability the agent can reach for is not a
/// decision the engine has moved to a different backend.
#[tokio::test]
async fn offering_the_tool_moves_no_judgment_site() {
    let (pool, db) = setup_test_db().await;
    seed_key(&pool).await;

    assert!(judgment_available(&pool).await);
    for site in JudgmentSite::ALL {
        assert!(
            system_one_for(&pool, site, A_TIMEOUT).await.is_none(),
            "{site:?} must still run on chat"
        );
    }
    teardown_test_db(&db).await;
}

/// The switches reach the command guard's backend, and that key is already
/// human-only. A settable master switch would be a back door around it. The
/// custom URL receives every state a site sends, so the agent must not pick it.
#[test]
fn the_system_one_settings_are_not_agent_settable() {
    use crate::core::preference_catalog::{internal_hint, lookup};

    let keys = SystemOneEndpoint::ALL
        .into_iter()
        .map(|endpoint| endpoint.switch_key().key())
        .chain(
            JudgmentSite::ALL
                .into_iter()
                .map(|site| site.preference_key().key()),
        )
        .chain([
            prefs::SYSTEM_ONE_CUSTOM_URL.key(),
            prefs::SYSTEM_ONE_CUSTOM_MODEL.key(),
        ]);
    for key in keys {
        assert!(lookup(key).is_none(), "{key} must not be agent-settable");
        let hint = internal_hint(key).unwrap_or_else(|| panic!("{key} must carry a hint"));
        assert!(
            hint.contains("Settings"),
            "the hint for {key} must point at the Settings surface, got: {hint}"
        );
    }
}

/// Picking Jev without a key degrades to chat rather than failing. Skipped
/// when the launch environment supplies a key, because then the case cannot
/// arise here.
#[tokio::test]
async fn picking_jev_without_a_key_runs_chat() {
    if std::env::var(TYPESAFE_API_KEY_ENV).is_ok() {
        return;
    }
    let (pool, db) = setup_test_db().await;
    seed_preference(
        &pool,
        JudgmentSite::QueryClassification.preference_key().key(),
        "jev",
    )
    .await
    .expect("seed the preference");

    assert!(
        system_one_for(&pool, JudgmentSite::QueryClassification, A_TIMEOUT)
            .await
            .is_none()
    );
    teardown_test_db(&db).await;
}

/// The custom endpoint's key goes only where its credential scope covers the
/// URL. A URL moved after the key was saved must not carry the key along.
#[tokio::test]
async fn the_custom_key_goes_only_inside_its_scope() {
    let (pool, db) = setup_test_db().await;
    seed_preference(
        &pool,
        JudgmentSite::CommandGuard.preference_key().key(),
        "custom",
    )
    .await
    .expect("seed the preference");
    seed_preference(&pool, prefs::SYSTEM_ONE_CUSTOM_MODEL.key(), "kev")
        .await
        .expect("seed the model");
    seed_credential(
        &pool,
        crate::llm::judgment::endpoint::SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
        "http://localhost:8080",
        AuthType::Bearer,
        "custom-key",
    )
    .await;

    for (url, keyed) in [
        ("http://localhost:8080/v1/systemone", true),
        ("https://decider.example/v1/systemone", false),
    ] {
        seed_preference(&pool, prefs::SYSTEM_ONE_CUSTOM_URL.key(), url)
            .await
            .expect("seed the URL");
        let provider = system_one_for(&pool, JudgmentSite::CommandGuard, A_TIMEOUT)
            .await
            .expect("a URL and a model are enough");
        assert_eq!(provider.sends_a_key(), keyed, "{url}");
    }
    teardown_test_db(&db).await;
}
