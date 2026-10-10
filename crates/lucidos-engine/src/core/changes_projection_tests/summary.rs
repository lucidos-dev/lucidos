use super::cp_helpers::*;
use super::*;

const TWO_COMMITS: &str = "fix: settle the read\nfeat: add the summary";
const THREE_COMMITS: &str = "fix: one more\nfix: settle the read\nfeat: add the summary";

fn proposed(change_id: Uuid, description: &str) -> ThreadEvent {
    let mut event = aggregate_proposed(change_id, "branch-s", "/repo");
    if let ThreadEvent::ChangeProposed { description: d, .. } = &mut event {
        *d = Some(description.to_string());
    }
    event
}

fn summarized(change_id: Uuid, summary: &str, description: &str) -> ThreadEvent {
    ThreadEvent::ChangeSummarized {
        change_id: change_id.to_string(),
        summary: summary.to_string(),
        description: description.to_string(),
    }
}

async fn stored_summary(pool: &PgPool, change_id: Uuid) -> Option<String> {
    ChangesProjection::new(pool.clone())
        .get_by_id(change_id)
        .await
        .unwrap()
        .expect("change row")
        .summary
}

/// Emits one proposal on a fresh thread and returns the pieces a test needs.
async fn proposed_change(description: &str) -> (PgPool, String, EventBus, Uuid, Uuid) {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(&bus, thread, proposed(change_id, description)).await;
    (pool, db, bus, thread, change_id)
}

#[tokio::test]
async fn a_summary_of_the_current_commit_list_is_stored() {
    let (pool, db, bus, thread, change_id) = proposed_change(TWO_COMMITS).await;

    emit(
        &bus,
        thread,
        summarized(change_id, "Adds change summaries", TWO_COMMITS),
    )
    .await;

    assert_eq!(
        stored_summary(&pool, change_id).await.as_deref(),
        Some("Adds change summaries")
    );
    teardown_test_db(&db).await;
}

/// The race this guards: the model was still summarizing two commits when a
/// third landed. Its late answer describes a list the change no longer has.
#[tokio::test]
async fn a_summary_of_an_older_commit_list_is_dropped() {
    let (pool, db, bus, thread, change_id) = proposed_change(TWO_COMMITS).await;
    emit(&bus, thread, proposed(change_id, THREE_COMMITS)).await;

    emit(&bus, thread, summarized(change_id, "Stale", TWO_COMMITS)).await;

    assert_eq!(stored_summary(&pool, change_id).await, None);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_new_commit_list_clears_the_summary() {
    let (pool, db, bus, thread, change_id) = proposed_change(TWO_COMMITS).await;
    emit(
        &bus,
        thread,
        summarized(change_id, "Adds change summaries", TWO_COMMITS),
    )
    .await;

    emit(&bus, thread, proposed(change_id, THREE_COMMITS)).await;

    assert_eq!(stored_summary(&pool, change_id).await, None);
    teardown_test_db(&db).await;
}

/// An idle re-proposes the same commits on every turn end. That must not throw
/// away a summary that still fits.
#[tokio::test]
async fn re_proposing_the_same_commit_list_keeps_the_summary() {
    let (pool, db, bus, thread, change_id) = proposed_change(TWO_COMMITS).await;
    emit(
        &bus,
        thread,
        summarized(change_id, "Adds change summaries", TWO_COMMITS),
    )
    .await;

    emit(&bus, thread, proposed(change_id, TWO_COMMITS)).await;

    assert_eq!(
        stored_summary(&pool, change_id).await.as_deref(),
        Some("Adds change summaries")
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_rebuilt_row_keeps_the_summary_of_its_commit_list() {
    let (pool, db, bus, thread, change_id) = proposed_change(TWO_COMMITS).await;
    emit(&bus, thread, summarized(change_id, "Stale", TWO_COMMITS)).await;
    emit(&bus, thread, proposed(change_id, THREE_COMMITS)).await;
    emit(
        &bus,
        thread,
        summarized(change_id, "Current", THREE_COMMITS),
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    ChangesProjection::new(pool.clone())
        .rebuild_missing_from_events()
        .await
        .unwrap();

    assert_eq!(
        stored_summary(&pool, change_id).await.as_deref(),
        Some("Current")
    );
    teardown_test_db(&db).await;
}
