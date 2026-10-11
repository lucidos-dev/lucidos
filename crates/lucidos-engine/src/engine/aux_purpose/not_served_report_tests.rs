use super::*;
use crate::engine::aux_purpose::NotServed;
use crate::llm::model_registry::ProviderKind;
use crate::test_support::{setup_test_db, teardown_test_db};

const HAIKU: &str = "claude-haiku-4-5";

/// A recorder over Vertex alone, where the prefix heuristic sends `HAIKU`.
fn recorder(pool: &PgPool, source: ModelSource) -> NotServedRecorder {
    let (bus, _rx) = EventBus::new(pool.clone());
    NotServedRecorder {
        bus,
        pool: pool.clone(),
        registry: crate::llm::model_registry::empty(),
        reach: Reach::configured(&[ProviderKind::Vertex]),
        purpose: ContextPurpose::CommandJudge,
        source,
    }
}

async fn count(pool: &PgPool, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
        .bind(event_type)
        .fetch_one(pool)
        .await
        .expect("count events")
}

fn refusal() -> ModelNotServed {
    ModelNotServed::new("Vertex has no `claude-haiku-4-5` in region `eu`")
}

/// A stored pick that failed for good tells the user once per window, and
/// names the provider that refused it (I6).
#[tokio::test]
async fn a_failed_stored_pick_is_announced_and_notified_once() {
    let (pool, db_name) = setup_test_db().await;
    let report = recorder(&pool, ModelSource::Preference);
    report.not_served(HAIKU, &refusal(), None).await;
    report.not_served(HAIKU, &refusal(), None).await;
    assert_eq!(count(&pool, NOT_SERVED_EVENT).await, 1);
    assert_eq!(count(&pool, "NotificationCreated").await, 1);
    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload->'data' FROM events WHERE event_type = $1")
            .bind(NOT_SERVED_EVENT)
            .fetch_one(&pool)
            .await
            .expect("the event");
    assert_eq!(payload["provider"], "vertex");
    assert_eq!(payload["model"], HAIKU);
    assert_eq!(payload["purpose"], "command_judge");
    assert!(payload.get("moved_to").is_none());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A default that moved on is recorded, so later defaults skip the model,
/// but the call answered, so nobody is interrupted.
#[tokio::test]
async fn a_default_that_moved_on_is_recorded_without_a_notification() {
    let (pool, db_name) = setup_test_db().await;
    let report = recorder(&pool, ModelSource::Default);
    report
        .not_served(HAIKU, &refusal(), Some("gemini-3-flash-preview"))
        .await;
    assert_eq!(count(&pool, NOT_SERVED_EVENT).await, 1);
    assert_eq!(count(&pool, "NotificationCreated").await, 0);
    assert_eq!(
        NotServed::recent(&pool).await,
        NotServed::of(&[(ProviderKind::Vertex, HAIKU)])
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A default that moved past a model does not silence a stored pick that
/// later fails on it: the failure still records and notifies.
#[tokio::test]
async fn a_failed_pick_notifies_after_a_default_moved_past_the_same_model() {
    let (pool, db_name) = setup_test_db().await;
    recorder(&pool, ModelSource::Default)
        .not_served(HAIKU, &refusal(), Some("gemini-3-flash-preview"))
        .await;
    recorder(&pool, ModelSource::Preference)
        .not_served(HAIKU, &refusal(), None)
        .await;
    assert_eq!(count(&pool, NOT_SERVED_EVENT).await, 2);
    assert_eq!(count(&pool, "NotificationCreated").await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two calls failing at once still record one event and one notification.
#[tokio::test]
async fn concurrent_failures_record_once() {
    let (pool, db_name) = setup_test_db().await;
    let (a, b) = (
        recorder(&pool, ModelSource::Preference),
        recorder(&pool, ModelSource::Preference),
    );
    let refusal = refusal();
    tokio::join!(
        a.not_served(HAIKU, &refusal, None),
        b.not_served(HAIKU, &refusal, None)
    );
    assert_eq!(count(&pool, NOT_SERVED_EVENT).await, 1);
    assert_eq!(count(&pool, "NotificationCreated").await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The window query and the emitted variant spell the event the same way.
#[test]
fn the_query_reads_the_variant_it_emits() {
    let event = SystemEvent::ModelNotServedObserved {
        model: HAIKU.to_string(),
        provider: "vertex".to_string(),
        purpose: ContextPurpose::CommandJudge,
        message: String::new(),
        moved_to: None,
    };
    assert_eq!(event.event_type(), NOT_SERVED_EVENT);
    assert!(event.is_persisted());
}

#[test]
fn a_task_reads_as_prose() {
    assert_eq!(task_name(ContextPurpose::CommandJudge), "command judge");
    assert_eq!(task_name(ContextPurpose::Title), "title");
}
