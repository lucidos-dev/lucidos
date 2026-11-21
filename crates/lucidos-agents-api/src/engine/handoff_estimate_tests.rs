mod db {
    use super::super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventMeta, ThreadEvent};
    use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};
    use chrono::Utc;
    use sqlx::PgPool;
    use uuid::Uuid;

    include!("../../../lucidos-engine/src/engine/handoff_estimate_capture_test.rs");

    /// Backdate every capture of `thread_id`, so its prompt cache is cold.
    async fn go_cold(pool: &PgPool, thread_id: Uuid) {
        sqlx::query(
            "UPDATE events SET created = now() - interval '2 hours' \
              WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
        )
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn card(pool: &PgPool, thread_id: Uuid) -> CardVerdict {
        let context = crate::engine::successor::thread_context(pool, thread_id)
            .await
            .unwrap();
        card_verdict(pool, thread_id, &context)
            .await
            .unwrap()
            .expect("a reading")
    }

    /// I6: the card's verdict comes from the handoff estimate, on both cache
    /// states, and the warm one names when it goes cold.
    #[tokio::test]
    async fn the_card_reads_both_cache_states_off_the_estimate() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        calls(&pool, thread, "claude-opus-5-5", 400_000).await;
        let warm = card(&pool, thread).await;
        assert!(warm.warm && warm.cold);
        assert!(warm.cold_at.is_some_and(|at| at > Utc::now()));
        assert!(warm.fresh_ratio > 1.0);
        go_cold(&pool, thread).await;
        let cold = card(&pool, thread).await;
        assert!(cold.cold && cold.cold_at.is_none());

        let small = seed_coding_thread(&bus, None).await;
        calls(&pool, small, "claude-opus-5-5", HANDOFF_FLOOR_TOKENS).await;
        let small = card(&pool, small).await;
        assert!(!small.warm && !small.cold, "never at the floor");

        // A write with no split has no lifetime, so no time for the card to
        // go cold. Its after-warm verdict claims no cold rewrite.
        let unknown = seed_coding_thread(&bus, None).await;
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
        )
        .bind(serde_json::json!({"model": "claude-opus-5-5", "usage": {
            "input_tokens": 400_000,
            "cache_creation_tokens": 1000,
        }}))
        .bind(unknown)
        .execute(&pool)
        .await
        .unwrap();
        let unknown = card(&pool, unknown).await;
        assert!(unknown.cold_at.is_none());
        assert_eq!(unknown.warm, warm.warm);

        teardown_test_db(&db).await;
    }

    /// I5: Stay here hides the card, and it returns past the hold.
    #[tokio::test]
    async fn a_decline_hides_the_card_until_the_thread_grows() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        calls(&pool, thread, "claude-opus-5-5", 400_000).await;
        assert!(card(&pool, thread).await.warm);
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::SuccessorOfferDeclined {
                input_tokens: 400_000,
            },
            meta: EventMeta::default(),
        })
        .await
        .unwrap();
        let held = card(&pool, thread).await;
        assert!(!held.warm && !held.cold);
        let edge = 400_000 + OFFER_DECLINE_HOLD_TOKENS;
        calls(&pool, thread, "claude-opus-5-5", edge).await;
        assert!(!card(&pool, thread).await.warm, "held at the edge");
        calls(&pool, thread, "claude-opus-5-5", edge + 1).await;
        assert!(card(&pool, thread).await.warm, "offered past it");
        teardown_test_db(&db).await;
    }

    /// I6: a thread holding a pending change gets no card.
    #[tokio::test]
    async fn an_open_change_shows_no_card() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        calls(&pool, thread, "claude-opus-5-5", 400_000).await;
        sqlx::query(
            "INSERT INTO changes (request_id, thread_id, branch_name, repo_root, status) \
             VALUES (gen_random_uuid(), $1, 'claude-code/x', '/repo', 'pending')",
        )
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
        let verdict = card(&pool, thread).await;
        assert!(!verdict.warm && !verdict.cold);

        teardown_test_db(&db).await;
    }
}
