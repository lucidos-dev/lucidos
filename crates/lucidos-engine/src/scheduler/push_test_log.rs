//! Test-only stub for the web-push transport.
//!
//! When the `e2e-test-hooks` feature is enabled, the per-subscription
//! `client.send(...)` call in [`crate::scheduler::push::fan_out_payload`]
//! is replaced with a write to the `push_log` table (see migration
//! `20260520120438_push_log_for_e2e_assertions.sql`). Browser e2e tests
//! then read the log via `GET /api/v1/_test/push-log` to assert
//! "OS push WAS sent for this notification on this device" without
//! waiting for actual APNs/FCM delivery.
//!
//! See `system-knowhow/notifications.md` §5.4 for the test harness.
//!
//! Production binaries never compile this module.

use sqlx::PgPool;
use uuid::Uuid;

/// Append one `push_log` row including the on-wire payload bytes. Returns
/// `Err` only on DB failure; the caller surfaces the error to the
/// operator-facing log just as the real transport would surface an APNs/FCM
/// error. `payload` is the JSON string the real transport would have
/// encrypted and sent — recording it lets e2e tests assert the Declarative
/// Web Push envelope shape, not just delivery.
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
