use crate::support::{base_url, unique_marker, user_client};

#[tokio::test]
async fn create_draft_refuses_a_caller_outside_a_thread() {
    let client = user_client().await;
    let marker = unique_marker("draft-refused");
    let resp = client
        .post(format!("{}/api/v1/threads/drafts", base_url()))
        .json(&serde_json::json!({
            "text": marker,
            "mode": "lucidos",
            "retry_key": "e2e",
        }))
        .send()
        .await
        .expect("create-draft request sends");
    assert_eq!(resp.status(), 403);
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(body["error"]
        .as_str()
        .expect("error is a string")
        .contains("agent inside a thread"));

    let drafts: serde_json::Value = client
        .get(format!("{}/api/v1/threads/drafts", base_url()))
        .send()
        .await
        .expect("drafts read sends")
        .json()
        .await
        .expect("drafts read is JSON");
    assert!(
        !drafts.to_string().contains(&marker),
        "a refused create writes no draft"
    );
}

#[test]
fn cli_create_draft_reaches_the_route() {
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args(["threads", "create-draft", "--text", "hello"])
        .output()
        .expect("lucidos threads create-draft runs");
    assert!(!out.status.success(), "a token-less CLI call is refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("403"),
        "the CLI must surface the engine's refusal, got: {stderr}"
    );
}
