//! E2E coverage for the `/api/v1/trigger-groups` HTTP surface and the trigger
//! `group_id` field, exercising the engine's in-memory invariants over real
//! HTTP — unique-name, unknown-id rejection, delete-when-empty, and the
//! denormalized `member_count` projection.

use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;

async fn create_group(
    client: &reqwest::Client,
    name: &str,
    order: Option<i64>,
) -> serde_json::Value {
    let mut body = json!({ "name": name });
    if let Some(o) = order {
        body["order"] = json!(o);
    }
    let resp = client
        .post(format!("{}/api/v1/trigger-groups", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /trigger-groups failed");
    assert_eq!(resp.status().as_u16(), 200, "Expected 200 OK");
    resp.json().await.expect("Invalid JSON")
}

async fn delete_group(client: &reqwest::Client, id: &str) -> reqwest::Response {
    client
        .delete(format!("{}/api/v1/trigger-groups?id={}", base_url(), id))
        .send()
        .await
        .expect("DELETE /trigger-groups failed")
}

async fn list_groups(client: &reqwest::Client) -> serde_json::Value {
    let resp = client
        .get(format!("{}/api/v1/trigger-groups", base_url()))
        .send()
        .await
        .expect("GET /trigger-groups failed");
    assert_eq!(resp.status().as_u16(), 200);
    resp.json().await.expect("Invalid JSON")
}

#[tokio::test]
async fn create_list_delete_round_trips() {
    let client = user_client().await;
    let name = unique_marker("e2e-group");
    let group = create_group(&client, &name, None).await;
    let group_id = group["id"].as_str().unwrap().to_string();
    assert_eq!(group["name"], name);
    assert_eq!(group["member_count"], 0);

    let listed = list_groups(&client).await;
    let found = listed["groups"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["id"] == group_id);
    assert!(found, "Created group must appear in list response");

    let resp = delete_group(&client, &group_id).await;
    assert_eq!(resp.status().as_u16(), 204);
}

#[tokio::test]
async fn create_rejects_duplicate_name_case_insensitive() {
    let client = user_client().await;
    let name = unique_marker("dup-group");
    let group = create_group(&client, &name, None).await;
    let group_id = group["id"].as_str().unwrap().to_string();

    // Same name, different case — engine must reject as duplicate.
    let upper = name.to_uppercase();
    let resp = client
        .post(format!("{}/api/v1/trigger-groups", base_url()))
        .json(&json!({ "name": upper }))
        .send()
        .await
        .expect("POST /trigger-groups failed");
    assert_eq!(
        resp.status().as_u16(),
        409,
        "Case-insensitive duplicate must return 409"
    );

    delete_group(&client, &group_id).await;
}

/// Regression: N parallel POSTs with the same group name must produce exactly
/// one row in the projection. Before the create-path lock was added, two POSTs
/// could both pass the read-time dedup check and both apply, leaving the
/// in-memory registry with duplicate `name` values that violate the
/// unique-name invariant the rest of the system assumes.
#[tokio::test]
async fn concurrent_creates_with_same_name_yield_one_group() {
    let client = user_client().await;
    // Append a uuid so two concurrent test-runner invocations (or rapid
    // re-runs landing on the same millisecond) can't share the marker and
    // poison each other's win/conflict counts.
    let name = format!("{}-{}", unique_marker("race-group"), uuid::Uuid::new_v4());

    // 8 racing POSTs is enough to reliably reproduce the bug pre-fix: with
    // sub-millisecond LAN latency the read-lock dedup window opens for every
    // request before any of them emits, so multiple inserts land. Holding the
    // write lock across read + emit + apply makes the dedup atomic.
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let client = client.clone();
        let name = name.clone();
        tasks.push(tokio::spawn(async move {
            client
                .post(format!("{}/api/v1/trigger-groups", base_url()))
                .json(&json!({ "name": name }))
                .send()
                .await
                .expect("POST /trigger-groups failed")
                .status()
                .as_u16()
        }));
    }

    let mut ok = 0;
    let mut conflict = 0;
    let mut other = Vec::new();
    for t in tasks {
        match t.await.expect("task panicked") {
            200 => ok += 1,
            409 => conflict += 1,
            s => other.push(s),
        }
    }
    assert!(other.is_empty(), "Unexpected status codes: {:?}", other);
    assert_eq!(
        ok, 1,
        "Exactly one POST must succeed (got ok={} conflict={})",
        ok, conflict
    );
    assert_eq!(conflict, 7, "All other POSTs must return 409");

    // Projection must agree: only one group with that name (the api-level
    // counts above can lie if both writers think they're the winner).
    let listed = list_groups(&client).await;
    let matching: Vec<_> = listed["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["name"].as_str() == Some(name.as_str()))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "Exactly one group should exist with name '{}' (found {}: {:#?})",
        name,
        matching.len(),
        matching
    );

    let group_id = matching[0]["id"].as_str().unwrap().to_string();
    delete_group(&client, &group_id).await;
}

#[tokio::test]
async fn unknown_group_id_on_trigger_create_is_rejected() {
    let client = user_client().await;
    // Make sure timezone is set so create_trigger doesn't reject for that reason.
    let _ = client
        .put(format!("{}/api/v1/preferences", base_url()))
        .json(&json!({ "key": "timezone", "value": "UTC" }))
        .send()
        .await;

    let body = json!({
        "name": unique_marker("unknown-gid"),
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "group_id": "00000000-0000-0000-0000-000000000000",
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    let api_result: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(api_result["success"], false);
    assert!(
        api_result["error"]
            .as_str()
            .unwrap_or("")
            .contains("Unknown group_id"),
        "Expected 'Unknown group_id' in error, got: {:?}",
        api_result["error"]
    );
}

#[tokio::test]
async fn delete_blocks_when_non_empty_and_returns_members() {
    let client = user_client().await;
    let _ = client
        .put(format!("{}/api/v1/preferences", base_url()))
        .json(&json!({ "key": "timezone", "value": "UTC" }))
        .send()
        .await;

    let group_name = unique_marker("non-empty-group");
    let group = create_group(&client, &group_name, None).await;
    let group_id = group["id"].as_str().unwrap().to_string();

    // Create a trigger inside the group.
    let trigger_body = json!({
        "name": unique_marker("group-member"),
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "group_id": group_id,
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&trigger_body)
        .send()
        .await
        .expect("POST /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger create must succeed: {:?}",
        res
    );

    // Find the trigger id via the list endpoint so we can clean up afterwards.
    // POST /triggers returns once the event is persisted; the list-endpoint
    // projection is updated asynchronously. Poll briefly so a slow projection
    // under parallel load (the API e2e suite runs all 145 tests concurrently)
    // doesn't flake the test before the new row materialises.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let trigger_id = loop {
        let triggers: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .expect("GET /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        let found = triggers["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["group_id"].as_str() == Some(&group_id))
            .map(|t| t["id"].as_str().unwrap().to_string());
        if let Some(id) = found {
            break id;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Trigger with the new group_id must appear in the list within 5s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    // Member-count badge should reflect the new member. Same projection
    // race as above — poll until the count catches up.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed = list_groups(&client).await;
        let bumped = listed["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == group_id)
            .expect("Group still listed");
        if bumped["member_count"] == 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "member_count must reflect the new trigger within 5s (saw {:?})",
            bumped["member_count"]
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Delete must refuse with 409 + structured body listing the member.
    let resp = delete_group(&client, &group_id).await;
    assert_eq!(resp.status().as_u16(), 409);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(body["error"], "non_empty");
    assert_eq!(body["member_count"], 1);
    assert_eq!(body["member_trigger_ids"][0], trigger_id);

    // Move trigger out of the group via PUT /triggers, then retry delete.
    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "group_id": null }))
        .send()
        .await
        .expect("PUT /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Clearing group_id must succeed: {:?}",
        res
    );

    // Clearing group_id rides the same async projection as the create above:
    // PUT /triggers returns once the TriggerUpdated event is persisted, but the
    // in-memory trigger registry the delete handler reads is updated by the
    // EventBus subscriber asynchronously. Poll the delete until the membership
    // drop has propagated (204) instead of asserting on the first attempt — a
    // single shot races the subscriber under parallel load and flakes with 409.
    // Delete is a safe retry: a 409 emits nothing, so re-issuing is a no-op
    // until the registry catches up.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let resp = delete_group(&client, &group_id).await;
        let status = resp.status().as_u16();
        if status == 204 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Empty group delete must succeed within 5s (last status {})",
            status
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Cleanup: delete the trigger so other tests' /triggers polls aren't
    // polluted by stale entries.
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
}

#[tokio::test]
async fn reorder_endpoint_updates_panel_order() {
    let client = user_client().await;
    let a = create_group(&client, &unique_marker("reorder-a"), Some(100)).await;
    let b = create_group(&client, &unique_marker("reorder-b"), Some(200)).await;
    let a_id = a["id"].as_str().unwrap().to_string();
    let b_id = b["id"].as_str().unwrap().to_string();

    // Flip their order.
    let resp = client
        .post(format!("{}/api/v1/trigger-groups/reorder", base_url()))
        .json(&json!({
            "ordering": [
                { "id": a_id, "order": 200 },
                { "id": b_id, "order": 100 },
            ]
        }))
        .send()
        .await
        .expect("POST /reorder failed");
    assert_eq!(resp.status().as_u16(), 204);

    let listed = list_groups(&client).await;
    let groups = listed["groups"].as_array().unwrap();
    let order_of = |id: &str| -> i64 {
        groups
            .iter()
            .find(|g| g["id"] == id)
            .map(|g| g["order"].as_i64().unwrap())
            .expect("group present")
    };
    assert_eq!(order_of(&a_id), 200);
    assert_eq!(order_of(&b_id), 100);

    delete_group(&client, &a_id).await;
    delete_group(&client, &b_id).await;
}
