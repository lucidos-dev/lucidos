use super::*;
use crate::engine::event_bus::EventBus;
use crate::test_support::{seed_coding_thread, seed_handoff, setup_test_db, teardown_test_db};

const OPUS: &str = "claude-opus-5-5";

/// Insert one event on `thread_id` and return its sequence.
async fn insert(
    pool: &PgPool,
    thread_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> (Uuid, i64) {
    sqlx::query_as(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), $1, $2, $3, 'thread', $3::text) RETURNING id, sequence",
    )
    .bind(event_type)
    .bind(payload)
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn calls(pool: &PgPool, thread_id: Uuid, n: usize) {
    for _ in 0..n {
        insert(
            pool,
            thread_id,
            "ContextCaptured",
            serde_json::json!({"model": OPUS, "usage": {"input_tokens": 50_000}}),
        )
        .await;
    }
}

/// A finished turn: its `ResponseGenerated`, then its idle.
async fn idle(pool: &PgPool, thread_id: Uuid) {
    insert(pool, thread_id, "ResponseGenerated", serde_json::json!({})).await;
    insert(pool, thread_id, "CodingAgentIdled", serde_json::json!({})).await;
}

/// A checkpoint run on `thread_id`, as `record_checkpoint` writes one.
async fn checkpoint(
    pool: &PgPool,
    thread_id: Uuid,
    name: &str,
    tokens: u64,
    progress: Option<(u32, u32)>,
    effort: Option<&str>,
) -> i64 {
    let mut payload = serde_json::json!({
        "name": name, "context_tokens": tokens, "model": format!("{OPUS}[1m]"),
        "reasoning_effort": effort,
    });
    if let Some((done, total)) = progress {
        payload["progress"] = serde_json::json!({"done": done, "total": total});
    }
    insert(pool, thread_id, "HandoffCheckpointReached", payload)
        .await
        .1
}

/// I5: a question card, an event wait or an unread message holds the turn
/// end open, and so does a turn that did not finish. A wait armed before the
/// checkpoint counts too. The sample runs to the first idle with none.
#[tokio::test]
async fn a_sample_ends_at_the_first_clean_turn_end() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let (first_message, _) = insert(
        &pool,
        thread,
        "MessageReceived",
        serde_json::json!({"text": "go"}),
    )
    .await;
    insert(
        &pool,
        thread,
        "CodingAgentInputRead",
        serde_json::json!({"input_event_id": first_message}),
    )
    .await;
    let wait = Uuid::new_v4();
    insert(
        &pool,
        thread,
        "EventWaitStarted",
        serde_json::json!({"wait_id": wait}),
    )
    .await;
    let start = checkpoint(&pool, thread, "pre-harden", 300_000, None, None).await;
    assert_eq!(sample_calls(&pool, thread, start).await.unwrap(), None);

    calls(&pool, thread, 2).await;
    insert(&pool, thread, "ResponseGenerated", serde_json::json!({})).await;
    let question =
        serde_json::json!({"tool_use_id": "toolu_q", "cc_session_id": "s", "question": "?"});
    insert(&pool, thread, "UserQuestionAsked", question).await;
    insert(&pool, thread, "CodingAgentIdled", serde_json::json!({})).await;
    calls(&pool, thread, 1).await;
    let answer =
        serde_json::json!({"tool_use_id": "toolu_q", "answer": {"kind": "text", "text": "y"}});
    insert(&pool, thread, "UserQuestionAnswered", answer).await;

    let stop = serde_json::json!({"cause": "user_stop"});
    insert(&pool, thread, "ResponseCanceled", stop).await;
    insert(&pool, thread, "CodingAgentIdled", serde_json::json!({})).await;
    calls(&pool, thread, 1).await;

    idle(&pool, thread).await;
    calls(&pool, thread, 1).await;
    insert(
        &pool,
        thread,
        "EventWaitDelivered",
        serde_json::json!({"wait_id": wait}),
    )
    .await;

    let (queued, _) = insert(
        &pool,
        thread,
        "MessageReceived",
        serde_json::json!({"text": "also"}),
    )
    .await;
    idle(&pool, thread).await;
    insert(
        &pool,
        thread,
        "CodingAgentIdled",
        serde_json::json!({"reason": KEEP_ALIVE}),
    )
    .await;
    calls(&pool, thread, 1).await;
    insert(
        &pool,
        thread,
        "CodingAgentInputRead",
        serde_json::json!({"input_event_id": queued}),
    )
    .await;
    assert_eq!(
        sample_calls(&pool, thread, start).await.unwrap(),
        None,
        "every idle so far held something open"
    );
    idle(&pool, thread).await;
    calls(&pool, thread, 9).await;
    assert_eq!(sample_calls(&pool, thread, start).await.unwrap(), Some(6));

    teardown_test_db(&db).await;
}

const KEEP_ALIVE: &str = crate::engine::thread_events::KEEP_ALIVE_IDLE_REASON;

/// I5: a thread that hands off before its turn ends counts its successor's
/// calls, up to the successor's own clean turn end. The old thread counts up
/// to the spawn: what it does after is not the work.
#[tokio::test]
async fn a_sample_follows_the_handoff() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let start = checkpoint(&pool, thread, "pre-harden", 300_000, None, None).await;
    calls(&pool, thread, 3).await;
    let successor = seed_handoff(&bus, thread, None).await;
    calls(&pool, thread, 1).await;
    idle(&pool, thread).await;
    calls(&pool, successor, 6).await;
    assert_eq!(
        sample_calls(&pool, thread, start).await.unwrap(),
        None,
        "the successor has not finished"
    );
    idle(&pool, successor).await;
    assert_eq!(sample_calls(&pool, thread, start).await.unwrap(), Some(9));

    teardown_test_db(&db).await;
}

/// Sample continuity: a handoff inside a sample leaves it whole. The old
/// session's idle is no clean turn end, so the sample runs on into the
/// successor session up to its own.
#[tokio::test]
async fn a_sample_runs_on_across_a_successor_session() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let start = checkpoint(&pool, thread, "pre-harden", 300_000, None, None).await;
    calls(&pool, thread, 3).await;
    insert(&pool, thread, "SessionHandedOff", serde_json::json!({})).await;
    idle(&pool, thread).await;
    assert_eq!(
        sample_calls(&pool, thread, start).await.unwrap(),
        None,
        "the handed-off session's idle ends no sample"
    );
    insert(
        &pool,
        thread,
        "SessionStarted",
        serde_json::json!({"continuity": {"kind": "handoff", "opening": "brief"}}),
    )
    .await;
    calls(&pool, thread, 4).await;
    idle(&pool, thread).await;
    assert_eq!(sample_calls(&pool, thread, start).await.unwrap(), Some(7));

    teardown_test_db(&db).await;
}

/// One finished run of `name` on a fresh thread, entering at `tokens`, that
/// took `n` calls for its units.
async fn finished_run(
    pool: &PgPool,
    bus: &EventBus,
    name: &str,
    tokens: u64,
    progress: Option<(u32, u32)>,
    effort: Option<&str>,
    n: usize,
) -> Uuid {
    let thread = seed_coding_thread(bus, None).await;
    checkpoint(pool, thread, name, tokens, progress, effort).await;
    calls(pool, thread, n).await;
    idle(pool, thread).await;
    thread
}

fn key(name: &str, effort: Option<&str>) -> HistoryKey {
    HistoryKey {
        repo: None,
        name: name.into(),
        model: OPUS.into(),
        reasoning_effort: effort.map(str::to_string),
    }
}

/// I6: the size band first, all sizes under 3 band samples, `None` under 3
/// samples of any size. Progress divides each sample by its units.
#[tokio::test]
async fn the_median_reads_the_band_then_all_sizes() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let per_unit = |tokens| {
        let key = key("phase-commit", Some("high"));
        let pool = pool.clone();
        async move { calls_per_unit(&pool, &key, tokens).await.unwrap() }
    };
    for n in [10, 20] {
        finished_run(&pool, &bus, "phase-commit", 60_000, None, Some("high"), n).await;
    }
    assert_eq!(per_unit(60_000).await, None, "two samples are too few");
    finished_run(
        &pool,
        &bus,
        "phase-commit",
        60_000,
        Some((1, 3)),
        Some("high"),
        60,
    )
    .await;
    assert_eq!(per_unit(60_000).await, Some(20.0), "60 calls over 2 units");
    assert_eq!(
        per_unit(400_000).await,
        Some(20.0),
        "no band samples at 400k, so all sizes"
    );
    for n in [40, 50, 60] {
        finished_run(&pool, &bus, "phase-commit", 300_000, None, Some("high"), n).await;
    }
    assert_eq!(
        per_unit(400_000).await,
        Some(50.0),
        "the band at 200k to 800k"
    );
    assert_eq!(
        per_unit(60_000).await,
        Some(20.0),
        "the band at 30k to 120k"
    );

    teardown_test_db(&db).await;
}

const APP_FOLDER: &str = "/home/user/workspaces/myws/data/apps/habit-tracker";

/// I6: another repository, app, name, model or effort never joins the pool.
#[tokio::test]
async fn the_median_pools_only_on_its_own_key() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for n in [10, 10, 10] {
        finished_run(&pool, &bus, "pre-harden", 300_000, None, Some("high"), n).await;
    }
    for n in [90, 90, 90] {
        finished_run(&pool, &bus, "phase-commit", 300_000, None, Some("high"), n).await;
        finished_run(&pool, &bus, "pre-harden", 300_000, None, Some("medium"), n).await;
        finished_run(&pool, &bus, "pre-harden", 300_000, None, None, n).await;
        let elsewhere =
            finished_run(&pool, &bus, "pre-harden", 300_000, None, Some("high"), n).await;
        sqlx::query("UPDATE thread_summaries SET cc_repo_id = 'example-repo' WHERE thread_id = $1")
            .bind(elsewhere)
            .execute(&pool)
            .await
            .unwrap();
        let app = finished_run(&pool, &bus, "pre-harden", 300_000, None, Some("high"), n).await;
        sqlx::query("UPDATE thread_summaries SET coding_agent_folder = $2 WHERE thread_id = $1")
            .bind(app)
            .bind(APP_FOLDER)
            .execute(&pool)
            .await
            .unwrap();
        let sonnet = seed_coding_thread(&bus, None).await;
        insert(
            &pool,
            sonnet,
            "HandoffCheckpointReached",
            serde_json::json!({"name": "pre-harden", "context_tokens": 300_000,
                               "model": "claude-sonnet-5-5", "reasoning_effort": "high"}),
        )
        .await;
        calls(&pool, sonnet, n).await;
        idle(&pool, sonnet).await;
    }
    assert_eq!(
        calls_per_unit(&pool, &key("pre-harden", Some("high")), 300_000)
            .await
            .unwrap(),
        Some(10.0)
    );
    let repo_key = HistoryKey {
        repo: Some("example-repo".into()),
        ..key("pre-harden", Some("high"))
    };
    assert_eq!(
        calls_per_unit(&pool, &repo_key, 300_000).await.unwrap(),
        Some(90.0),
        "the other repository pools its own runs"
    );
    let app_key = HistoryKey {
        repo: Some(APP_FOLDER.into()),
        ..key("pre-harden", Some("high"))
    };
    assert_eq!(
        calls_per_unit(&pool, &app_key, 300_000).await.unwrap(),
        Some(90.0),
        "an app thread pools on its folder"
    );

    teardown_test_db(&db).await;
}

/// The fresh median reads each successor's tenth main-session call.
#[tokio::test]
async fn the_fresh_median_reads_the_tenth_call_of_recent_successors() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let at = |thread, tokens| {
        let pool = pool.clone();
        async move {
            insert(
                &pool,
                thread,
                "ContextCaptured",
                serde_json::json!({"model": OPUS, "usage": {"input_tokens": tokens}}),
            )
            .await;
        }
    };
    for tokens in [70_000, 80_000, 90_000] {
        let old = seed_coding_thread(&bus, None).await;
        let successor = seed_handoff(&bus, old, None).await;
        for _ in 0..9 {
            at(successor, 1_000).await;
        }
        at(successor, tokens).await;
        at(successor, 500_000).await;
    }
    let short = seed_handoff(&bus, seed_coding_thread(&bus, None).await, None).await;
    at(short, 1_000).await;
    assert_eq!(fresh_tokens_median(&pool).await.unwrap(), Some(80_000.0));

    teardown_test_db(&db).await;
}

/// A successor session's fresh context counts from its own start, never from
/// the calls of the session that handed off.
#[tokio::test]
async fn the_fresh_median_reads_the_tenth_call_of_a_successor_session() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let at = |thread, tokens| {
        let pool = pool.clone();
        async move {
            insert(
                &pool,
                thread,
                "ContextCaptured",
                serde_json::json!({"model": OPUS, "usage": {"input_tokens": tokens}}),
            )
            .await;
        }
    };
    for tokens in [60_000, 70_000, 75_000] {
        let thread = seed_coding_thread(&bus, None).await;
        for _ in 0..12 {
            at(thread, 400_000).await;
        }
        insert(
            &pool,
            thread,
            "SessionStarted",
            serde_json::json!({"continuity": {"kind": "handoff", "opening": "narrative"}}),
        )
        .await;
        for _ in 0..9 {
            at(thread, 1_000).await;
        }
        at(thread, tokens).await;
    }
    assert_eq!(fresh_tokens_median(&pool).await.unwrap(), Some(70_000.0));

    teardown_test_db(&db).await;
}

#[test]
fn the_median_of_an_even_count_averages_the_middle_pair() {
    assert_eq!(median_of_recent(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    assert_eq!(median_of_recent(&[1.0, 2.0]), None);
}
