use super::*;
use super::test_helpers::*;

#[tokio::test]
async fn get_saved_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (parent, child) = insert_parent_child(&pool, true).await;

    let saved = store.get_saved_threads().await.expect("get_saved_threads");

    let row = saved
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in saved");
    assert_eq!(
        row.parent_thread_id.as_deref(),
        Some(parent.to_string().as_str())
    );
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

/// Regression test for fe5212ea: `get_recent_threads` wraps thread_summaries
/// in a derived table, so the parent_thread_title subquery must reference
/// the outer alias `t`, not the inner table name. Pre-fix code aliased the
/// outer as `ranked` but the subquery hardcoded `thread_summaries`, and
/// /api/v1/threads 500'd with "invalid reference to FROM-clause entry".
#[tokio::test]
async fn get_recent_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let recent = store
        .get_recent_threads(10)
        .await
        .expect("get_recent_threads");

    let row = recent
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in recent");
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_older_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let older = store
        .get_older_threads(cutoff, 10, None, None, None, None)
        .await
        .expect("get_older_threads");

    let row = older
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in older");
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

/// `get_recent_threads` must surface every thread that NEEDS user action
/// (`coding_agent_proposed=TRUE`, `status='waiting_for_user_answer'`, `status='failed'`)
/// even when the per-source `rn <= per_source` window would otherwise drop it.
///
/// REVIEW is a "needs attention" pile. Without this guarantee, a CC thread
/// pushed past the per-source window vanishes from the drawer entirely —
/// the user has no way to Apply/Discard the changes, no way to see them in
/// REVIEW, no Diff button. The `changes` data still exists in the DB but
/// the thread carrying it is invisible until the user manually scrolls
/// far enough to trigger `get_older_threads`.
///
/// Regression: 2026-04-25 dev workspace had four CC threads with pending
/// changes at rn=17, 18, 19, 40 — all hidden from /api/v1/threads.
#[tokio::test]
async fn get_recent_threads_always_includes_actionable_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 18 CC threads with descending last_activity. The three at i=15..17
    // carry actionable signals — each picks an inert status so the only
    // thing that lets it bypass the rn<=15 cap is the predicate under
    // test: coding_agent_proposed (#15), waiting_for_user_answer (#16),
    // failed (#17). One distinct second per row stabilizes the ranking.
    let now = chrono::Utc::now();
    let mut ids = Vec::with_capacity(18);
    for i in 0..18 {
        let id = Uuid::new_v4();
        ids.push(id);
        let last_activity = now - chrono::Duration::seconds(i as i64);
        let (status, coding_agent_proposed, section) = match i {
            15 => (ThreadStatus::Idle.as_str(), true, "inbox"),
            16 => (ThreadStatus::WaitingForUserAnswer.as_str(), false, "inbox"),
            17 => (ThreadStatus::Failed.as_str(), false, "inbox"),
            _ => (ThreadStatus::Idle.as_str(), false, "archived"),
        };
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, coding_agent_proposed, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, $4, $5, $6)",
        )
        .bind(id)
        .bind(format!("Thread {}", i))
        .bind(last_activity)
        .bind(status)
        .bind(coding_agent_proposed)
        .bind(section)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }
    let pending_changes = ids[15];
    let needs_answer = ids[16];
    let failed = ids[17];

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");

    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    let pending = pending_changes.to_string();
    let answer = needs_answer.to_string();
    let fail = failed.to_string();
    assert!(
            returned.contains(pending.as_str()),
            "thread with coding_agent_proposed=TRUE at rn>per_source must surface (Apply/Discard buttons live here); returned {} entries",
            recent.len()
        );
    assert!(
            returned.contains(answer.as_str()),
            "thread with status=waiting_for_user_answer at rn>per_source must surface (Question card lives here); returned {} entries",
            recent.len()
        );
    assert!(
            returned.contains(fail.as_str()),
            "thread with status=failed at rn>per_source must surface (error indicator lives here); returned {} entries",
            recent.len()
        );

    teardown_test_db(&db).await;
}

/// REVIEW must contain every inbox thread, not just the top-N per source.
/// An inbox row is one the user hasn't dismissed; capping it would silently
/// hide work — e.g. a CC thread whose subprocess crashed mid-flow without
/// emitting a terminal event keeps `coding_agent_proposed=false` and would be
/// gated out solely by recency.
#[tokio::test]
async fn get_recent_threads_returns_all_inbox_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 20 inert idle inbox CC threads. None carry an actionable signal,
    // so the only thing that can surface row 19 (rn=20, past the window
    // of 15) is the inbox bypass under test.
    let now = chrono::Utc::now();
    let mut ids = Vec::with_capacity(20);
    for i in 0..20 {
        let id = Uuid::new_v4();
        ids.push(id);
        let last_activity = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, coding_agent_proposed, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, 'idle', FALSE, 'inbox')",
        )
        .bind(id)
        .bind(format!("Inbox thread {}", i))
        .bind(last_activity)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }
    let furthest_back = ids[19];

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");

    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    let needed = furthest_back.to_string();
    assert!(
        returned.contains(needed.as_str()),
        "inbox thread at rn>per_source must surface; got {} entries",
        recent.len()
    );
    assert_eq!(
        recent.len(),
        20,
        "all 20 inbox threads must appear; got {}",
        recent.len()
    );

    teardown_test_db(&db).await;
}

/// Archive (archived threads) stays capped per source so the drawer
/// doesn't load the whole archive on refresh; `get_older_threads` pages
/// backward through what this omits.
#[tokio::test]
async fn get_recent_threads_caps_archived_threads_at_per_source() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 20 archived idle chats with no actionable signal — only the top 15
    // per source should come back.
    let now = chrono::Utc::now();
    for i in 0..20 {
        let last_activity = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, coding_agent_proposed, archive_state) \
                 VALUES ($1, $2, 'chat', 1, $3, TRUE, 'idle', FALSE, 'archived')",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Archived chat {}", i))
        .bind(last_activity)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");
    let chat_count = recent.iter().filter(|t| t.channel == "chat").count();
    assert_eq!(
        chat_count, 15,
        "archived threads must stay capped at per_source; got {}",
        chat_count
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_threads_by_ids_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let infos = store
        .get_threads_by_ids(&[child.to_string()])
        .await
        .expect("get_threads_by_ids");

    assert_eq!(infos.len(), 1);
    assert_eq!(
        infos[0].parent_thread_title.as_deref(),
        Some("Parent thread")
    );

    teardown_test_db(&db).await;
}

// -- coding_agent_has_diff plumbing through ThreadSummary / ThreadAggregate ----------

/// `get_threads_by_ids` (the canonical "fetch one summary" read path) returns
/// the new `coding_agent_has_diff` field populated from the `coding_agent_has_diff`
/// column. Without this, the wire format would drop the signal even though
/// the projection + sweep maintain the column reliably on the write side.
#[tokio::test]
async fn thread_summary_includes_coding_agent_has_diff() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, coding_agent_has_diff) \
             VALUES ($1, 'CC with diff', 'claude_code', 1, NOW(), TRUE, TRUE)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let infos = store
        .get_threads_by_ids(&[id.to_string()])
        .await
        .expect("get_threads_by_ids");

    assert_eq!(infos.len(), 1, "the seeded row must come back");
    assert!(
        infos[0].coding_agent_has_diff,
        "coding_agent_has_diff=TRUE in DB must surface as coding_agent_has_diff=true on ThreadSummary"
    );

    teardown_test_db(&db).await;
}

/// `fetch_thread_aggregate` returns `coding_agent_has_diff` populated from the
/// DB column, and the JSON serialization uses the `codingAgentHasDiff` camelCase
/// key — that's the field the frontend `meta.codingAgentHasDiff` reads.
#[tokio::test]
async fn thread_aggregate_includes_coding_agent_has_diff() {
    let (pool, db) = setup_test_db().await;

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, coding_agent_has_diff) \
             VALUES ($1, 'CC with diff', 'claude_code', 1, NOW(), TRUE, TRUE)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let agg = fetch_thread_aggregate(&pool, id)
        .await
        .expect("fetch_thread_aggregate")
        .expect("aggregate row exists");

    assert!(
        agg.coding_agent_has_diff,
        "coding_agent_has_diff=TRUE in DB must surface as coding_agent_has_diff=true on ThreadAggregate"
    );

    let json = serde_json::to_value(&agg).expect("serialize ThreadAggregate");
    assert_eq!(
        json.get("codingAgentHasDiff"),
        Some(&serde_json::json!(true)),
        "wire JSON must use camelCase key 'codingAgentHasDiff' (matches the frontend's meta.codingAgentHasDiff); got {}",
        json
    );

    teardown_test_db(&db).await;
}

/// `fetch_family_extension` returns ancestors and descendants of the base set
/// (NOT the base ids themselves) so the drawer can render the whole family
/// together — see `ThreadDrawer.tsx` → `categorizeThreads` / `nestByParent`,
/// which assume every family member is present in `threadMap` and silently
/// drop members that aren't.
#[tokio::test]
async fn fetch_family_extension_loads_ancestors_and_descendants() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let grandparent = Uuid::new_v4();
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    let unrelated = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
         VALUES ($1, 'GP', 'chat', 1, NOW() - INTERVAL '10 hours', TRUE, FALSE, NULL), \
                ($2, 'P',  'chat', 1, NOW() - INTERVAL '5 hours',  TRUE, FALSE, $1), \
                ($3, 'C',  'chat', 1, NOW() - INTERVAL '1 hour',   TRUE, FALSE, $2), \
                ($4, 'U',  'chat', 1, NOW(),                       TRUE, FALSE, NULL)",
    )
    .bind(grandparent)
    .bind(parent)
    .bind(child)
    .bind(unrelated)
    .execute(&pool)
    .await
    .expect("insert family");

    let extension = store
        .fetch_family_extension(&[parent.to_string()], i64::MAX)
        .await
        .expect("fetch_family_extension");

    let ids: std::collections::HashSet<String> =
        extension.iter().map(|t| t.thread_id.clone()).collect();
    assert!(ids.contains(&grandparent.to_string()), "ancestor not loaded");
    assert!(ids.contains(&child.to_string()), "descendant not loaded");
    assert!(
        !ids.contains(&parent.to_string()),
        "base id leaked into extension"
    );
    assert!(
        !ids.contains(&unrelated.to_string()),
        "unrelated thread leaked"
    );

    teardown_test_db(&db).await;
}

/// Reproduces the user-reported bug: a nightly trigger with 4 child CC
/// sessions where 3 are recent and 1 is older than the pagination cursor.
/// After natural pagination loads the trigger + 3 recent children,
/// `fetch_family_extension` must surface the 4th (older) child so the
/// drawer's "4/4 done" badge matches what gets nested under the parent.
#[tokio::test]
async fn fetch_family_extension_loads_child_below_pagination_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let trigger = Uuid::new_v4();
    let recent_a = Uuid::new_v4();
    let recent_b = Uuid::new_v4();
    let recent_c = Uuid::new_v4();
    let old_child = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
         VALUES ($1, 'Trigger',  'trigger',     1, NOW() - INTERVAL '12 hours', TRUE, FALSE, NULL), \
                ($2, 'Recent A', 'claude_code', 1, NOW() - INTERVAL '1 minute', TRUE, FALSE, $1), \
                ($3, 'Recent B', 'claude_code', 1, NOW() - INTERVAL '2 minute', TRUE, FALSE, $1), \
                ($4, 'Recent C', 'claude_code', 1, NOW() - INTERVAL '3 minute', TRUE, FALSE, $1), \
                ($5, 'Old',      'claude_code', 1, NOW() - INTERVAL '24 hours', TRUE, FALSE, $1)",
    )
    .bind(trigger)
    .bind(recent_a)
    .bind(recent_b)
    .bind(recent_c)
    .bind(old_child)
    .execute(&pool)
    .await
    .expect("insert nightly family");

    let base_ids: Vec<String> = vec![
        trigger.to_string(),
        recent_a.to_string(),
        recent_b.to_string(),
        recent_c.to_string(),
    ];

    let extension = store
        .fetch_family_extension(&base_ids, i64::MAX)
        .await
        .expect("fetch_family_extension");

    let ids: std::collections::HashSet<String> =
        extension.iter().map(|t| t.thread_id.clone()).collect();
    assert_eq!(
        ids.len(),
        1,
        "exactly one family member (the old child) should be loaded; got {:?}",
        ids
    );
    assert!(
        ids.contains(&old_child.to_string()),
        "old_child missing from family extension"
    );

    teardown_test_db(&db).await;
}

/// Empty base set returns empty — no query overhead, no error.
#[tokio::test]
async fn fetch_family_extension_empty_base_returns_empty() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let extension = store
        .fetch_family_extension(&[], i64::MAX)
        .await
        .expect("fetch_family_extension on empty input");

    assert!(extension.is_empty());

    teardown_test_db(&db).await;
}

/// Parent cycle (data corruption): A→B→A. Recursive CTE uses UNION (dedup)
/// so it terminates. Family extension still surfaces the other cycle member.
#[tokio::test]
async fn fetch_family_extension_terminates_on_cycle() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, parent_thread_id) \
         VALUES ($1, 'A', 'chat', 1, NOW(), TRUE, $2), \
                ($2, 'B', 'chat', 1, NOW(), TRUE, $1)",
    )
    .bind(a)
    .bind(b)
    .execute(&pool)
    .await
    .expect("insert cycle");

    let extension = store
        .fetch_family_extension(&[a.to_string()], i64::MAX)
        .await
        .expect("fetch_family_extension on cycle");

    let ids: std::collections::HashSet<String> =
        extension.iter().map(|t| t.thread_id.clone()).collect();
    assert!(ids.contains(&b.to_string()), "cycle peer must be surfaced");
    assert!(
        !ids.contains(&a.to_string()),
        "base id stays excluded even in cycle"
    );

    teardown_test_db(&db).await;
}

/// `max_family` caps the result, keeping the newest members first. Without
/// this cap a pathological fan-out root (one chat spawning hundreds of CC /
/// trigger children) balloons the initial `/api/v1/threads` payload — the bug
/// that motivated the cap was personal-ws hitting 401 family threads behind
/// 24 base rows.
#[tokio::test]
async fn fetch_family_extension_respects_max_family() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let parent = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
         VALUES ($1, 'P', 'chat', 1, NOW() - INTERVAL '1 day', TRUE, FALSE, NULL)",
    )
    .bind(parent)
    .execute(&pool)
    .await
    .expect("insert parent");

    let mut children: Vec<Uuid> = (0..10).map(|_| Uuid::new_v4()).collect();
    for (i, child) in children.iter().enumerate() {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
             VALUES ($1, $2, 'chat', 1, NOW() - (($3 || ' minutes')::interval), TRUE, FALSE, $4)",
        )
        .bind(child)
        .bind(format!("Child {}", i))
        .bind(i as i32)
        .bind(parent)
        .execute(&pool)
        .await
        .expect("insert child");
    }
    // children[0] is newest (0 minutes ago); children[9] is oldest.
    children.reverse(); // now [oldest .. newest]

    let extension = store
        .fetch_family_extension(&[parent.to_string()], 3)
        .await
        .expect("fetch_family_extension capped");

    assert_eq!(extension.len(), 3, "cap of 3 must be honored");
    let kept: std::collections::HashSet<String> =
        extension.iter().map(|t| t.thread_id.clone()).collect();
    // The 3 newest are children[7], children[8], children[9] (after reverse).
    for newest in children.iter().rev().take(3) {
        assert!(
            kept.contains(&newest.to_string()),
            "cap must keep newest first; missing {}",
            newest
        );
    }

    teardown_test_db(&db).await;
}
