#[tokio::test]
async fn automatic_build_policy_is_workspace_persistent_and_defaults_to_auto() {
    use crate::core::prefs;
    use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    assert!(super::automatic_source_builds_enabled(&pool).await);
    seed_preference(&pool, prefs::FOLLOW_MAIN_BUILD.key(), "on-switch")
        .await
        .unwrap();
    assert!(!super::automatic_source_builds_enabled(&pool).await);
    seed_preference(&pool, prefs::FOLLOW_MAIN_BUILD.key(), "auto")
        .await
        .unwrap();
    assert!(super::automatic_source_builds_enabled(&pool).await);
    teardown_test_db(&db).await;
}

#[test]
fn following_never_restarts_the_engine() {
    let source = include_str!("follow_main.rs");
    let code = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in ["restart", "request_switch", "exit("] {
        let calls = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains(forbidden));
        assert!(!calls, "follow_main.rs must not call {forbidden}");
    }
    assert!(code.contains("automatic_background_rebuild().await"));
}
