use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// INVARIANT: The longest dedupe window a hook may set, and the age at which [`prune`]
/// drops a row. One constant for both: a horizon shorter than the window would
/// drop a row the ledger still owes an answer for.
///
/// [`prune`]: DeliveryLedger::prune
pub const MAX_WINDOW_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct DeliveryKey {
    pub webhook_id: Uuid,
    pub key: String,
    pub window_secs: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Emitted(Uuid),
    Duplicate(Uuid),
}

/// INVARIANT: The record of which deliveries a webhook has already emitted.
pub struct DeliveryLedger;

impl DeliveryLedger {
    pub(crate) async fn record(
        tx: &mut Transaction<'_, Postgres>,
        delivery: &DeliveryKey,
        event_id: Uuid,
    ) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
        let won: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO webhook_deliveries (webhook_id, delivery_key, event_id) \
             VALUES ($1, $2, $4) \
             ON CONFLICT (webhook_id, delivery_key) DO UPDATE \
                 SET created = NOW(), event_id = $4 \
                 WHERE webhook_deliveries.created < NOW() - make_interval(secs => $3) \
             RETURNING event_id",
        )
        .bind(delivery.webhook_id)
        .bind(&delivery.key)
        .bind(delivery.window_secs as f64)
        .bind(event_id)
        .fetch_optional(&mut **tx)
        .await?;
        if won.is_some() {
            return Ok(None);
        }
        // INVARIANT: A new statement sees the first copy's committed row.
        let first: Uuid = sqlx::query_scalar(
            "SELECT event_id FROM webhook_deliveries \
             WHERE webhook_id = $1 AND delivery_key = $2",
        )
        .bind(delivery.webhook_id)
        .bind(&delivery.key)
        .fetch_one(&mut **tx)
        .await?;
        Ok(Some(first))
    }

    pub async fn prune(pool: &PgPool) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let done = sqlx::query(
            "DELETE FROM webhook_deliveries WHERE created < NOW() - make_interval(secs => $1)",
        )
        .bind(MAX_WINDOW_SECS as f64)
        .execute(pool)
        .await?;
        Ok(done.rows_affected())
    }
}

#[cfg(test)]
#[path = "webhook_deliveries_tests.rs"]
mod tests;
