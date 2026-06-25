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

/// The Saved section sorts by `last_user_action`, NOT `last_activity` — so a
/// thread the agent churned on more recently must still sort BELOW one the user
/// touched more recently. This is the "stop background agent churn reshuffling my
/// list" guarantee, which now lives on the Saved query (`get_saved_threads`);
/// Current + Archive moved to `created_at` (see the archive-window /
/// `get_older_threads` tests below). `churned` has the newer last_activity but
/// older last_user_action; `acted` is the opposite. Expected: acted, then churned.
#[tokio::test]
async fn get_saved_threads_sorts_by_last_user_action_not_last_activity() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let churned = Uuid::new_v4();
    let acted = Uuid::new_v4();
    // Both saved. churned: agent streamed 1 min ago, user last acted 1 day ago.
    // acted: user typed 1 hour ago, agent silent since.
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, is_saved, \
              last_activity, last_user_action, last_agent_action) \
         VALUES \
             ($1, 'Churned', 'claude_code', 1, TRUE, TRUE, \
              NOW() - INTERVAL '1 minute', NOW() - INTERVAL '1 day', NOW() - INTERVAL '1 minute'), \
             ($2, 'Acted',   'chat',        1, TRUE, TRUE, \
              NOW() - INTERVAL '1 hour',   NOW() - INTERVAL '1 hour', NOW() - INTERVAL '1 hour')",
    )
    .bind(churned)
    .bind(acted)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let saved = store.get_saved_threads().await.expect("get_saved_threads");
    let order: Vec<&str> = saved.iter().map(|t| t.thread_id.as_str()).collect();
    let acted_s = acted.to_string();
    let churned_s = churned.to_string();
    let acted_pos = order.iter().position(|id| *id == acted_s).expect("acted present");
    let churned_pos = order
        .iter()
        .position(|id| *id == churned_s)
        .expect("churned present");
    assert!(
        acted_pos < churned_pos,
        "the recently-USER-acted thread must sort above the recently-AGENT-churned one \
         (last_user_action drives the Saved order, not last_activity); got {order:?}"
    );

    teardown_test_db(&db).await;
}

/// The per-source Archive window (`get_recent_threads`' `ROW_NUMBER`) selects the
/// newest-`created_at` per source — the SAME axis the drawer's Archive section
/// sorts and `get_older_threads` pages by, so the window/page seam is gap-free.
/// With `per_source=1` and two archived threads where created_at diverges from
/// BOTH last_user_action and last_activity, only the newest-CREATED one makes the
/// cut. (Pre-fix the window ordered by last_user_action, which would have kept the
/// other one — the thread the user touched later but created earlier.)
#[tokio::test]
async fn get_recent_threads_archive_window_selects_by_created_at() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let newest_created = Uuid::new_v4(); // created last, never touched again
    let touched_later = Uuid::new_v4(); // created first, but user acted later
    // Same source + both archived idle, so only the rn<=per_source window decides
    // inclusion. created_at is the ONLY axis under which `newest_created` wins.
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, archive_state, \
              created_at, last_user_action, last_activity) \
         VALUES \
             ($1, 'Newest created', 'chat', 1, TRUE, 'archived', \
              TIMESTAMPTZ '2026-06-20 00:00:00Z', TIMESTAMPTZ '2026-06-11 00:00:00Z', TIMESTAMPTZ '2026-06-12 00:00:00Z'), \
             ($2, 'Touched later',  'chat', 1, TRUE, 'archived', \
              TIMESTAMPTZ '2026-06-10 00:00:00Z', TIMESTAMPTZ '2026-06-25 00:00:00Z', TIMESTAMPTZ '2026-06-26 00:00:00Z')",
    )
    .bind(newest_created)
    .bind(touched_later)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let recent = store.get_recent_threads(1).await.expect("get_recent_threads");
    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    assert!(
        returned.contains(newest_created.to_string().as_str()),
        "the newest-CREATED archived thread must be in the per-source window; got {} entries",
        recent.len()
    );
    assert!(
        !returned.contains(touched_later.to_string().as_str()),
        "the earlier-created thread must fall outside the window even though its \
         last_user_action / last_activity are newer (window orders by created_at)"
    );

    teardown_test_db(&db).await;
}

/// `get_older_threads` pages the Archive by `created_at` (cursor filter + order),
/// matching the drawer's Archive display sort so a recently-created-but-stale
/// thread can't page in late and go missing from the top. With three archived
/// threads whose created_at diverges from last_user_action, a `created_at` cursor
/// returns exactly the older-CREATED ones, newest-created first. (Pre-fix the
/// cursor filtered/ordered by last_user_action — a different, wrong set.)
#[tokio::test]
async fn get_older_threads_pages_by_created_at() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let after_cursor = Uuid::new_v4(); // created_at Jun 20 — newer than the cursor
    let mid = Uuid::new_v4(); //          created_at Jun 15 — older than the cursor
    let oldest = Uuid::new_v4(); //       created_at Jun 10 — older than the cursor
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, archive_state, \
              created_at, last_user_action) \
         VALUES \
             ($1, 'After',  'chat', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-20 00:00:00Z', TIMESTAMPTZ '2026-06-05 00:00:00Z'), \
             ($2, 'Mid',    'chat', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-15 00:00:00Z', TIMESTAMPTZ '2026-06-25 00:00:00Z'), \
             ($3, 'Oldest', 'chat', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-10 00:00:00Z', TIMESTAMPTZ '2026-06-12 00:00:00Z')",
    )
    .bind(after_cursor)
    .bind(mid)
    .bind(oldest)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    // Cursor at Jun 18 (created_at). created_at < Jun18 → {mid, oldest}, ordered
    // created_at DESC → [mid, oldest]. `after_cursor` (created Jun 20) is excluded.
    // A last_user_action cursor would instead return {after_cursor, oldest}.
    let before = chrono::DateTime::parse_from_rfc3339("2026-06-18T00:00:00Z")
        .expect("parse cursor")
        .with_timezone(&chrono::Utc);
    let older = store
        .get_older_threads(before, 15, None, None, None, None)
        .await
        .expect("get_older_threads");
    let order: Vec<&str> = older.iter().map(|t| t.thread_id.as_str()).collect();

    assert_eq!(
        order,
        vec![mid.to_string(), oldest.to_string()],
        "older page must be created_at < cursor, ordered created_at DESC; got {order:?}"
    );

    teardown_test_db(&db).await;
}

/// The two attributed-recency columns plumb through `get_threads_by_ids` (the
/// canonical single-summary read) onto `ThreadSummary`, and the camelCase wire
/// keys land on `ThreadAggregate` — those are what the frontend's
/// `meta.lastUserAction` (sort) and the row tooltip read.
#[tokio::test]
async fn thread_summary_and_aggregate_include_attributed_recency() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, \
              last_user_action, last_agent_action) \
         VALUES ($1, 'T', 'chat', 1, TRUE, NOW() - INTERVAL '2 hours', NOW() - INTERVAL '30 minutes')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let infos = store
        .get_threads_by_ids(&[id.to_string()])
        .await
        .expect("get_threads_by_ids");
    assert_eq!(infos.len(), 1);
    assert!(
        infos[0].last_user_action < infos[0].last_agent_action,
        "the agent acted more recently than the user in this fixture"
    );

    let agg = fetch_thread_aggregate(&pool, id)
        .await
        .expect("fetch_thread_aggregate")
        .expect("aggregate row exists");
    let json = serde_json::to_value(&agg).expect("serialize ThreadAggregate");
    assert!(
        json.get("lastUserAction").is_some(),
        "wire JSON must carry camelCase 'lastUserAction' (the frontend sort key); got {json}"
    );
    assert!(
        json.get("lastAgentAction").is_some(),
        "wire JSON must carry camelCase 'lastAgentAction' (the tooltip's Agent line); got {json}"
    );

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

/// `count_archived_threads` powers the collapsed Archive section's count badge.
/// It counts the archived pile — `archive_state='archived'` AND NOT saved — so
/// the badge shows the true total instead of the loaded window. A saved+archived
/// thread routes to the Saved section, not Archive, so it must NOT be counted;
/// inbox threads (saved or not) belong to Review, so they're excluded too.
#[tokio::test]
async fn count_archived_threads_counts_archived_unsaved_only() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // (archive_state, is_saved) → counted?
    //   ('archived', false) ×3  → yes
    //   ('archived', true)  ×1  → no  (shows in Saved)
    //   ('inbox',    false) ×1  → no  (shows in Review)
    //   ('inbox',    true)  ×1  → no  (shows in Saved)
    let rows: &[(&str, bool)] = &[
        ("archived", false),
        ("archived", false),
        ("archived", false),
        ("archived", true),
        ("inbox", false),
        ("inbox", true),
    ];
    for (i, (archive_state, is_saved)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, archive_state, is_saved) \
                 VALUES ($1, $2, 'chat', 1, NOW(), TRUE, 'idle', $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Thread {}", i))
        .bind(*archive_state)
        .bind(*is_saved)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    let count = store
        .count_archived_threads(None, None, None, None)
        .await
        .expect("count_archived_threads");

    assert_eq!(
        count, 3,
        "only archived + unsaved threads count toward the Archive badge total"
    );

    teardown_test_db(&db).await;
}

/// The Archive badge must "respect the filter": when the drawer is narrowed to a
/// channel (`sources`) or a repo facet (`repo_ids`), the count reflects only the
/// archived threads matching that filter — mirroring what scroll-pagination
/// surfaces. Without the filter params the badge undercounts (shows the loaded
/// window) or overcounts (shows the global total) under an active filter.
#[tokio::test]
async fn count_archived_threads_respects_source_and_repo_filters() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4().to_string();
    // (source, archived?, cc_repo_id) — all unsaved.
    //   chat archived       ×2
    //   claude_code archived ×3 (2 bound to repo_a, 1 to another repo)
    //   chat inbox          ×1 (excluded — not archived)
    let other_repo = Uuid::new_v4().to_string();
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("chat", "archived", None),
        ("chat", "archived", None),
        ("claude_code", "archived", Some(repo_a.as_str())),
        ("claude_code", "archived", Some(repo_a.as_str())),
        ("claude_code", "archived", Some(other_repo.as_str())),
        ("chat", "inbox", None),
    ];
    for (i, (source, archive_state, repo)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, archive_state, is_saved, cc_repo_id) \
                 VALUES ($1, $2, $3, 1, NOW(), TRUE, 'idle', $4, FALSE, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Thread {}", i))
        .bind(*source)
        .bind(*archive_state)
        .bind(*repo)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    // Unfiltered: 5 archived (2 chat + 3 claude_code).
    assert_eq!(
        store
            .count_archived_threads(None, None, None, None)
            .await
            .expect("count unfiltered"),
        5,
    );

    // Channel filter → only claude_code archived (3).
    let cc = vec!["claude_code".to_string()];
    assert_eq!(
        store
            .count_archived_threads(Some(&cc), None, None, None)
            .await
            .expect("count by source"),
        3,
        "source filter counts only archived threads on that channel"
    );

    // Repo facet narrows WITHIN the selected channel: claude_code is gated in by
    // `sources`, then the repo facet keeps only the 2 archived threads bound to
    // repo_a (the third claude_code thread is on another repo). The chat rows are
    // already excluded by the channel gate, so the union here is just repo_a's 2.
    let repos = vec![repo_a.clone()];
    assert_eq!(
        store
            .count_archived_threads(Some(&cc), None, Some(&repos), None)
            .await
            .expect("count by repo"),
        2,
        "repo facet counts only archived threads bound to that repo"
    );

    teardown_test_db(&db).await;
}

/// Regression for the "count is wrong for lucidos, cc and ONE trigger" report:
/// when whole channels are combined with a single facet, the badge must count
/// the UNION, mirroring the frontend `threadPassesChannelFilter` (channel gate,
/// THEN per-channel facet narrowing). The pre-fix facet branch ignored
/// `sources` entirely, so it counted ONLY the trigger's archived threads and
/// dropped every archived chat / claude_code thread — the badge read a tiny
/// number while the drawer listed far more.
#[tokio::test]
async fn count_archived_threads_channels_plus_facet_counts_union() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // All archived + unsaved. The two trigger rows split between the selected
    // trigger and another, so the per-trigger narrowing is observable.
    let rows: &[(&str, Option<&str>)] = &[
        ("chat", None),
        ("chat", None),
        ("claude_code", None),
        ("trigger", Some("trig-keep")),
        ("trigger", Some("trig-drop")),
    ];
    for (i, (source, trigger_id)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, archive_state, is_saved, trigger_id) \
                 VALUES ($1, $2, $3, 1, NOW(), TRUE, 'idle', 'archived', FALSE, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Thread {}", i))
        .bind(*source)
        .bind(*trigger_id)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    // chat + claude_code + trigger all selected → `sources` is None (the
    // all-channels case the frontend sends as `sources: undefined`), plus ONE
    // trigger sub-selected.
    let trig = vec!["trig-keep".to_string()];
    let count = store
        .count_archived_threads(None, Some(&trig), None, None)
        .await
        .expect("count channels + facet");

    assert_eq!(
        count, 4,
        "2 chat + 1 claude_code + 1 trig-keep — NOT just the single trigger match \
         (pre-fix: the facet branch ignored the channel selections)"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn count_archived_threads_empty_is_zero() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let count = store
        .count_archived_threads(None, None, None, None)
        .await
        .expect("count_archived_threads");
    assert_eq!(count, 0, "empty projection → zero archived");

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
