use crate::support::{base_url, http_client, user_client};

fn url(suffix: &str) -> String {
    format!("{}/api/v1/frontend-preview{}", base_url(), suffix)
}

#[tokio::test]
async fn a_workspace_with_no_preview_says_so_and_offers_nothing_to_open() {
    let body: serde_json::Value = http_client()
        .get(url(""))
        .send()
        .await
        .expect("frontend-preview status request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(body["running"], false);
    for absent in ["port", "thread_id", "url", "worktree", "started_at"] {
        assert!(
            body.get(absent).is_none(),
            "a stopped preview must not report `{absent}`, got {body}"
        );
    }
}

#[tokio::test]
async fn a_thread_with_no_worktree_is_refused_by_name() {
    let resp = user_client()
        .await
        .post(url("/start"))
        .json(&serde_json::json!({ "thread_id": "00000000-0000-4000-8000-000000000000" }))
        .send()
        .await
        .expect("frontend-preview start request failed");

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(
        message.contains(".lucidos/worktrees"),
        "the refusal must name the worktree it looked for, got: {message}"
    );
}

#[tokio::test]
async fn a_malformed_thread_id_never_reaches_the_worktree_resolver() {
    let resp = user_client()
        .await
        .post(url("/start"))
        .json(&serde_json::json!({ "thread_id": "../../../etc" }))
        .send()
        .await
        .expect("frontend-preview start request failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "a non-uuid thread id must be refused, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn stopping_a_preview_that_is_not_running_is_a_no_op() {
    let body: serde_json::Value = user_client()
        .await
        .post(url("/stop"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("frontend-preview stop request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(body["running"], false);
}
