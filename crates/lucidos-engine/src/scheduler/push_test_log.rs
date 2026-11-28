use sqlx::PgPool;
use uuid::Uuid;

pub async fn record(
    pool: &PgPool,
    device_id: &str,
    notification_id: Uuid,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("INSERT INTO push_log (device_id, notification_id, payload) VALUES ($1, $2, $3)")
        .bind(device_id)
        .bind(notification_id)
        .bind(payload)
        .execute(pool)
        .await?;
    Ok(())
}
