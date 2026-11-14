//! E2E coverage for a batch of reads run as one parallel run (ADR 0246).
//!
//! The mock's `MOCK_READ_FILES:` sentinel puts several `read_file` calls in one
//! response. Only that reaches the parallel path through the real agent loop.
//! Each call emits its own events as it starts and ends, so the rows may
//! interleave in any order. The contract is the pairing: every result names
//! its own call, lands after it, and carries that call's outcome.
//!
//! The paths do not exist on purpose. `read_file` resolves under `data/`, which
//! the workspace tracks in git, and a scratch file there would dirty the tree
//! the apply tests merge into. A missing file's result names its own path,
//! which is all the pairing check needs.

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use serde_json::json;

#[tokio::test]
async fn a_batch_of_reads_pairs_each_result_with_its_own_call() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let client = user_client().await;

    let marker = unique_marker("api-parallel-reads");
    let dir = format!("e2e-parallel-{}", uuid::Uuid::new_v4().simple());
    let paths: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|name| format!("{dir}/{name}.txt"))
        .collect();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_READ_FILES: {}", paths.join(" ")),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let done: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type IN ('ResponseGenerated', 'ResponseFailed', 'ResponseAborted')",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("DB query failed");
        if done > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the batched turn never terminated"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let rows: Vec<(uuid::Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT id, event_type, payload FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type IN ('ToolCalled', 'ToolResult') \
          ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("DB query failed");

    let calls: Vec<(usize, &uuid::Uuid, &serde_json::Value)> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, kind, _))| kind == "ToolCalled")
        .map(|(position, (id, _, payload))| (position, id, payload))
        .collect();
    let results: Vec<(usize, &serde_json::Value)> = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, kind, _))| kind == "ToolResult")
        .map(|(position, (_, _, payload))| (position, payload))
        .collect();
    assert_eq!(calls.len(), 3, "one ToolCalled per read: {rows:?}");
    assert_eq!(results.len(), 3, "one ToolResult per read: {rows:?}");

    // Concurrent inserts can commit in either order, so a call is known by
    // its path rather than by its position.
    let mut started: Vec<&str> = calls
        .iter()
        .filter_map(|(_, _, call)| call["args"]["path"].as_str())
        .collect();
    started.sort_unstable();
    assert_eq!(
        started,
        paths.iter().map(String::as_str).collect::<Vec<_>>()
    );

    for (call_position, call_id, call) in &calls {
        let path = call["args"]["path"].as_str().unwrap_or_default();
        let answers: Vec<&(usize, &serde_json::Value)> = results
            .iter()
            .filter(|(_, result)| {
                result["tool_called_event_id"].as_str() == Some(call_id.to_string().as_str())
            })
            .collect();
        assert_eq!(
            answers.len(),
            1,
            "{path} must get exactly one result: {rows:?}"
        );
        let (result_position, result) = answers[0];
        assert!(
            result_position > call_position,
            "{path}'s result must land after its call"
        );
        let text = result["result"].as_str().unwrap_or_default();
        assert!(
            text.contains(path),
            "{path}'s result must be its own outcome, got: {text}"
        );
    }
}
