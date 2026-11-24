//! `GET /api/v1/hooks` lists the workspace's event hooks with their arming,
//! and the owner arms or removes one (ADR 0746).
//!
//! Only a live request proves the wiring: the engine executable hands the
//! leaf crate's loader to the router, and a unit test never sees that.

use crate::support::{base_url, http_client, unique_marker, workspace_path};

/// One test, because it writes into the workspace tree and holds its lock for
/// the whole sequence.
#[tokio::test]
async fn the_hook_list_shows_definitions_and_the_owner_removes_one() {
    let client = http_client();
    let _tree = crate::support::workspace_tree_lock().read().await;

    let valid = unique_marker("hook-valid");
    let invalid = unique_marker("hook-invalid");
    let hooks = workspace_path().join("data").join("hooks");
    let valid_dir = hooks.join(&valid);
    std::fs::create_dir_all(&valid_dir).unwrap();
    std::fs::write(
        valid_dir.join("hook.toml"),
        "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"check.py\"\n",
    )
    .unwrap();
    std::fs::write(valid_dir.join("check.py"), "print('allow')\n").unwrap();
    let invalid_dir = hooks.join(&invalid);
    std::fs::create_dir_all(&invalid_dir).unwrap();
    std::fs::write(
        invalid_dir.join("hook.toml"),
        "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"missing.py\"\n",
    )
    .unwrap();

    let (status, body) = list(&client).await;
    let owner = crate::support::user_client().await;
    let hook = |slug: &str, verb: &str| format!("{}/api/v1/hooks/{slug}{verb}", base_url());
    let arm = owner
        .post(hook(&valid, "/arm"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("arm request failed");
    let arm_status = arm.status();
    let removal = owner
        .delete(hook(&valid, ""))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("remove request failed")
        .status();
    let removed_from_disk = !valid_dir.exists();
    let (_, after) = list(&client).await;

    let _ = std::fs::remove_dir_all(&valid_dir);
    let _ = std::fs::remove_dir_all(&invalid_dir);

    assert_eq!(status, 200, "{body}");
    let row = |body: &serde_json::Value, slug: &str| {
        body["hooks"]
            .as_array()
            .expect("a hooks array")
            .iter()
            .find(|row| row["slug"] == slug)
            .cloned()
    };
    let valid_row = row(&body, &valid).unwrap_or_else(|| panic!("{valid} is not listed: {body}"));
    assert_eq!(valid_row["status"], "valid", "{valid_row}");
    assert_eq!(valid_row["event_kind"], "thread", "{valid_row}");
    assert_eq!(valid_row["arm_state"], "disarmed", "{valid_row}");
    assert_eq!(
        valid_row["content_hash"].as_str().map(str::len),
        Some(64),
        "{valid_row}"
    );
    let invalid_row =
        row(&body, &invalid).unwrap_or_else(|| panic!("{invalid} is not listed: {body}"));
    assert_eq!(invalid_row["status"], "invalid", "{invalid_row}");
    assert!(
        invalid_row["problems"][0]
            .as_str()
            .is_some_and(|p| p.contains("missing.py")),
        "{invalid_row}"
    );

    // No asking site asks about any event yet, so even the owner's arm is refused.
    assert!(
        valid_row["arm_refusal"]
            .as_str()
            .is_some_and(|r| r.contains("MessageReceived")),
        "{valid_row}"
    );
    assert_eq!(arm_status, 409);
    assert_eq!(removal, 200);
    assert!(removed_from_disk, "remove deletes data/hooks/{valid}/");
    assert!(row(&after, &valid).is_none(), "{after}");
}

async fn list(client: &reqwest::Client) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = client
        .get(format!("{}/api/v1/hooks", base_url()))
        .send()
        .await
        .expect("hook list request failed");
    let status = resp.status();
    (status, resp.json().await.expect("hook list is JSON"))
}
