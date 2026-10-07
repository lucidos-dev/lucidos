//! The Tree backfill read and its estimate (ADR 0362).
//!
//! Settings → System → Memory draws both. The estimate is read before anything
//! is spent. The backfill read is the snapshot the progress bar opens on, and
//! the `TreeBackfill*` SSE frames move it after that. The e2e engine runs on
//! the mock model, which the compactor uses too, so choosing Tree here costs
//! nothing and finishes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::support::{base_url, http_client, user_client};

async fn backfill() -> Value {
    let resp = http_client()
        .get(format!("{}/api/v1/memory/tree-backfill", base_url()))
        .send()
        .await
        .expect("tree-backfill request failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("Invalid JSON")
}

/// Off, on Classic. `started` says whether any summary is stored, which
/// depends on what the workspace held before this test, so only its type is
/// pinned.
fn assert_off(body: &Value) {
    assert_eq!(body["state"], "off", "{body}");
    assert!(body["started"].is_boolean(), "{body}");
}

async fn set_memory_module(value: &str) {
    let resp = user_client()
        .await
        .put(format!(
            "{}/api/v1/preferences?key=memory_module",
            base_url()
        ))
        .json(&json!({ "value": value }))
        .send()
        .await
        .expect("preference write failed");
    assert!(
        resp.status().is_success(),
        "memory_module={value}: {}",
        resp.status()
    );
}

#[tokio::test]
async fn the_estimate_prices_a_backfill_before_anything_is_spent() {
    let resp = http_client()
        .get(format!(
            "{}/api/v1/memory/tree-backfill/estimate",
            base_url()
        ))
        .send()
        .await
        .expect("estimate request failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("Invalid JSON");
    for field in ["calls", "usable_secs", "complete_secs", "daily_calls"] {
        let low = body[field]["low"]
            .as_u64()
            .unwrap_or_else(|| panic!("{field}.low: {body}"));
        let high = body[field]["high"]
            .as_u64()
            .unwrap_or_else(|| panic!("{field}.high: {body}"));
        assert!(low <= high, "{field}: {low} > {high}");
    }
    let costs = body["costs"].as_array().expect("costs");
    let flash = costs
        .iter()
        .find(|c| c["model"] == "gemini-3.8-flash")
        .expect("a compactor model is priced");
    let by_effort = flash["by_effort"].as_array().expect("by_effort");
    let low_row = by_effort
        .iter()
        .find(|e| e["effort"] == "low")
        .expect("a low-tier row");
    let low_usd = low_row["backfill_usd"]["low"].as_f64().unwrap();
    let high_usd = low_row["backfill_usd"]["high"].as_f64().unwrap();
    assert!(low_usd <= high_usd);
    let central = low_row["backfill_usd_central"].as_f64().unwrap();
    assert!((low_usd..=high_usd).contains(&central), "{low_row}");

    let high_row = by_effort
        .iter()
        .find(|e| e["effort"] == "high")
        .expect("a high-tier row");
    assert!(
        high_row["backfill_usd"]["high"].as_f64().unwrap() >= high_usd,
        "a higher reasoning tier must not cost less: {by_effort:?}"
    );
}

/// Off on Classic, running then ready on Tree, off again on Classic.
#[tokio::test]
async fn the_backfill_read_follows_the_module_to_ready() {
    set_memory_module("classic").await;
    assert_off(&backfill().await);

    set_memory_module("tree").await;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let body = backfill().await;
        match body["state"].as_str() {
            Some("ready") => break,
            Some("running") => {
                let done = body["progress"]["done"].as_u64().expect("done");
                let total = body["progress"]["total"].as_u64().expect("total");
                assert!(done <= total, "{body}");
                assert!(body["progress"]["waiting_for_model"].is_boolean(), "{body}");
            }
            _ => panic!("Tree is chosen, so the read is running or ready: {body}"),
        }
        assert!(
            Instant::now() < deadline,
            "the backfill never finished: {body}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    set_memory_module("classic").await;
    assert_off(&backfill().await);
}

async fn get_json(path: &str) -> (u16, Value) {
    let resp = http_client()
        .get(format!("{}/api/v1{path}", base_url()))
        .send()
        .await
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    let status = resp.status().as_u16();
    (status, resp.json().await.expect("Invalid JSON"))
}

/// The summary tree browser's two reads answer on any workspace, built or not:
/// the top of a tree, and the threads that have one. A thread's top names
/// lines in that thread's tree, which `/api/v1/recall/zoom` opens.
#[tokio::test]
async fn the_tree_browser_reads_a_top_and_the_threads_with_trees() {
    let (status, top) = get_json("/memory/tree").await;
    assert_eq!(status, 200, "{top}");
    assert!(top["entries"].is_u64(), "{top}");
    for line in top["lines"].as_array().expect("lines") {
        assert!(line["id"].as_str().unwrap().starts_with("w/"), "{line}");
        assert!(line["text"].is_string(), "{line}");
    }

    let (status, page) = get_json("/memory/tree/threads?limit=5").await;
    assert_eq!(status, 200, "{page}");
    let threads = page["threads"].as_array().expect("threads");
    assert!(threads.len() <= 5);
    assert!(
        page["total"].as_u64().unwrap() >= threads.len() as u64,
        "{page}"
    );
    assert!(page["has_more"].is_boolean(), "{page}");

    if let Some(thread) = threads.first() {
        let id = thread["thread_id"].as_str().expect("thread_id");
        assert!(thread["summarised"].is_u64(), "{thread}");
        let (status, top) = get_json(&format!("/memory/tree?thread={id}")).await;
        assert_eq!(status, 200, "{top}");
        for line in top["lines"].as_array().expect("lines") {
            assert!(line["id"].as_str().unwrap().starts_with(id), "{line}");
        }
    }

    let (status, body) = get_json(&format!("/memory/tree?thread={}", uuid::Uuid::new_v4())).await;
    assert_eq!(status, 400, "an unknown thread is refused: {body}");
}
