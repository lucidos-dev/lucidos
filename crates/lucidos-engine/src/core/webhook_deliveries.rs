//! Delivery idempotency: recognising a resend before it emits a second event.
//!
//! Full design:
//! `docs/plans/2026-08-24-a-resent-delivery-fires-the-event-once.md`.
//!
//! # A nonce ledger, not a delivery log
//!
//! The `webhook_deliveries` row holds no payload, no surface lists it, and its
//! only reader is the next delivery. The emitted domain event stays the record
//! of what arrived. That distinction is what lets this exist beside the delivery
//! log the webhooks design refused (ADR 0122).
//!
//! # The row commits with its event
//!
//! The bus writes the row inside the domain event's own transaction, through
//! [`DeliveryLedger::record`] (ADR 0444). So the row and the event exist
//! together or not at all. A dropped handler or a restart rolls both back,
//! which leaves nothing to strand and nothing to release.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// The longest dedupe window a hook may set, and the age at which [`prune`]
/// drops a row. One constant for both: a horizon shorter than the window would
/// drop a row the ledger still owes an answer for.
///
/// [`prune`]: DeliveryLedger::prune
pub const MAX_WINDOW_SECS: i64 = 7 * 24 * 60 * 60;

/// Which delivery an emit is, for a hook that dedupes.
#[derive(Debug, Clone)]
pub struct DeliveryKey {
    pub webhook_id: Uuid,
    /// The digest from `webhooks::dedupe_key`.
    pub key: String,
    pub window_secs: i64,
}

/// What a deduped delivery came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// This delivery emitted the event.
    Emitted(Uuid),
    /// A resend inside the window. It emitted nothing, and this is the event
    /// the first delivery emitted.
    Duplicate(Uuid),
}

/// The record of which deliveries a webhook has already emitted.
pub struct DeliveryLedger;

impl DeliveryLedger {
    /// Record `event_id` as `delivery`'s event, inside the transaction that
    /// inserts the event. Returns the first delivery's event id when this one
    /// is a resend, and the caller then rolls back.
    ///
    /// A fresh key inserts. A key older than its window is taken over. A key
    /// inside the window fails the `WHERE`, so `DO UPDATE` skips and no row
    /// returns, which is the whole duplicate test. A copy still in its own
    /// transaction holds the key uncommitted, so this statement waits for it.
    ///
    /// The window reaches SQL as a duration rather than a computed cutoff, so
    /// the host clock never has to agree with the database clock (ADR 0053).
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
        // A new statement sees the first copy's committed row.
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

    /// Drop rows nothing can still be deduping against. Returns how many went.
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
