use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};

async fn post_follow_up(
    client: &reqwest::Client,
    child_thread_id: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    client
        .post(format!(
            "{}/api/v1/threads/{child_thread_id}/follow-up",
            base_url()
        ))
        .json(&body)
        .send()
        .await
        .expect("follow-up request sends")
}

#[tokio::test]
async fn follow_up_route_refuses_a_caller_less_request() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let child = uuid::Uuid::new_v4();
    seed_chat_thread_summary(&pool, child, "idle").await;

    let resp = post_follow_up(
        &client,
        &child.to_string(),
        serde_json::json!({ "message": "go the other way" }),
    )
    .await;

    assert_eq!(resp.status(), 403, "a caller-less follow-up must be 403");
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    let msg = body["error"].as_str().expect("error is a string");
    assert!(
        msg.contains("caller thread"),
        "the refusal must name what is missing, got: {msg}"
    );
}

#[tokio::test]
async fn follow_up_route_creates_nothing_when_it_refuses() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let phantom = uuid::Uuid::new_v4();

    let resp = post_follow_up(
        &client,
        &phantom.to_string(),
        serde_json::json!({ "message": "into the void" }),
    )
    .await;
    assert!(!resp.status().is_success());

    let materialised: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(phantom)
            .fetch_one(&pool)
            .await
            .expect("existence query");
    assert!(
        !materialised,
        "a refused follow-up must never materialise the thread it addressed"
    );
}

#[tokio::test]
async fn follow_up_route_rejects_a_malformed_thread_id() {
    let client = http_client();
    let resp = post_follow_up(
        &client,
        "not-a-uuid",
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(body["error"]
        .as_str()
        .expect("error is a string")
        .contains("Invalid thread id"));
}

#[tokio::test]
async fn follow_up_route_rejects_an_empty_message() {
    let client = http_client();
    let child = uuid::Uuid::new_v4();
    for body in [
        serde_json::json!({ "message": "" }),
        serde_json::json!({ "message": "   \n  " }),
    ] {
        let resp = post_follow_up(&client, &child.to_string(), body).await;
        assert_eq!(resp.status(), 400, "an empty message must be 400");
    }
    let resp = post_follow_up(&client, &child.to_string(), serde_json::json!({})).await;
    assert!(
        resp.status().is_client_error(),
        "a message-less body is the caller's error, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn follow_up_route_is_mounted_and_post_only() {
    let client = http_client();
    let url = format!(
        "{}/api/v1/threads/{}/follow-up",
        base_url(),
        uuid::Uuid::new_v4()
    );
    let resp = client.get(&url).send().await.expect("GET sends");
    assert_eq!(
        resp.status(),
        405,
        "the path must resolve (405), not 404, which would mean it is unmounted"
    );
}

#[test]
fn cli_follow_up_refuses_a_title_instead_of_an_id() {
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args([
            "threads",
            "follow-up",
            "--thread",
            "Research the pricing page",
            "--message",
            "go the other way",
        ])
        .output()
        .expect("lucidos threads follow-up runs");
    assert!(!out.status.success(), "a title is not a thread id");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--my-children"),
        "the refusal must say how to find the id, got: {stderr}"
    );
}

#[test]
fn cli_follow_up_reaches_the_route() {
    let marker = unique_marker("cli-follow-up");
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args([
            "threads",
            "follow-up",
            "--thread",
            &uuid::Uuid::new_v4().to_string(),
            "--message",
            &marker,
        ])
        .output()
        .expect("lucidos threads follow-up runs");
    assert!(!out.status.success(), "a token-less CLI call is refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("403"),
        "the CLI must surface the engine's refusal verbatim, got: {stderr}"
    );
}

#[tokio::test]
async fn urgent_follow_up_is_still_subject_to_the_refusal_ladder() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let child = uuid::Uuid::new_v4();
    seed_chat_thread_summary(&pool, child, "running").await;

    let resp = post_follow_up(
        &client,
        &child.to_string(),
        serde_json::json!({ "message": "stop the run", "urgent": true }),
    )
    .await;

    assert_eq!(
        resp.status(),
        403,
        "urgency must not buy a caller-less request any reach"
    );
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(
        !body["error"]
            .as_str()
            .expect("error is a string")
            .is_empty(),
        "the refusal keeps the standard error body"
    );
}

#[tokio::test]
async fn follow_up_refuses_a_non_boolean_urgent() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let child = uuid::Uuid::new_v4();
    seed_chat_thread_summary(&pool, child, "running").await;

    let resp = post_follow_up(
        &client,
        &child.to_string(),
        serde_json::json!({ "message": "stop the run", "urgent": "yes" }),
    )
    .await;

    assert_eq!(
        resp.status(),
        422,
        "a non-boolean urgent must be rejected, never coerced to not-urgent"
    );
}

#[test]
fn cli_follow_up_sends_urgent_only_when_asked() {
    for (args, label) in [
        (vec!["--urgent"], "with --urgent"),
        (vec![], "without --urgent"),
    ] {
        let mut cmd = crate::lucidos_cli_test::lucidos_cmd();
        cmd.args([
            "threads",
            "follow-up",
            "--thread",
            &uuid::Uuid::new_v4().to_string(),
            "--message",
            &unique_marker("cli-urgent"),
        ]);
        cmd.args(&args);
        let out = cmd.output().expect("lucidos threads follow-up runs");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("403"),
            "{label}: the body must parse and reach the ladder, got: {stderr}"
        );
    }
}
