//! Place workspaces in the release notice sequence on startup.

use std::collections::HashMap;
use std::sync::RwLock;

use lucidos_engine::engine::release_notices::{
    all, has_legacy_trigger_grants, place_workspace, running_release,
};
use lucidos_engine::triggers::TriggerConfig;
use sqlx::PgPool;

/// Place a workspace in the sequence, on the first boot that finds it outside.
///
/// Runs once per boot. The running release determines how far a workspace
/// without threads or retired trigger grants can start in the sequence.
/// An existing usable cursor stays in place.
pub async fn seed_cursor_at_startup(
    pool: &PgPool,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
) {
    let notices = all();
    // Retired grants are existing workspace content even before any trigger
    // has run.
    if has_legacy_trigger_grants(trigger_configs) {
        return;
    }
    match running_release() {
        Ok(running) => place_workspace(pool, notices, &running).await,
        Err(e) => lucidos_core::log_legacy!("[ReleaseNotices] {e}, so no notice can be placed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_core::test_support::{setup_test_db, teardown_test_db};
    use lucidos_engine::engine::command_guard::SideEffectCategory;
    use lucidos_engine::engine::release_notices::stored_cursor;

    #[tokio::test]
    async fn retired_trigger_grants_are_visible_before_review() {
        let (pool, db_name) = setup_test_db().await;
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "nightly",
            "name": "Trigger nightly",
            "schedule": ["0 0 3 * * *"],
            "run": { "type": "intent", "intent": "fix what broke" },
            "coding_agent_grant": false,
        }))
        .expect("build TriggerConfig");
        let configs = RwLock::new(HashMap::from([("nightly".to_string(), config)]));

        configs
            .write()
            .unwrap()
            .get_mut("nightly")
            .unwrap()
            .side_effect_grant = vec![SideEffectCategory::Email];
        seed_cursor_at_startup(&pool, &configs).await;
        assert!(stored_cursor(&pool).await.is_none());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
