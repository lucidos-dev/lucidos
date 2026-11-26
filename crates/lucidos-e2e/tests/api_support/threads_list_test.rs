use crate::support::{base_url, db_url, http_client, unique_marker};
use lucidos_engine::engine::thread_lifecycle::ThreadStatus;

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

    let idle = seed_summary_row(&pool, &format!("{marker}-idle"), "chat", ThreadStatus::Idle).await;
    sqlx::query(
        "UPDATE thread_summaries SET live_event_wait_count = 1, \
         live_event_waits = jsonb_build_array($2::jsonb || \
             jsonb_build_object('expires_at', now() + interval '1 hour')) \
         WHERE thread_id = $1",
    )
    .bind(idle)
    .bind(serde_json::json!({
        "wait_id": uuid::Uuid::new_v4(),
        "on": [{"event_type": "BackgroundBashCompleted"}],
        "reason": "the test task",
    }))
    .execute(&pool)
    .await
    .expect("seed an event wait");
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
        "coding_agent_change_state",
        "state",
        "compose_text",
        "compose_images",
        "compose_epoch",
    ] {
        assert!(
            row.get(field).is_some(),
            "ThreadSummary wire shape missing field `{field}` — got {row}"
        );
    }
    assert_eq!(row["status"].as_str(), Some("running"));
    assert_eq!(row["channel"].as_str(), Some("chat"));

    let idle = ours
        .iter()
        .find(|r| r["title"].as_str() == Some(&format!("{marker}-idle")))
        .expect("idle row must be present");
    assert_eq!(idle["status"], "idle");
    assert_eq!(idle["liveness"]["status"], "idle");
    assert_eq!(idle["liveness"]["state"], "waiting");
    let waits = idle["liveness"]["live_event_waits"].as_array().unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0]["reason"], "the test task");
    assert_eq!(waits[0]["on"][0]["event_type"], "BackgroundBashCompleted");
    assert!(waits[0]["expires_in_secs"].as_i64().unwrap() > 0);
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
    let parent = seed_summary_row(
        &pool,
        &format!("{marker}-parent"),
        "chat",
        ThreadStatus::Idle,
    )
    .await;
    let _running = seed_child_row(
        &pool,
        &format!("{marker}-r1"),
        parent,
        ThreadStatus::Running,
    )
    .await;
    let _wait_user = seed_child_row(
        &pool,
        &format!("{marker}-r2"),
        parent,
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;
    let _idle = seed_child_row(&pool, &format!("{marker}-i"), parent, ThreadStatus::Idle).await;

    let list_url = format!(
        "{}/api/v1/threads/list?parent={parent}&active=true&limit=1000",
        base_url()
    );
    let count_url = format!(
        "{}/api/v1/threads/count?parent={parent}&active=true",
        base_url()
    );

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

    assert_eq!(list_len, count_val, "list and count filters must agree");

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
