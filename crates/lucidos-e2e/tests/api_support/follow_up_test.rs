//! Coverage for `POST /api/v1/threads/:thread_id/follow-up`, the HTTP surface
//! of a child follow-up.
//!
//! ## What this suite can and cannot reach, stated rather than implied
//!
//! Authorizing a follow-up requires a **thread-bound origin token**, which the
//! engine mints per spawn and hands out only through a subprocess's
//! environment. An HTTP client standing outside the engine has no way to
//! obtain one, and deliberately so: a test-only endpoint that minted a token
//! for an arbitrary thread would be gated on `debug_assertions`, which is
//! exactly the build `web-dev.sh` runs, so it would put a token minter on
//! every developer's live workspace.
//!
//! So this suite covers every refusal an unauthenticated caller can provoke,
//! plus the two things that must be true of a refusal (no thread created, and
//! the standard error body). The authorized delivery path is covered where a
//! token is constructible: the engine's own tests over
//! `LucidosEngine::follow_up_child_thread` and `mint_agent_origin_token`.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};

/// Every request here is token-less, so the engine resolves `NoCaller`
/// before it ever reads the target row. That is the point: this is the
/// posture of any caller that is not a Lucidos-spawned subprocess.
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

/// A caller with no origin token has no thread whose children could be looked
/// up, so it is refused before anything is read or written. 403, and the
/// message says what is missing rather than leaking whether the target exists.
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

/// The refusal ladder never creates a thread on any branch, which is the
/// invariant that keeps a typo'd uuid from silently spawning one.
///
/// Asserted against the addressed id specifically, not a table count: other
/// tests run in parallel against this workspace and legitimately add rows, so
/// a count comparison could only ever say "rows did not vanish", which nothing
/// here could make happen.
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

/// A malformed target is a 400 with an actionable message, never a 404 that
/// would read as "that thread is gone" or a 500.
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

/// An empty message is refused before the caller ladder runs, because an
/// empty follow-up would land a blank message in the child's conversation
/// even if it were authorized.
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
    // And a body with no message at all is a 422 from the extractor, not a
    // 500: the field is required, and serde says so before the handler runs.
    let resp = post_follow_up(&client, &child.to_string(), serde_json::json!({})).await;
    assert!(
        resp.status().is_client_error(),
        "a message-less body is the caller's error, got {}",
        resp.status()
    );
}

/// The route is mounted where the docs say it is. A `GET` on the same path
/// must 405 rather than 404, which is what proves the path itself resolved
/// (and that `:thread_id` did not collide with the `/threads/:id` leaf).
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

/// `lucidos threads follow-up` refuses a non-uuid before it makes a request,
/// with wording that points at the way to find the right id. Runs the real
/// binary, so it also proves the subcommand is wired into `main.rs`.
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

/// The CLI reaches the real route and gets the real refusal. Token-less, so
/// the expected outcome is `NoCaller`, which is still the full loop through
/// `client()`, the gateway-safe URL, and the handler.
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

/// `urgent` is accepted on the wire and changes nothing about authorization.
/// The refusal ladder runs before the flag is ever consulted, so an urgent
/// follow-up from a caller with no origin token is refused exactly like a plain
/// one. Worth pinning: `urgent` is the only field on this route that CHANGES
/// the child's state rather than describing the message, so a version of it
/// that skipped a gate would be the serious kind of bug.
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

/// A malformed `urgent` is a 400 from the body parse, not a silent coercion to
/// `false`. Silently reading `"true"` (the string) as not-urgent would drop a
/// cancellation on the floor and report success, which is the worst shape this
/// flag can fail in: the caller believes the child was stopped.
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

/// Omitting `urgent` is legal and means not urgent, so every caller written
/// before the flag existed keeps working unchanged. Proven here by the plain
/// body still reaching the same refusal rather than a parse error.
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
