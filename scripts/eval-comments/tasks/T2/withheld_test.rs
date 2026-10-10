use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

#[tokio::test]
async fn bare_rows_of_1m_default_claude_families_get_the_1m_window() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));

    for id in [
        "claude-opus-5-5",
        "claude-opus-5@default",
        "claude-fable-5",
        "claude-fable-5-1",
    ] {
        assert_eq!(context_window_for(&registry, id), 1_000_000, "{id}");
    }
    for id in [
        "claude-sonnet-5",
        "claude-opus-4-7",
        "claude-opus-4-8@default",
    ] {
        assert_eq!(context_window_for(&registry, id), 200_000, "{id}");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}
