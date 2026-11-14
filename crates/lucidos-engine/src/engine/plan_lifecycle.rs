//! Durable planning stages. A start is an event, never permission to edit.
use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{EventMeta, PlanKind, ThreadEvent};
use uuid::Uuid;

/// Serialize planning starts across engines. Retrying after an acknowledged or
/// interrupted request observes the committed event and does not start twice.
/// A kind switch or a completed approval/landing starts a new planning cycle.
pub(crate) async fn start(
    bus: &EventBus,
    thread_id: Uuid,
    kind: PlanKind,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut claim = bus.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("plan-lifecycle:{thread_id}"))
        .execute(&mut *claim)
        .await?;
    let last: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT event_type, payload->>'kind' FROM events WHERE aggregate_id = $1 \
         AND event_type IN ('PlanStarted', 'PlanApproved', 'ChangeApplied') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut *claim)
    .await?;
    if last.as_ref().is_some_and(|(event, prior_kind)| {
        event == "PlanStarted" && prior_kind.as_deref() == Some(kind.as_str())
    }) {
        return Ok(());
    }
    let recorded = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PlanStarted { kind },
            meta: EventMeta::NONE,
        })
        .await?;
    if recorded.is_none() {
        return Err("planning start was refused".into());
    }
    claim.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn failed_append_is_reported_without_a_stage_fact() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        crate::test_support::refuse_event_type(&pool, "PlanStarted").await;
        let thread = seed_coding_thread(&bus, None).await;
        let error = start(&bus, thread, PlanKind::Design).await.unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted'",
        )
        .bind(thread.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn retries_and_concurrent_starts_record_one_stage_and_no_marker() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let (first, second) = tokio::join!(
            start(&bus, thread, PlanKind::Implementation),
            start(&bus, thread, PlanKind::Implementation)
        );
        first.unwrap();
        second.unwrap();
        start(&bus, thread, PlanKind::Implementation).await.unwrap();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'kind' FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted' ORDER BY sequence",
        ).bind(thread.to_string()).fetch_all(&pool).await.unwrap();
        assert_eq!(kinds, ["implementation"]);
        start(&bus, thread, PlanKind::Design).await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted'",
        )
        .bind(thread.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2, "a different kind starts a real stage");
        let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(markers, 0, "a start never opens the edit gate");
        teardown_test_db(&db).await;
    }
}
