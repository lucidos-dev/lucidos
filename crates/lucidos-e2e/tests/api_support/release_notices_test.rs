//! `/api/v1/release-notices`: what this workspace still owes the reader, and
//! the answer that settles one.

use crate::support::{base_url, http_client, user_client};
use serde_json::Value;

fn list_url() -> String {
    format!("{}/api/v1/release-notices", base_url())
}

async fn fetch_list() -> Value {
    http_client()
        .get(list_url())
        .send()
        .await
        .expect("release-notices request failed")
        .json()
        .await
        .expect("Invalid JSON")
}

/// The list is the contract both surfaces read, so its shape is what breaks
/// them. The modal draws `next_id`, the panel draws `notices` with `resolved`
/// per row, and neither has a second source to fall back on.
///
/// Which notices are VISIBLE is decided in `engine::release_notices` and pinned
/// by its own tests, against a release this build is not on. That cannot be
/// reproduced over HTTP, where the running release is whatever the engine is.
#[tokio::test]
async fn the_list_serves_both_surfaces_from_one_shape() {
    let resp = http_client()
        .get(list_url())
        .send()
        .await
        .expect("release-notices request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("Invalid JSON");
    let notices = body["notices"]
        .as_array()
        .expect("`notices` must be an array");
    assert!(
        body["next_id"].is_string() || body["next_id"].is_null(),
        "next_id names the notice the modal shows, or nothing: {body}"
    );

    for notice in notices {
        for field in ["id", "since", "title", "body"] {
            let value = notice[field]
                .as_str()
                .unwrap_or_else(|| panic!("every notice needs {field}: {notice}"));
            assert!(!value.trim().is_empty(), "empty {field}: {notice}");
        }
        assert!(
            notice["resolved"].is_boolean(),
            "the panel reads `resolved` per notice: {notice}"
        );
        // Paired by construction, so a button is never drawn with nothing to
        // send. Refused at parse time; asserted here on the wire shape.
        assert_eq!(
            notice["action_label"].is_string(),
            notice["action_prompt"].is_string(),
            "an action is both fields or neither: {notice}"
        );
    }
}

/// A resolve names a notice, so an id this build does not carry is a 404 rather
/// than a silent no-op. The cursor must never move to a name nothing defines.
#[tokio::test]
async fn resolving_an_unknown_notice_is_refused() {
    let resp = user_client()
        .await
        .post(format!("{}/resolve", list_url()))
        .json(&serde_json::json!({ "id": "no-such-release-notice" }))
        .send()
        .await
        .expect("resolve request failed");
    assert_eq!(resp.status(), 404);
}

/// Answering the same notice twice changes nothing and is not an error.
///
/// Two devices showing one modal is the ordinary case, so the second answer has
/// to be a quiet no-op. This runs against whichever notice the e2e workspace has
/// already settled, and skips only a build that ships none at all.
#[tokio::test]
async fn answering_a_settled_notice_changes_nothing() {
    let before = fetch_list().await;
    let notices = before["notices"].as_array().expect("`notices` array");
    let Some(settled) = notices
        .iter()
        .find(|n| n["resolved"] == Value::Bool(true))
        .and_then(|n| n["id"].as_str())
        .map(str::to_string)
    else {
        return;
    };

    // Resolving walks the workspace past the notice, so the caller has to be
    // one the engine can name (ADR 0169). Reading the list needs nobody.
    let resp = user_client()
        .await
        .post(format!("{}/resolve", list_url()))
        .json(&serde_json::json!({ "id": settled }))
        .send()
        .await
        .expect("resolve request failed");
    assert_eq!(resp.status(), 200);

    // Asserted per notice rather than as whole-list equality: these tests share
    // one workspace, and a sibling answering the owed notice concurrently would
    // legitimately change the list around this one.
    let after: Value = resp.json().await.expect("Invalid JSON");
    let row = after["notices"]
        .as_array()
        .expect("`notices` array")
        .iter()
        .find(|n| n["id"].as_str() == Some(settled.as_str()))
        .unwrap_or_else(|| panic!("a settled notice stays in the list: {after}"));
    assert_eq!(row["resolved"], Value::Bool(true));
    assert_ne!(
        after["next_id"].as_str(),
        Some(settled.as_str()),
        "re-answering must not put a settled notice back on the modal"
    );
}

/// Answering settles the notice and hands back the same list shape, so the
/// caller needs no second request to know what is left. Skipped when this build
/// ships no outstanding notice, which is the ordinary state of most releases.
#[tokio::test]
async fn answering_the_owed_notice_settles_it() {
    let before = fetch_list().await;
    let Some(owed) = before["next_id"].as_str().map(str::to_string) else {
        return;
    };

    let resp = user_client()
        .await
        .post(format!("{}/resolve", list_url()))
        .json(&serde_json::json!({ "id": owed }))
        .send()
        .await
        .expect("resolve request failed");
    assert_eq!(resp.status(), 200);

    let after: Value = resp.json().await.expect("Invalid JSON");
    let settled = after["notices"]
        .as_array()
        .expect("`notices` must be an array")
        .iter()
        .find(|n| n["id"].as_str() == Some(owed.as_str()))
        .unwrap_or_else(|| panic!("the answered notice stays in the panel list: {after}"));
    assert_eq!(settled["resolved"], Value::Bool(true));
    assert_ne!(
        after["next_id"].as_str(),
        Some(owed.as_str()),
        "the answered notice must not still be the one owed"
    );
}
