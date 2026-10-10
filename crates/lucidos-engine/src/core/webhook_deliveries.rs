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
//! # The claim is one statement, on purpose
//!
//! Two copies of one delivery can be in flight at once, so "read, then decide,
//! then write" is a race with no lock to take. [`DeliveryLedger::claim`] pushes
//! the whole decision into Postgres, where the primary key serialises them and
//! exactly one caller comes back holding the claim.

use sqlx::PgPool;
use uuid::Uuid;

/// The longest a caller may hold a claim, and the age at which [`prune`] drops a
/// row. One constant for both: a horizon shorter than the window would expire a
/// claim the ledger still owes an answer for.
///
/// [`prune`]: DeliveryLedger::prune
pub const MAX_WINDOW_SECS: i64 = 7 * 24 * 60 * 60;

/// How many times [`DeliveryLedger::claim`] re-runs when the key turns out to
/// be held by nobody.
///
/// That happens when the holder releases a failed emit between the claim
/// statement and the read that follows it. The key is then free, so retrying
/// wins it. Reporting a duplicate instead would 2xx a delivery that emitted
/// nothing, which loses it.
const CLAIM_ATTEMPTS: usize = 3;

/// What [`DeliveryLedger::claim`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// This caller owns the delivery and must emit. Nothing else holds the key,
    /// either because it is new or because the previous claim aged out.
    ///
    /// `claim_id` is proof of ownership, and every later write takes it. An
    /// emit outrunning its own window can be replaced mid-flight. Without the
    /// token, this caller would then record its event against the claim that
    /// replaced it, or delete it outright.
    Won { claim_id: Uuid },
    /// Somebody else already emitted this delivery, and here is what it emitted.
    /// `None` means the first copy is still in flight, which is honest rather
    /// than a missing answer: the event has no id yet.
    Duplicate { event_id: Option<Uuid> },
}

/// The record of which deliveries a webhook has already emitted.
pub struct DeliveryLedger;

impl DeliveryLedger {
    /// Take the claim for one delivery, or learn that it is a resend.
    ///
    /// The statement covers all three cases. A fresh key inserts. A key older
    /// than `window_secs` is taken over, its timestamp reset and its event id
    /// cleared. A key inside the window fails the `WHERE`, so `DO UPDATE` skips
    /// and no row returns, which is the whole duplicate test.
    ///
    /// `window_secs` reaches SQL as a duration rather than a computed cutoff, so
    /// the host clock never has to agree with the database clock (ADR 0053).
    pub async fn claim(
        pool: &PgPool,
        webhook_id: Uuid,
        delivery_key: &str,
        window_secs: i64,
    ) -> Result<Claim, Box<dyn std::error::Error + Send + Sync>> {
        for _ in 0..CLAIM_ATTEMPTS {
            let claim_id = Uuid::new_v4();
            let won: Option<(Uuid,)> = sqlx::query_as(
                "INSERT INTO webhook_deliveries (webhook_id, delivery_key, claim_id) \
                 VALUES ($1, $2, $4) \
                 ON CONFLICT (webhook_id, delivery_key) DO UPDATE \
                     SET created = NOW(), event_id = NULL, claim_id = $4 \
                     WHERE webhook_deliveries.created < NOW() - make_interval(secs => $3) \
                 RETURNING claim_id",
            )
            .bind(webhook_id)
            .bind(delivery_key)
            .bind(window_secs as f64)
            .bind(claim_id)
            .fetch_optional(pool)
            .await?;
            if won.is_some() {
                return Ok(Claim::Won { claim_id });
            }

            // Somebody holds it. Report what they emitted, so the sender's
            // retry gets the answer its first attempt did.
            let held: Option<(Option<Uuid>,)> = sqlx::query_as(
                "SELECT event_id FROM webhook_deliveries \
                 WHERE webhook_id = $1 AND delivery_key = $2",
            )
            .bind(webhook_id)
            .bind(delivery_key)
            .fetch_optional(pool)
            .await?;
            if let Some((event_id,)) = held {
                return Ok(Claim::Duplicate { event_id });
            }
            // No row at all, so the holder released a failed emit in the gap
            // between those two statements. The key is free, so go round again
            // rather than calling an un-emitted delivery a duplicate.
        }
        Err("the delivery claim kept being released underneath us".into())
    }

    /// Record which event the claimed delivery emitted.
    ///
    /// Scoped to `claim_id`, so a caller whose emit outran the window updates
    /// nothing rather than stamping its event onto a successor's claim.
    pub async fn record_event(
        pool: &PgPool,
        webhook_id: Uuid,
        delivery_key: &str,
        claim_id: Uuid,
        event_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "UPDATE webhook_deliveries SET event_id = $4 \
             WHERE webhook_id = $1 AND delivery_key = $2 AND claim_id = $3",
        )
        .bind(webhook_id)
        .bind(delivery_key)
        .bind(claim_id)
        .bind(event_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Give the claim back, because the emit it was taken for failed.
    ///
    /// Otherwise the sender's retry answers "duplicate" for an event that never
    /// happened. That is the one path where this feature could lose an event
    /// rather than merely repeat one.
    ///
    /// Scoped to `claim_id` for the reason [`record_event`] is: a stale owner
    /// must not delete the claim that replaced it.
    ///
    /// [`record_event`]: DeliveryLedger::record_event
    pub async fn release(
        pool: &PgPool,
        webhook_id: Uuid,
        delivery_key: &str,
        claim_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "DELETE FROM webhook_deliveries \
             WHERE webhook_id = $1 AND delivery_key = $2 AND claim_id = $3",
        )
        .bind(webhook_id)
        .bind(delivery_key)
        .bind(claim_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Drop claims nothing can still be waiting on. Returns how many went.
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
