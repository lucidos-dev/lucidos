//! Coverage for `GET /api/v1/threads/list` and `GET /api/v1/threads/count` —
//! the script/trigger/LLM-tool surface for querying thread summaries.
//!
//! The strategy seeds rows directly into `thread_summaries` with a unique
//! marker title, then asserts the new endpoints surface them with the right
//! shape and the active filter behaves correctly. Going through the chat
//! endpoint would have to wait for an LLM round-trip and would race other
//! parallel tests; seeding the projection row is the contract surface the
//! endpoints actually read.

use crate::support::{base_url, db_url, http_client, unique_marker};
use lucidos_engine::engine::thread_lifecycle::ThreadStatus;

/// Insert a `thread_summaries` row with controlled `status` / `source` /
/// title for assertion. Returns the seeded `thread_id`. Takes
/// `ThreadStatus` (not a bare string) so a future variant rename breaks
/// this test at compile time instead of producing silently empty filters.
async fn seed_summary_row(
    pool: &sqlx::PgPool,
    title: &str,
    source: &str,
    status: ThreadStatus,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, first_message, source, initiator, \
             created_at, last_activity, message_count, status) \
         VALUES ($1, $2, $2, $3, 'user', NOW(), NOW(), 1, $4)",
    )
    .bind(id)
    .bind(title)
    .bind(source)
    .bind(status.as_str())
    .execute(pool)
    .await
    .expect("seed thread_summaries");
    id
}

/// Look up our seeded summaries inside the response by title prefix —
/// other parallel tests' rows must not pollute the assertions.
fn rows_with_title_prefix<'a>(
    body: &'a serde_json::Value,
    prefix: &str,
) -> Vec<&'a serde_json::Value> {
    body.as_array()
        .expect("list response must be a JSON array")
        .iter()
        .filter(|r| {
            r["title"]
                .as_str()
                .map(|t| t.starts_with(prefix))
                .unwrap_or(false)
        })
        .collect()
}

#[tokio::test]
async fn list_returns_seeded_threads_with_full_shape() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-shape");

    let _idle =
        seed_summary_row(&pool, &format!("{marker}-idle"), "chat", ThreadStatus::Idle).await;
    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;

    let url = format!("{}/api/v1/threads/list?limit=1000", base_url());
    let resp = client.get(&url).send().await.expect("list request");
    assert_eq!(resp.status(), 200, "list endpoint should return 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    assert_eq!(ours.len(), 2, "both seeded rows should appear");

    // Spot-check the full ThreadSummary shape — every field the projection
    // populates must serialise into the wire response.
    let row = ours
        .iter()
        .find(|r| r["title"].as_str() == Some(&format!("{marker}-running")))
        .expect("running row must be present");
    for field in [
        "thread_id",
        "title",
        "channel",
        "initiator",
        "created_at",
        "last_activity",
        "message_count",
        "section",
        "status",
        "active_children_count",
        "total_children_count",
        "coding_agent_has_diff",
        "coding_agent_proposed",
        "state",
        "compose_text",
        "compose_images",
        // The client echoes this back on every compose PUT, so a summary that
        // omits it leaves every write unfenced.
        "compose_epoch",
    ] {
        assert!(
            row.get(field).is_some(),
            "ThreadSummary wire shape missing field `{field}` — got {row}"
        );
    }
    assert_eq!(row["status"].as_str(), Some("running"));
    assert_eq!(row["channel"].as_str(), Some("chat"));
}

#[tokio::test]
async fn list_active_filter_excludes_idle_and_failed() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-active");

    let _idle =
        seed_summary_row(&pool, &format!("{marker}-idle"), "chat", ThreadStatus::Idle).await;
    let _failed = seed_summary_row(
        &pool,
        &format!("{marker}-failed"),
        "chat",
        ThreadStatus::Failed,
    )
    .await;
    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let _waiting_for_user = seed_summary_row(
        &pool,
        &format!("{marker}-wait-user"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;
    // `waiting` (CC proposed changes — loop has stopped) is NOT active per
    // the canonical definition.
    let _waiting = seed_summary_row(
        &pool,
        &format!("{marker}-waiting"),
        "claude_code",
        ThreadStatus::Waiting,
    )
    .await;

    let url = format!("{}/api/v1/threads/list?active=true&limit=1000", base_url());
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("active=true request")
        .json()
        .await
        .expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    let titles: Vec<&str> = ours.iter().filter_map(|r| r["title"].as_str()).collect();
    assert!(
        titles.contains(&format!("{marker}-running").as_str()),
        "running row must appear in active list, got {titles:?}"
    );
    assert!(
        titles.contains(&format!("{marker}-wait-user").as_str()),
        "waiting_for_user_answer row must appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-idle").as_str()),
        "idle row must NOT appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-failed").as_str()),
        "failed row must NOT appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-waiting").as_str()),
        "waiting (post-CC-idled with proposed changes) must NOT appear in active list \
         — the loop has paused; got {titles:?}"
    );
}

#[tokio::test]
async fn list_source_filter_narrows_results() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-source");

    let _chat =
        seed_summary_row(&pool, &format!("{marker}-chat"), "chat", ThreadStatus::Idle).await;
    let _cc = seed_summary_row(
        &pool,
        &format!("{marker}-cc"),
        "claude_code",
        ThreadStatus::Idle,
    )
    .await;
    let _trig = seed_summary_row(
        &pool,
        &format!("{marker}-trig"),
        "trigger",
        ThreadStatus::Idle,
    )
    .await;

    let url = format!(
        "{}/api/v1/threads/list?source=chat,trigger&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("source request")
        .json()
        .await
        .expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    let channels: Vec<&str> = ours.iter().filter_map(|r| r["channel"].as_str()).collect();
    assert!(
        channels.contains(&"chat"),
        "chat row missing from source=chat,trigger filter: {channels:?}"
    );
    assert!(
        channels.contains(&"trigger"),
        "trigger row missing from source=chat,trigger filter: {channels:?}"
    );
    assert!(
        channels.iter().all(|c| *c != "claude_code"),
        "claude_code row leaked through source=chat,trigger filter: {channels:?}"
    );
}

#[tokio::test]
async fn count_matches_list_length_under_same_filter() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-count");

    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-r1"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let _wait_user = seed_summary_row(
        &pool,
        &format!("{marker}-r2"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;
    let _idle = seed_summary_row(&pool, &format!("{marker}-i"), "chat", ThreadStatus::Idle).await;

    // count and list must agree for the same filter set. Run against the
    // global active filter (no source narrowing) since other tests may also
    // have active rows — we only assert count >= our seeded count, AND that
    // count and list lengths agree.
    let list_url = format!("{}/api/v1/threads/list?active=true&limit=1000", base_url());
    let count_url = format!("{}/api/v1/threads/count?active=true", base_url());

    let list_body: serde_json::Value = client
        .get(&list_url)
        .send()
        .await
        .expect("list req")
        .json()
        .await
        .expect("invalid JSON");
    let count_body: serde_json::Value = client
        .get(&count_url)
        .send()
        .await
        .expect("count req")
        .json()
        .await
        .expect("invalid JSON");

    let list_len = list_body.as_array().expect("list is array").len() as i64;
    let count_val = count_body["count"].as_i64().expect("count is integer");

    // Both views must agree — the count endpoint is just a thinner version
    // of the same SQL filter. Other parallel tests may push the absolute
    // number around, but list.len() and count.count must match within the
    // same instant... we don't get atomicity across two HTTP calls, but the
    // margin should be tiny. Allow ±5 to absorb parallel test noise.
    assert!(
        (list_len - count_val).abs() <= 5,
        "list length {list_len} and count {count_val} disagree by more than 5 — \
         filters likely diverge"
    );

    // Our seeded active rows must be counted.
    let ours = rows_with_title_prefix(&list_body, &marker);
    assert_eq!(
        ours.len(),
        2,
        "expected both seeded active rows to appear (got {})",
        ours.len()
    );
}

#[tokio::test]
async fn list_limit_clamps_to_max_1000() {
    // The handler clamps `limit` to 1..=1000. Passing 999999 must NOT 500;
    // it should silently clamp and return success.
    let client = http_client();
    let url = format!("{}/api/v1/threads/list?limit=999999", base_url());
    let resp = client.get(&url).send().await.expect("oversized limit req");
    assert_eq!(
        resp.status(),
        200,
        "oversized limit must be clamped to 1000 server-side, not error"
    );
}

#[tokio::test]
async fn get_by_id_returns_the_seeded_thread_summary() {
    // `GET /api/v1/threads/:thread_id` — the by-id complement to `/list`, used
    // by the message-route popover to resolve a cross-workspace Origin's thread
    // name from the source workspace's engine.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-get-by-id");

    let id = seed_summary_row(
        &pool,
        &format!("{marker}-title"),
        "chat",
        ThreadStatus::Idle,
    )
    .await;

    let url = format!("{}/api/v1/threads/{}", base_url(), id);
    let resp = client.get(&url).send().await.expect("get-by-id request");
    assert_eq!(resp.status(), 200, "by-id endpoint should return 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");

    assert_eq!(body["thread_id"].as_str(), Some(id.to_string().as_str()));
    assert_eq!(
        body["title"].as_str(),
        Some(format!("{marker}-title").as_str())
    );
}

#[tokio::test]
async fn get_by_id_returns_404_for_unknown_thread() {
    let client = http_client();
    let unknown = uuid::Uuid::new_v4();
    let url = format!("{}/api/v1/threads/{}", base_url(), unknown);
    let resp = client.get(&url).send().await.expect("unknown-id request");
    assert_eq!(
        resp.status(),
        404,
        "an unknown thread id must 404, not return an empty 200"
    );
}

/// Seed a `thread_summaries` row that is a child of `parent`.
async fn seed_child_row(
    pool: &sqlx::PgPool,
    title: &str,
    parent: uuid::Uuid,
    status: ThreadStatus,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, first_message, source, initiator, \
             created_at, last_activity, message_count, status, parent_thread_id, depth) \
         VALUES ($1, $2, $2, 'chat', 'system', NOW(), NOW(), 1, $3, $4, 1)",
    )
    .bind(id)
    .bind(title)
    .bind(status.as_str())
    .bind(parent)
    .execute(pool)
    .await
    .expect("seed child thread_summaries");
    id
}

/// The read side of a parent orchestrating its own fan-out. Without it the
/// model's only recovery from losing a child's `thread_id` (history trimming
/// drops the oldest tool results first, so a fan-out orchestrator's spawn
/// results are the first to go) is to spawn a duplicate child.
#[tokio::test]
async fn list_parent_filter_returns_only_that_parents_direct_children() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-parent");

    let parent = seed_summary_row(
        &pool,
        &format!("{marker}-parent"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let other_parent = seed_summary_row(
        &pool,
        &format!("{marker}-other"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let child_a = seed_child_row(
        &pool,
        &format!("{marker}-child-a"),
        parent,
        ThreadStatus::Running,
    )
    .await;
    let child_b = seed_child_row(
        &pool,
        &format!("{marker}-child-b"),
        parent,
        ThreadStatus::Idle,
    )
    .await;
    let _grandchild = seed_child_row(
        &pool,
        &format!("{marker}-grandchild"),
        child_a,
        ThreadStatus::Running,
    )
    .await;
    let _stranger = seed_child_row(
        &pool,
        &format!("{marker}-stranger"),
        other_parent,
        ThreadStatus::Running,
    )
    .await;

    let url = format!(
        "{}/api/v1/threads/list?parent={parent}&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("parent request")
        .json()
        .await
        .expect("invalid JSON");

    let mut ids: Vec<&str> = body
        .as_array()
        .expect("list response must be a JSON array")
        .iter()
        .filter_map(|r| r["thread_id"].as_str())
        .collect();
    ids.sort_unstable();
    let mut expected = [child_a.to_string(), child_b.to_string()];
    expected.sort();
    assert_eq!(
        ids,
        expected.iter().map(String::as_str).collect::<Vec<_>>(),
        "parent= must return exactly the direct children: no grandchild, no \
         other parent's child, and not the parent itself"
    );

    // Composes with the other filters rather than replacing them.
    let url = format!(
        "{}/api/v1/threads/list?parent={parent}&active=true&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("parent+active request")
        .json()
        .await
        .expect("invalid JSON");
    let ids: Vec<&str> = body
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|r| r["thread_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [child_a.to_string().as_str()],
        "parent= composes with active=true: only the child still working"
    );
}

#[tokio::test]
async fn count_honours_the_parent_filter() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-count-parent");

    let parent = seed_summary_row(
        &pool,
        &format!("{marker}-parent"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    for i in 0..3 {
        seed_child_row(
            &pool,
            &format!("{marker}-child-{i}"),
            parent,
            ThreadStatus::Running,
        )
        .await;
    }

    let url = format!("{}/api/v1/threads/count?parent={parent}", base_url());
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("count request")
        .json()
        .await
        .expect("invalid JSON");
    assert_eq!(
        body["count"].as_i64(),
        Some(3),
        "count must apply the same parent filter as list"
    );
}

/// A malformed `parent` is a 400, never a silent "no filter" that would return
/// the whole workspace when the caller asked for one thread's children.
#[tokio::test]
async fn list_rejects_a_malformed_parent() {
    let client = http_client();
    let url = format!("{}/api/v1/threads/list?parent=not-a-uuid", base_url());
    let response = client.get(&url).send().await.expect("parent request");
    assert_eq!(
        response.status().as_u16(),
        400,
        "a malformed parent id must be rejected, not ignored"
    );
}

/// The reason the `status` filter exists. `active=true` is the union of
/// `running` and `waiting_for_user_answer`, so an idle detector gated on it
/// stays silent for as long as anybody is parked on an unanswered question. On
/// 2026-08-07 that hid four pending changes for three hours. `status=running`
/// is the same question asked precisely, and it must not pick up the parked
/// thread.
#[tokio::test]
async fn status_running_excludes_a_thread_parked_on_a_question() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-status-running");

    seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    seed_summary_row(
        &pool,
        &format!("{marker}-wait-user"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;

    let url = format!(
        "{}/api/v1/threads/list?status=running&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("status=running request")
        .json()
        .await
        .expect("invalid JSON");

    let titles: Vec<&str> = rows_with_title_prefix(&body, &marker)
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    assert_eq!(
        titles,
        [format!("{marker}-running").as_str()],
        "status=running must return the working thread and ONLY it: a thread \
         awaiting a user answer is blocked on the human, not working"
    );
}

/// The other direction of the same split: "is anything waiting on me?" must
/// not count the thread that is busy working.
#[tokio::test]
async fn status_waiting_for_user_answer_excludes_a_running_thread() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-status-awaiting");

    seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    seed_summary_row(
        &pool,
        &format!("{marker}-wait-user"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;

    let url = format!(
        "{}/api/v1/threads/list?status=waiting_for_user_answer&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("status=waiting_for_user_answer request")
        .json()
        .await
        .expect("invalid JSON");

    let titles: Vec<&str> = rows_with_title_prefix(&body, &marker)
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    assert_eq!(
        titles,
        [format!("{marker}-wait-user").as_str()],
        "status=waiting_for_user_answer must exclude the running thread"
    );
}

/// A list can be filtered client-side; a count cannot. `count` is the surface
/// an idle detector actually calls, so it has to honour `status` too.
#[tokio::test]
async fn count_honours_the_status_filter() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-count-status");

    let parent = seed_summary_row(
        &pool,
        &format!("{marker}-parent"),
        "chat",
        ThreadStatus::Idle,
    )
    .await;
    seed_child_row(
        &pool,
        &format!("{marker}-child-running"),
        parent,
        ThreadStatus::Running,
    )
    .await;
    for i in 0..2 {
        seed_child_row(
            &pool,
            &format!("{marker}-child-parked-{i}"),
            parent,
            ThreadStatus::WaitingForUserAnswer,
        )
        .await;
    }

    // Scoped to one parent so parallel tests' rows cannot move the number.
    let count = |status: &str| {
        let url = format!(
            "{}/api/v1/threads/count?parent={parent}&status={status}",
            base_url()
        );
        let client = client.clone();
        async move {
            let body: serde_json::Value = client
                .get(&url)
                .send()
                .await
                .expect("count request")
                .json()
                .await
                .expect("invalid JSON");
            body["count"].as_i64().expect("count is an integer")
        }
    };

    assert_eq!(count("running").await, 1, "one child is actually working");
    assert_eq!(
        count("waiting_for_user_answer").await,
        2,
        "two children are parked on a question"
    );
    assert_eq!(
        count("running,waiting_for_user_answer").await,
        3,
        "naming both statuses is the union, and agrees with active=true"
    );

    let active_url = format!(
        "{}/api/v1/threads/count?parent={parent}&active=true",
        base_url()
    );
    let active_body: serde_json::Value = client
        .get(&active_url)
        .send()
        .await
        .expect("active count request")
        .json()
        .await
        .expect("invalid JSON");
    assert_eq!(
        active_body["count"].as_i64(),
        Some(3),
        "active=true is unchanged: it still returns the union of both states"
    );
}

/// Two answers to one question. Silently intersecting them would make
/// `active=true&status=idle` an empty result that reads as "nothing matched".
#[tokio::test]
async fn active_and_status_together_is_rejected() {
    let client = http_client();
    for path in ["list", "count"] {
        let url = format!(
            "{}/api/v1/threads/{path}?active=true&status=running",
            base_url()
        );
        let response = client.get(&url).send().await.expect("conflict request");
        assert_eq!(
            response.status().as_u16(),
            400,
            "{path} must refuse active and status together, not intersect them"
        );
    }
}

/// A typo that silently returned zero rows would read as "the workspace is
/// quiet", which is the failure this filter exists to prevent.
#[tokio::test]
async fn an_unknown_status_is_rejected_rather_than_matching_nothing() {
    let client = http_client();
    for raw in ["runnign", ""] {
        let url = format!("{}/api/v1/threads/list?status={raw}", base_url());
        let response = client.get(&url).send().await.expect("bad status request");
        assert_eq!(
            response.status().as_u16(),
            400,
            "status={raw:?} must be refused, not read as an empty or absent filter"
        );
    }
}
