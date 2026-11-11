//! E2E for the thread people routes (ADR 0623).
//!
//! What it pins, against a real engine:
//!
//! * `GET /api/v1/threads/:id/people` names the family root, and a 404 for an
//!   unknown thread
//! * Take and Watch either act as the caller's person or refuse a device no
//!   person holds with a 409 `no_person`. Which one depends on whether the
//!   run's gateway lists operators, so both answers are pinned.
//! * A repeat that changes nothing records nothing

use crate::support::{
    base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client, with_pool_room,
};
use uuid::Uuid;

async fn count_events(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(pool)
        .await
        .expect("DB query failed")
}

async fn post(url: &str) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = user_client()
        .await
        .post(url)
        .send()
        .await
        .expect("request failed");
    let status = resp.status();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn thread_people_are_read_taken_and_watched_over_http() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let unknown = user_client()
        .await
        .get(format!(
            "{}/api/v1/threads/{}/people",
            base_url(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .expect("request failed");
    assert_eq!(unknown.status(), 404);

    // A human chat waits for a free pool slot before its thread row exists.
    // The rest of the suite pins the pool at its ceiling.
    let client = user_client().await;
    with_pool_room(&client, async move { roles_round_trip(&pool).await }).await;
}

async fn roles_round_trip(pool: &sqlx::PgPool) {
    let marker = unique_marker("api-thread-people");
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({ "message": marker, "mode": "human" }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(pool, &marker, 20)
        .await
        .thread_id;

    let url = format!("{}/api/v1/threads/{}/people", base_url(), thread_id);
    let roles: serde_json::Value = user_client()
        .await
        .get(&url)
        .send()
        .await
        .expect("request failed")
        .json()
        .await
        .expect("roles body");
    assert_eq!(roles["root_thread_id"], thread_id.to_string());
    assert!(roles["watchers"].is_array());

    for (verb, event_type) in [
        ("take", "WorkItemDriverTaken"),
        ("watch", "WorkItemWatched"),
    ] {
        let (status, body) = post(&format!("{url}/{verb}")).await;
        if status == 409 {
            assert_eq!(body["reason"], "no_person", "{verb}: {body}");
            assert_eq!(count_events(pool, thread_id, event_type).await, 0);
            continue;
        }
        assert_eq!(status, 200, "{verb}: {body}");
        let recorded = count_events(pool, thread_id, event_type).await;
        assert!(recorded <= 1, "{verb} recorded {recorded} events");
        let (again, _) = post(&format!("{url}/{verb}")).await;
        assert_eq!(again, 200);
        assert_eq!(count_events(pool, thread_id, event_type).await, recorded);
    }
    let (status, body) = post(&format!("{url}/watch")).await;
    if status == 200 {
        assert_eq!(body["watchers"].as_array().map(Vec::len), Some(1));
    }
}
