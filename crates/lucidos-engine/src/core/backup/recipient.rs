//! The backup recipient: the age public key every new archive is sealed to.
//!
//! The engine stores only this public half. The backup key, its private half,
//! is shown to the user once and never written anywhere. So after the switch,
//! nothing on the machine can open a new backup.
//!
//! A minted key pair seals nothing until the user pastes its backup key back
//! and the engine derives the same recipient from it. A recipient the user
//! brings themselves needs no such gate.

use std::path::Path;

use sqlx::PgPool;

use super::crypto::{self, SealingKey};
use super::key::BackupKeyStore;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the table holds, with ages measured by the database (ADR 0053).
#[derive(Debug, Default, Clone, sqlx::FromRow)]
pub struct RecipientState {
    /// The recipient new archives are sealed to, once confirmed.
    pub recipient: Option<String>,
    /// A minted recipient waiting for its backup key to be pasted back.
    pub pending_recipient: Option<String>,
    /// Seconds since the user last proved they hold the backup key. `None`
    /// until they have, as for a recipient they brought themselves.
    pub secs_since_verified: Option<i64>,
    /// Seconds since the recipient was set.
    pub secs_since_set: Option<i64>,
}

impl RecipientState {
    /// How long the backup key has gone unproven: since the last check, or
    /// since the recipient was set when there never was one.
    pub fn secs_unproven(&self) -> Option<i64> {
        self.secs_since_verified.or(self.secs_since_set)
    }
}

/// How a pasted backup key compared with the recipient it should match.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyCheck {
    Matches,
    Differs,
    /// There was nothing to compare it with.
    NothingToCheck,
}

pub async fn load(pool: &PgPool) -> Result<RecipientState, BoxError> {
    let state = sqlx::query_as::<_, RecipientState>(
        "SELECT recipient, pending_recipient, \
                EXTRACT(EPOCH FROM now() - verified_at)::bigint AS secs_since_verified, \
                EXTRACT(EPOCH FROM now() - recipient_set_at)::bigint AS secs_since_set \
         FROM backup_recipient",
    )
    .fetch_optional(pool)
    .await?;
    Ok(state.unwrap_or_default())
}

/// Mint a key pair and keep its recipient as pending. Returns the backup key,
/// which the caller shows once and then drops.
pub async fn start_setup(pool: &PgPool) -> Result<String, BoxError> {
    let (identity, recipient) = crypto::generate_key_pair();
    sqlx::query(
        "INSERT INTO backup_recipient (pending_recipient) VALUES ($1) \
         ON CONFLICT (singleton) DO UPDATE SET pending_recipient = EXCLUDED.pending_recipient",
    )
    .bind(recipient.to_string())
    .execute(pool)
    .await?;
    Ok(crypto::backup_key_text(&identity))
}

/// Promote the pending recipient when `pasted` is its backup key. The key is
/// compared and dropped, never stored.
pub async fn confirm(
    pool: &PgPool,
    bus: &EventBus,
    pasted: &age::x25519::Identity,
    actor: Option<MessageOrigin>,
) -> Result<KeyCheck, BoxError> {
    let derived = pasted.to_public().to_string();
    let mut tx = pool.begin().await?;
    let pending: Option<String> = sqlx::query_scalar(
        "SELECT pending_recipient FROM backup_recipient WHERE singleton FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(pending) = pending else {
        return Ok(KeyCheck::NothingToCheck);
    };
    if pending != derived {
        return Ok(KeyCheck::Differs);
    }
    sqlx::query(
        "UPDATE backup_recipient SET recipient = pending_recipient, pending_recipient = NULL, \
         recipient_set_at = now(), verified_at = now() WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    bus.emit_or_log(recipient_set(derived, false, actor), RECIPIENT_SET_CTX)
        .await;
    Ok(KeyCheck::Matches)
}

/// Set a recipient the user made themselves. Any pending minted pair goes.
pub async fn use_own(
    pool: &PgPool,
    bus: &EventBus,
    recipient: &age::x25519::Recipient,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    let recipient = recipient.to_string();
    sqlx::query(
        "INSERT INTO backup_recipient (recipient, recipient_set_at) VALUES ($1, now()) \
         ON CONFLICT (singleton) DO UPDATE SET recipient = EXCLUDED.recipient, \
         recipient_set_at = now(), verified_at = NULL, pending_recipient = NULL",
    )
    .bind(&recipient)
    .execute(pool)
    .await?;
    bus.emit_or_log(recipient_set(recipient, true, actor), RECIPIENT_SET_CTX)
        .await;
    Ok(())
}

/// Check that `pasted` still matches the recipient, and record when it does.
pub async fn verify(pool: &PgPool, pasted: &age::x25519::Identity) -> Result<KeyCheck, BoxError> {
    let derived = pasted.to_public().to_string();
    let Some(recipient) = load(pool).await?.recipient else {
        return Ok(KeyCheck::NothingToCheck);
    };
    if recipient != derived {
        return Ok(KeyCheck::Differs);
    }
    // Stamp only the recipient just compared, so a rotation landing between
    // the read and this write is never marked as held.
    let stamped = sqlx::query(
        "UPDATE backup_recipient SET verified_at = now() WHERE singleton AND recipient = $1",
    )
    .bind(&derived)
    .execute(pool)
    .await?;
    if stamped.rows_affected() == 0 {
        return Ok(KeyCheck::Differs);
    }
    Ok(KeyCheck::Matches)
}

const RECIPIENT_SET_CTX: &str = "[Backup] BackupRecipientSet";

fn recipient_set(recipient: String, own_key: bool, actor: Option<MessageOrigin>) -> BusEvent {
    BusEvent::System(SystemEvent::BackupRecipientSet {
        recipient,
        own_key,
        actor,
    })
}

/// What seals the next archive: the recipient when one is set, else the
/// legacy key of a workspace that has not switched. `None` means backups are
/// not set up, and nothing may be sealed.
pub async fn sealing_key(pool: &PgPool, workspace: &Path) -> Result<Option<SealingKey>, BoxError> {
    if let Some(recipient) = load(pool).await?.recipient {
        let recipient = recipient.parse().map_err(|e| {
            format!("the stored backup recipient is not a valid age public key: {e}")
        })?;
        return Ok(Some(SealingKey::Recipient(recipient)));
    }
    Ok(BackupKeyStore::for_engine(workspace)?
        .load()?
        .map(SealingKey::Legacy))
}

/// After a switch, forget the legacy key: no new archive is sealed with it,
/// and keeping it for old archives was always the user's job.
pub fn forget_legacy_key(workspace: &Path) {
    match BackupKeyStore::for_engine(workspace).and_then(|keys| keys.delete()) {
        Ok(()) => log!("[Backup] Switched to the backup recipient; forgot the legacy key"),
        Err(e) => log!(
            "[Backup] Switched to the backup recipient, but cannot delete the legacy key: {}",
            e
        ),
    }
}

#[cfg(test)]
#[path = "recipient_tests.rs"]
mod tests;
