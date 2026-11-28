async fn calls(pool: &PgPool, thread_id: Uuid, model: &str, tokens: u64) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
    )
    .bind(serde_json::json!({"model": model, "usage": {
        "input_tokens": tokens,
        "cache_creation_tokens": 1000,
        "cache_creation_1h_tokens": 0,
    }}))
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap();
}
