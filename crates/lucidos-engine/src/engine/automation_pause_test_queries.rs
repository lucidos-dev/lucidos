/// Every step request on the thread, posted or queued, once each: its action
/// and its option labels.
async fn step_requests(pool: &PgPool, thread_id: Uuid) -> Vec<(String, Vec<String>)> {
    let rows: Vec<(Option<String>, serde_json::Value)> = sqlx::query_as(&format!(
        "SELECT action, options FROM ( \
           SELECT DISTINCT ON (payload->>'tool_use_id') sequence, \
             payload->'permission_request'->>'action' AS action, payload->'options' AS options \
           FROM events WHERE thread_id = $1 AND event_type IN ({}) \
           ORDER BY payload->>'tool_use_id', sequence) cards \
         ORDER BY sequence",
        crate::engine::question_park_sql::CARD_EVENT_TYPES_SQL,
    ))
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(action, options)| {
            let labels = options
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["label"].as_str().unwrap().to_string())
                .collect();
            (action.unwrap_or_default(), labels)
        })
        .collect()
}

async fn status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}
