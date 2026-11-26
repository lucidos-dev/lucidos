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

    let releases = body["releases"].as_array().expect("`releases` array");
    let grouped: Vec<_> = releases
        .iter()
        .flat_map(|release| {
            let items = release["items"].as_array().expect("release items array");
            assert!(!items.is_empty());
            assert!(items.iter().all(|item| item["since"] == release["since"]));
            assert_eq!(
                release["resolved"],
                items.iter().all(|item| item["resolved"] == true)
            );
            items
        })
        .collect();
    assert_eq!(
        grouped.iter().map(|item| &item["id"]).collect::<Vec<_>>(),
        notices.iter().map(|item| &item["id"]).collect::<Vec<_>>(),
    );
    assert_eq!(
        body["owed_through"].as_str(),
        grouped
            .iter()
            .rev()
            .find(|item| item["resolved"] == false)
            .and_then(|item| item["id"].as_str())
    );

    for notice in notices {
        for field in ["id", "since", "title", "summary", "body"] {
            let value = notice[field]
                .as_str()
                .unwrap_or_else(|| panic!("every notice needs {field}: {notice}"));
            assert!(!value.trim().is_empty(), "empty {field}: {notice}");
        }
        assert!(
            notice["resolved"].is_boolean(),
            "the panel reads `resolved` per notice: {notice}"
        );
        assert_eq!(
            notice["action_label"].is_string(),
            notice["action_prompt"].is_string(),
            "a label requires a prompt: {notice}"
        );
    }
}

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

    let resp = user_client()
        .await
        .post(format!("{}/resolve", list_url()))
        .json(&serde_json::json!({ "id": settled }))
        .send()
        .await
        .expect("resolve request failed");
    assert_eq!(resp.status(), 200);

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
