use super::*;
use crate::llm::model_prices::CacheShares;

fn opus() -> ModelPrice {
    price_of("claude-opus-5-5").expect("priced")
}

/// I2: any well-formed name is a checkpoint, and the engine knows none.
#[test]
fn any_well_formed_name_is_a_checkpoint() {
    for name in ["pre-harden", "root-cause", "x", "phase-2", "9lives"] {
        assert_eq!(CheckpointName::parse(name).unwrap().as_str(), name);
    }
    let long = "a".repeat(65);
    for name in [
        "",
        "-lead",
        "Upper",
        "with space",
        "under_score",
        "dot.ted",
        &long,
    ] {
        assert!(CheckpointName::parse(name).is_err(), "{name:?}");
    }
}

#[test]
fn the_progress_parses_and_counts_the_units_left() {
    let run = |progress| CheckpointRun::parse("phase-commit", progress);
    assert_eq!(run(Some("2/5")).unwrap().units_left(), 3);
    assert_eq!(run(Some("0/4")).unwrap().units_left(), 4);
    assert_eq!(run(Some("3/3")).unwrap().units_left(), 0);
    assert_eq!(
        run(None).unwrap().units_left(),
        1,
        "no progress is one unit"
    );
    for progress in ["4/3", "0/0", "two of three", "3", "-1/3"] {
        assert!(run(Some(progress)).is_err(), "{progress}");
    }
}

/// I2: a successor's title names the checkpoint as the flow passed it.
#[test]
fn a_checkpoint_labels_its_successor_title_by_its_own_name() {
    let label = |progress| {
        CheckpointRun::parse("phase-commit", progress)
            .unwrap()
            .title_label()
    };
    assert_eq!(label(Some("2/5")), "phase-commit 2/5");
    assert_eq!(label(None), "phase-commit");
}

/// I4: never at or under the floor, whatever the calls left.
#[test]
fn the_floor_holds_at_any_call_count() {
    let at_floor = estimate(
        HANDOFF_FLOOR_TOKENS,
        &opus(),
        1,
        10_000.0,
        10_000.0,
        CacheWarmth::Warm,
    );
    assert!(!at_floor.pays);
    assert!(
        at_floor.saving_usd > at_floor.cost_usd,
        "only the floor refuses"
    );
    assert!(
        estimate(
            HANDOFF_FLOOR_TOKENS + 1,
            &opus(),
            1,
            10_000.0,
            10_000.0,
            CacheWarmth::Warm
        )
        .pays
    );
}

/// I3: calls left are units left times calls per unit.
#[test]
fn calls_left_are_units_times_calls_per_unit() {
    let reading = estimate(200_000, &opus(), 3, 30.0, 60_000.0, CacheWarmth::Warm);
    assert_eq!(reading.calls_left, 90.0);
    assert_eq!(reading.units_left, 3);
    assert_eq!(reading.calls_per_unit, 30.0);
    assert!(
        !estimate(400_000, &opus(), 0, 30.0, 60_000.0, CacheWarmth::Warm).pays,
        "no units left"
    );
}

/// The measured cases: 272k at 21 calls does not pay, at 38 it does.
#[test]
fn the_saving_weighs_the_gap_to_a_fresh_context() {
    assert!(!estimate(272_000, &opus(), 1, 21.0, 220_000.0, CacheWarmth::Warm).pays);
    assert!(estimate(272_000, &opus(), 1, 38.0, 116_000.0, CacheWarmth::Warm).pays);
    let smaller_than_fresh = estimate(110_000, &opus(), 1, 21.0, 116_000.0, CacheWarmth::Warm);
    assert_eq!(smaller_than_fresh.saving_usd, 0.0);
    assert!(!smaller_than_fresh.pays);
}

/// The price cancels out: only the cache shares move the verdict. A cheaper
/// read share needs more calls to pay.
#[test]
fn a_cheaper_read_share_needs_more_calls() {
    let fable = price_of("claude-fable-5-1").expect("priced");
    assert!(estimate(150_000, &opus(), 1, 40.0, 60_000.0, CacheWarmth::Warm).pays);
    assert!(!estimate(150_000, &fable, 1, 40.0, 60_000.0, CacheWarmth::Warm).pays);
}

/// A price with round numbers, so the sums below read off by hand: a cached
/// read costs 0.1 a token, a write 1, and an output token 1.
fn round_price() -> ModelPrice {
    let mut price = opus();
    price.input = 1e6;
    price.output = 1e6;
    price.cache = CacheShares {
        read: 0.1,
        write_5m: 1.0,
        write_1h: 1.0,
    };
    price.flex = None;
    price.long = None;
    price
}

/// I7: a warm handoff pays for the narrative turn: the old context reread at
/// the read price, and the narrative written.
#[test]
fn a_warm_handoff_prices_the_narrative_turn() {
    let warm = estimate(
        200_000,
        &round_price(),
        1,
        10.0,
        50_000.0,
        CacheWarmth::Warm,
    );
    assert_eq!(warm.saving_usd, 10.0 * 150_000.0 * 0.1);
    assert_eq!(
        warm.cost_usd,
        50_000.0 + 200_000.0 * 0.1 + NARRATIVE_OUTPUT_TOKENS
    );
    assert!(warm.pays);
    assert_eq!(warm.warmth, CacheWarmth::Warm);
}

/// I7: a cold handoff costs only the successor's context, and staying pays one
/// full rewrite of the old one.
#[test]
fn a_cold_handoff_saves_the_rewrite_of_staying() {
    let cold = estimate(
        200_000,
        &round_price(),
        1,
        10.0,
        50_000.0,
        CacheWarmth::Cold,
    );
    assert_eq!(cold.saving_usd, 10.0 * 150_000.0 * 0.1 + 200_000.0);
    assert_eq!(cold.cost_usd, 50_000.0);
    assert!(cold.pays);
}

/// I7: a handoff too few calls repay while the cache is warm pays once it is
/// cold, because the next call would rewrite the whole context.
#[test]
fn the_same_context_pays_cold_and_not_warm() {
    let few = |warmth| estimate(200_000, &round_price(), 1, 4.0, 50_000.0, warmth);
    assert!(!few(CacheWarmth::Warm).pays, "60k saved against 74k spent");
    assert!(few(CacheWarmth::Cold).pays, "260k saved against 50k spent");
}

/// I5: a decline hides the card for 100k tokens past its size, and a context
/// that shrank below it lifts the hold.
#[test]
fn a_decline_holds_the_card_for_a_hundred_thousand_tokens() {
    assert!(!offer_held_back(None, 500_000));
    let held = |current| offer_held_back(Some(300_000), current);
    assert!(held(300_000));
    assert!(held(300_000 + OFFER_DECLINE_HOLD_TOKENS));
    assert!(!held(300_000 + OFFER_DECLINE_HOLD_TOKENS + 1));
    assert!(!held(80_000), "a compaction lifts the hold");
}

mod db {
    use super::super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

    async fn calls(pool: &PgPool, thread_id: Uuid, model: &str, tokens: u64) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
        )
        .bind(serde_json::json!({"model": model, "usage": {"input_tokens": tokens, "cache_creation_tokens": 1000}}))
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    }

    fn pre_harden() -> CheckpointRun {
        CheckpointRun::parse("pre-harden", None).unwrap()
    }

    /// I3: an unpriced model, or a thread with no reading, gives no verdict.
    #[tokio::test]
    async fn an_unpriced_model_gives_no_verdict() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let run = pre_harden();
        let verdict = || handoff_verdict(&pool, thread, &run);
        assert_eq!(verdict().await.unwrap().pays, None, "no reading yet");
        calls(&pool, thread, "my-local-model", 400_000).await;
        assert_eq!(verdict().await.unwrap().pays, None);
        calls(&pool, thread, "claude-opus-5-5", 400_000).await;
        let paying = verdict().await.unwrap();
        assert_eq!(paying.pays, Some(true));
        assert_eq!(
            paying.estimate.unwrap().calls_per_unit,
            CALLS_PER_UNIT_FALLBACK,
            "no history reads the default"
        );

        teardown_test_db(&db).await;
    }

    /// I4: a pending change may be the one an Apply waits to harden, and a
    /// handoff would withdraw it, so the verdict never pays then.
    #[tokio::test]
    async fn a_pending_change_never_pays() {
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
        let verdict = handoff_verdict(&pool, thread, &pre_harden()).await.unwrap();
        assert_eq!(verdict.pays, Some(false));

        teardown_test_db(&db).await;
    }

    /// A run is recorded with what the estimate read, so a later median
    /// pools on it.
    #[tokio::test]
    async fn a_checkpoint_run_records_what_it_read() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        calls(&pool, thread, "claude-opus-5-5[1m]", 400_000).await;
        let run = CheckpointRun::parse("plan-approval", Some("0/4")).unwrap();
        let verdict = record_checkpoint(&bus, &pool, thread, &run, true, None)
            .await
            .unwrap();
        assert_eq!(verdict.pays, Some(true));
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events \
              WHERE thread_id = $1 AND event_type = 'HandoffCheckpointReached'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(payload["name"], "plan-approval");
        assert_eq!(
            payload["progress"],
            serde_json::json!({"done": 0, "total": 4})
        );
        assert_eq!(payload["context_tokens"], 400_000);
        assert_eq!(payload["model"], "claude-opus-5-5[1m]");
        assert_eq!(payload["offered"], true);
        assert_eq!(payload["pays"], true);

        teardown_test_db(&db).await;
    }

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

/// I2: the engine reads warmth from the seconds the cache has left.
#[test]
fn a_cache_with_time_left_is_warm_and_any_other_is_cold() {
    assert_eq!(CacheWarmth::of_remaining_secs(Some(1)), CacheWarmth::Warm);
    assert_eq!(CacheWarmth::of_remaining_secs(None), CacheWarmth::Cold);
}
