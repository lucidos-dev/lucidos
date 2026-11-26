use super::test_helpers::*;
use super::*;

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

#[tokio::test]
async fn get_saved_threads_sorts_by_last_user_action_not_last_activity() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let churned = Uuid::new_v4();
    let acted = Uuid::new_v4();
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
    let acted_pos = order
        .iter()
        .position(|id| *id == acted_s)
        .expect("acted present");
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

#[tokio::test]
async fn get_recent_threads_archive_window_selects_by_created_at() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let newest_created = Uuid::new_v4();
    let touched_later = Uuid::new_v4();
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

    let recent = store
        .get_recent_threads(1)
        .await
        .expect("get_recent_threads");
    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    assert!(
        returned.contains(newest_created.to_string().as_str()),
        "the newest-CREATED archived thread must be in the archive window; got {} entries",
        recent.len()
    );
    assert!(
        !returned.contains(touched_later.to_string().as_str()),
        "the earlier-created thread must fall outside the window even though its \
         last_user_action / last_activity are newer (window orders by created_at)"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_older_threads_pages_by_created_at() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let after_cursor = Uuid::new_v4();
    let mid = Uuid::new_v4();
    let oldest = Uuid::new_v4();
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

#[tokio::test]
async fn get_recent_threads_excludes_actionable_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let now = chrono::Utc::now();
    for i in 0..15 {
        let created_at = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, created_at, last_activity, \
                  has_response, status, coding_agent_change_state, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, $3, TRUE, 'idle', 'none', 'archived')",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Window thread {}", i))
        .bind(created_at)
        .execute(&pool)
        .await
        .expect("insert window thread");
    }

    let actionable: [(Uuid, &str, bool, i64); 3] = [
        (Uuid::new_v4(), ThreadStatus::Idle.as_str(), true, 100),
        (
            Uuid::new_v4(),
            ThreadStatus::WaitingForUserAnswer.as_str(),
            false,
            101,
        ),
        (Uuid::new_v4(), ThreadStatus::Failed.as_str(), false, 102),
    ];
    for (id, status, proposed, secs) in actionable {
        let created_at = now - chrono::Duration::seconds(secs);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, created_at, last_activity, \
                  has_response, status, coding_agent_change_state, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, $3, TRUE, $4, $5, 'archived')",
        )
        .bind(id)
        .bind(format!("Beyond {}", status))
        .bind(created_at)
        .bind(status)
        .bind(if proposed { "proposed" } else { "none" })
        .execute(&pool)
        .await
        .expect("insert beyond-window actionable thread");
    }

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");

    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    for (id, status, _, _) in actionable {
        let id = id.to_string();
        assert!(
            !returned.contains(id.as_str()),
            "archived {status} thread beyond the window must NOT be injected (reachable only via pagination); got {} entries",
            recent.len()
        );
    }
    assert_eq!(
        recent.len(),
        15,
        "exactly the 15 window rows come back; got {}",
        recent.len()
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_recent_threads_returns_all_inbox_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let now = chrono::Utc::now();
    let mut ids = Vec::with_capacity(20);
    for i in 0..20 {
        let id = Uuid::new_v4();
        ids.push(id);
        let last_activity = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, coding_agent_change_state, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, 'idle', 'none', 'inbox')",
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
        "inbox thread at rn>archive_limit must surface; got {} entries",
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

#[tokio::test]
async fn get_recent_threads_returns_inbox_proposed_without_response() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, \
              status, coding_agent_change_state, archive_state) \
             VALUES ($1, 'Proposed inbox', 'claude_code', 1, FALSE, 'idle', 'proposed', 'inbox')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert inbox proposed thread");

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");
    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    assert!(
        returned.contains(id.to_string().as_str()),
        "inbox proposed thread with no response must surface; got {} entries",
        recent.len()
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_recent_threads_caps_archived_threads_at_limit() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let now = chrono::Utc::now();
    for i in 0..20 {
        let created_at = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, created_at, last_activity, has_response, \
                  status, coding_agent_change_state, archive_state) \
                 VALUES ($1, $2, 'chat', 1, $3, $3, TRUE, 'idle', 'none', 'archived')",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Archived chat {}", i))
        .bind(created_at)
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
        "archived threads must stay capped at archive_limit; got {}",
        chat_count
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_recent_threads_archive_window_is_global_not_per_source() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let chat_new = Uuid::new_v4();
    let chat_mid = Uuid::new_v4();
    let cc_old = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, archive_state, created_at, last_activity) \
         VALUES \
             ($1, 'Chat new', 'chat',        1, TRUE, 'archived', TIMESTAMPTZ '2026-06-25 00:00:00Z', TIMESTAMPTZ '2026-06-25 00:00:00Z'), \
             ($2, 'Chat mid', 'chat',        1, TRUE, 'archived', TIMESTAMPTZ '2026-06-24 00:00:00Z', TIMESTAMPTZ '2026-06-24 00:00:00Z'), \
             ($3, 'CC old',   'claude_code', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-10 00:00:00Z', TIMESTAMPTZ '2026-06-10 00:00:00Z')",
    )
    .bind(chat_new)
    .bind(chat_mid)
    .bind(cc_old)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let recent = store
        .get_recent_threads(2)
        .await
        .expect("get_recent_threads");
    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    assert!(
        returned.contains(chat_new.to_string().as_str())
            && returned.contains(chat_mid.to_string().as_str()),
        "the two newest-CREATED archived threads (both chats) must be in the global window"
    );
    assert!(
        !returned.contains(cc_old.to_string().as_str()),
        "the older coding-agent thread must fall outside the global window — a per-source \
         window would wrongly include it as the newest of its source"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_recent_threads_archive_window_seam_is_gap_free() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let d1 = Uuid::new_v4();
    let d2 = Uuid::new_v4();
    let d3 = Uuid::new_v4();
    let d4 = Uuid::new_v4();
    let d5 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, has_response, archive_state, created_at, last_activity) \
         VALUES \
             ($1, 'd1', 'chat',        1, TRUE, 'archived', TIMESTAMPTZ '2026-06-25 00:00:00Z', TIMESTAMPTZ '2026-06-25 00:00:00Z'), \
             ($2, 'd2', 'chat',        1, TRUE, 'archived', TIMESTAMPTZ '2026-06-24 00:00:00Z', TIMESTAMPTZ '2026-06-24 00:00:00Z'), \
             ($3, 'd3', 'claude_code', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-23 00:00:00Z', TIMESTAMPTZ '2026-06-23 00:00:00Z'), \
             ($4, 'd4', 'chat',        1, TRUE, 'archived', TIMESTAMPTZ '2026-06-22 00:00:00Z', TIMESTAMPTZ '2026-06-22 00:00:00Z'), \
             ($5, 'd5', 'claude_code', 1, TRUE, 'archived', TIMESTAMPTZ '2026-06-21 00:00:00Z', TIMESTAMPTZ '2026-06-21 00:00:00Z')",
    )
    .bind(d1)
    .bind(d2)
    .bind(d3)
    .bind(d4)
    .bind(d5)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let window = store
        .get_recent_threads(2)
        .await
        .expect("get_recent_threads");
    let window_ids: std::collections::HashSet<&str> =
        window.iter().map(|t| t.thread_id.as_str()).collect();
    assert!(
        window_ids.contains(d1.to_string().as_str())
            && window_ids.contains(d2.to_string().as_str()),
        "window must be the two newest-created archived rows"
    );

    let before = chrono::DateTime::parse_from_rfc3339("2026-06-24T00:00:00Z")
        .expect("parse cursor")
        .with_timezone(&chrono::Utc);
    let older = store
        .get_older_threads(before, 15, None, None, None, None)
        .await
        .expect("get_older_threads");
    let order: Vec<&str> = older.iter().map(|t| t.thread_id.as_str()).collect();
    assert_eq!(
        order,
        vec![d3.to_string(), d4.to_string(), d5.to_string()],
        "the next page must be the contiguous older archived rows (d3, d4, d5) with no gap; got {order:?}"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn count_archived_threads_counts_only_the_archived_pile() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let rows: &[(&str, bool)] = &[
        ("archived", false),
        ("archived", false),
        ("archived", false),
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
        "only archived threads count toward the Archive badge total"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn count_archived_threads_respects_source_and_repo_filters() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4().to_string();
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

    assert_eq!(
        store
            .count_archived_threads(None, None, None, None)
            .await
            .expect("count unfiltered"),
        5,
    );

    let cc = vec!["claude_code".to_string()];
    assert_eq!(
        store
            .count_archived_threads(Some(&cc), None, None, None)
            .await
            .expect("count by source"),
        3,
        "source filter counts only archived threads on that channel"
    );

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

#[tokio::test]
async fn count_archived_threads_channels_plus_facet_counts_union() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

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

#[tokio::test]
async fn thread_summary_includes_saved() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let saved_id = Uuid::new_v4();
    let unsaved_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved) \
             VALUES ($1, 'Parked', 'chat', 1, NOW(), TRUE, TRUE), \
                    ($2, 'Not parked', 'chat', 1, NOW(), TRUE, FALSE)",
    )
    .bind(saved_id)
    .bind(unsaved_id)
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    let infos = store
        .get_threads_by_ids(&[saved_id.to_string(), unsaved_id.to_string()])
        .await
        .expect("get_threads_by_ids");

    let saved = infos
        .iter()
        .find(|t| t.thread_id == saved_id.to_string())
        .expect("the saved row must come back");
    let unsaved = infos
        .iter()
        .find(|t| t.thread_id == unsaved_id.to_string())
        .expect("the unsaved row must come back");

    assert!(
        saved.saved,
        "is_saved=TRUE in DB must surface as saved=true on ThreadSummary"
    );
    assert!(
        !unsaved.saved,
        "is_saved=FALSE in DB must surface as saved=false on ThreadSummary"
    );

    teardown_test_db(&db).await;
}

async fn insert_withheld_thread(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, \
              coding_agent_change_state, coding_agent_unproposed_reason) \
             VALUES ($1, 'CC with work', 'claude_code', 1, NOW(), TRUE, \
                     'unproposed', 'turn_incomplete')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("insert thread_summaries");
    id
}

#[tokio::test]
async fn thread_summary_includes_the_change_state_and_its_reason() {
    use crate::engine::thread_events::UnproposedReason;
    use crate::engine::thread_lifecycle::CodingAgentChangeState;

    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let id = insert_withheld_thread(&pool).await;

    let infos = store
        .get_threads_by_ids(&[id.to_string()])
        .await
        .expect("get_threads_by_ids");

    assert_eq!(infos.len(), 1, "the seeded row must come back");
    assert_eq!(
        infos[0].coding_agent_change_state,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );
    let json = serde_json::to_value(&infos[0]).expect("serialize ThreadSummary");
    assert_eq!(
        json.get("coding_agent_change_state"),
        Some(&serde_json::json!({"kind": "unproposed", "reason": "turn_incomplete"})),
        "{json}"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn thread_aggregate_includes_the_change_state_and_its_reason() {
    use crate::engine::thread_events::UnproposedReason;
    use crate::engine::thread_lifecycle::CodingAgentChangeState;

    let (pool, db) = setup_test_db().await;
    let id = insert_withheld_thread(&pool).await;

    let agg = fetch_thread_aggregate(&pool, id)
        .await
        .expect("fetch_thread_aggregate")
        .expect("aggregate row exists");

    assert_eq!(
        agg.coding_agent_change_state,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );
    let json = serde_json::to_value(&agg).expect("serialize ThreadAggregate");
    assert_eq!(
        json.get("codingAgentChangeState"),
        Some(&serde_json::json!({"kind": "unproposed", "reason": "turn_incomplete"})),
        "{json}"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn thread_summaries_refuse_every_illegal_change_state() {
    let (pool, db) = setup_test_db().await;
    let insert = |kind: &'static str, reason: Option<&'static str>, restart: bool| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO thread_summaries \
                     (thread_id, title, source, message_count, last_activity, has_response, \
                      coding_agent_change_state, coding_agent_unproposed_reason, \
                      coding_agent_requires_restart) \
                 VALUES ($1, 'I1', 'claude_code', 0, NOW(), FALSE, $2, $3, $4)",
            )
            .bind(Uuid::new_v4())
            .bind(kind)
            .bind(reason)
            .bind(restart)
            .execute(&pool)
            .await
        }
    };

    for (kind, reason, restart) in [
        ("proposed", Some("turn_incomplete"), false),
        ("none", Some("plan_missing"), false),
        ("unproposed", Some("tired"), false),
        ("half_done", None, false),
        ("unproposed", None, true),
        ("none", None, true),
    ] {
        assert!(
            insert(kind, reason, restart).await.is_err(),
            "({kind}, {reason:?}, restart={restart}) must not store"
        );
    }

    let mut legal = vec![
        ("none", None, false),
        ("unproposed", None, false),
        ("proposed", None, false),
        ("proposed", None, true),
    ];
    for reason in crate::engine::thread_events::UnproposedReason::ALL {
        legal.push(("unproposed", Some(reason.as_str()), false));
    }
    for (kind, reason, restart) in legal {
        assert!(
            insert(kind, reason, restart).await.is_ok(),
            "({kind}, {reason:?}, restart={restart}) is legal"
        );
    }

    teardown_test_db(&db).await;
}

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
    assert!(
        ids.contains(&grandparent.to_string()),
        "ancestor not loaded"
    );
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
    children.reverse();

    let extension = store
        .fetch_family_extension(&[parent.to_string()], 3)
        .await
        .expect("fetch_family_extension capped");

    assert_eq!(extension.len(), 3, "cap of 3 must be honored");
    let kept: std::collections::HashSet<String> =
        extension.iter().map(|t| t.thread_id.clone()).collect();
    for newest in children.iter().rev().take(3) {
        assert!(
            kept.contains(&newest.to_string()),
            "cap must keep newest first; missing {}",
            newest
        );
    }

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn titles_awaiting_answer_excludes_every_thread_the_user_put_down() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "Waiting and live",
            "waiting_for_user_answer",
            "active",
            "inbox",
        ),
        (
            "Waiting but archived",
            "waiting_for_user_answer",
            "active",
            "archived",
        ),
        (
            "Waiting but discarded",
            "waiting_for_user_answer",
            "discarded",
            "inbox",
        ),
        (
            "Waiting but composing",
            "waiting_for_user_answer",
            "composing",
            "inbox",
        ),
        ("Live but working", "running", "active", "inbox"),
        ("Live and idle", "idle", "active", "inbox"),
    ];
    for (title, status, state, archive_state) in rows {
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, state, archive_state) \
                 VALUES ($1, $2, 'chat', 1, NOW(), TRUE, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(*title)
        .bind(*status)
        .bind(*state)
        .bind(*archive_state)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    let titles = store
        .titles_awaiting_answer(20)
        .await
        .expect("titles_awaiting_answer");

    assert_eq!(titles, vec!["Waiting and live".to_string()]);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn titles_awaiting_answer_names_a_thread_with_no_title_yet() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, \
              status, state, archive_state) \
             VALUES ($1, NULL, 'chat', 1, NOW(), TRUE, \
                     'waiting_for_user_answer', 'active', 'inbox')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("insert thread_summaries");

    assert_eq!(
        store
            .titles_awaiting_answer(20)
            .await
            .expect("titles_awaiting_answer"),
        vec!["Untitled".to_string()]
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_first_message_of_a_spoken_only_thread_is_the_spoken_row() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let thread_id = Uuid::new_v4();
    insert_thread(&pool, thread_id, "").await;
    insert_message(
        &pool,
        thread_id,
        "SpokenMessageReceived",
        "what's going on in the workspace now",
    )
    .await;

    let found = store
        .get_thread_first_message(&thread_id.to_string())
        .await
        .expect("read the first message");

    assert_eq!(
        found.map(|(text, _, _)| text),
        Some("what's going on in the workspace now".to_string())
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_first_message_is_the_earliest_of_either_kind() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let thread_id = Uuid::new_v4();
    insert_thread(&pool, thread_id, "").await;
    insert_message(&pool, thread_id, "SpokenMessageReceived", "spoken first").await;
    insert_message(&pool, thread_id, "MessageReceived", "typed second").await;

    let found = store
        .get_thread_first_message(&thread_id.to_string())
        .await
        .expect("read the first message");

    assert_eq!(
        found.map(|(text, _, _)| text),
        Some("spoken first".to_string())
    );

    teardown_test_db(&db).await;
}
