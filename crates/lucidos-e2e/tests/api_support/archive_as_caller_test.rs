use crate::support::{base_url, count_events_of_type, db_url, http_client, user_client};
use uuid::Uuid;

async fn seed(pool: &sqlx::PgPool, thread_id: Uuid, parent: Option<Uuid>, status: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, source, is_coding_agent, created_at, \
             last_activity, message_count, status, archive_state, has_response, state \
         ) VALUES ($1, $2, 'chat', FALSE, NOW(), NOW(), 0, $3, 'inbox', TRUE, 'active')",
    )
    .bind(thread_id)
    .bind(parent)
    .bind(status)
    .execute(pool)
    .await
    .expect("seed thread_summaries row");
}

async fn cleanup(pool: &sqlx::PgPool, ids: &[Uuid]) {
    let _ = sqlx::query("DELETE FROM events WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
}

async fn post_archive(client: &reqwest::Client, thread: &str) -> reqwest::Response {
    client
        .post(format!("{}/api/v1/threads/{thread}/archive", base_url()))
        .send()
        .await
        .expect("archive request sends")
}

async fn archive_state(pool: &sqlx::PgPool, thread: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_user_archives_a_thread_and_its_sub_threads() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let (parent, child, grandchild) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    seed(&pool, parent, None, "idle").await;
    seed(&pool, child, Some(parent), "idle").await;
    seed(&pool, grandchild, Some(child), "idle").await;

    let resp = post_archive(&client, &child.to_string()).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json ack");
    let archived: Vec<String> = serde_json::from_value(body["archived"].clone()).unwrap();
    assert!(archived.contains(&child.to_string()), "{body}");
    assert!(archived.contains(&grandchild.to_string()), "{body}");

    assert_eq!(archive_state(&pool, child).await, "archived");
    assert_eq!(archive_state(&pool, grandchild).await, "archived");
    assert_eq!(archive_state(&pool, parent).await, "inbox");
    assert_eq!(
        count_events_of_type(&pool, child, "ThreadArchived").await,
        1
    );
    assert_eq!(
        count_events_of_type(&pool, parent, "ThreadArchived").await,
        0
    );

    cleanup(&pool, &[parent, child, grandchild]).await;
}

#[tokio::test]
async fn a_waiting_thread_is_refused_like_the_archive_route() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let waiting = Uuid::new_v4();
    seed(&pool, waiting, None, "waiting_for_user_answer").await;

    for url in [
        format!("{}/api/v1/threads/{waiting}/archive", base_url()),
        format!("{}/api/v1/threads/archive", base_url()),
    ] {
        let resp = client
            .post(&url)
            .json(&serde_json::json!({ "thread_id": waiting.to_string() }))
            .send()
            .await
            .expect("archive request sends");
        assert_eq!(resp.status(), 409, "{url}");
        let body: serde_json::Value = resp.json().await.expect("json refusal");
        assert_eq!(body["reason"], "parent_not_archivable", "{url}: {body}");
        assert_eq!(body["parent_status"], "waiting_for_user_answer", "{url}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|m| m.contains("waiting on the user")),
            "{url}: {body}"
        );
    }
    assert_eq!(archive_state(&pool, waiting).await, "inbox");
    assert_eq!(
        count_events_of_type(&pool, waiting, "ThreadArchived").await,
        0
    );

    cleanup(&pool, &[waiting]).await;
}

#[tokio::test]
async fn the_route_refuses_what_it_cannot_name() {
    let client = user_client().await;
    assert_eq!(post_archive(&client, "current").await.status(), 400);
    assert_eq!(post_archive(&client, "not-a-uuid").await.status(), 400);
    assert_eq!(
        post_archive(&client, &Uuid::new_v4().to_string())
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn the_route_refuses_a_caller_with_no_credential() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let thread = Uuid::new_v4();
    seed(&pool, thread, None, "idle").await;

    assert_eq!(
        post_archive(&http_client(), &thread.to_string())
            .await
            .status(),
        401
    );
    assert_eq!(archive_state(&pool, thread).await, "inbox");

    cleanup(&pool, &[thread]).await;
}
