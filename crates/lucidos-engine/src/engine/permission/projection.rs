//! The `permissions` table: a projection of `PermissionGranted`,
//! `PermissionSpent`, `PermissionRevoked` and `ChangeApplied`, written in the
//! thread projection's transaction, and of `PermissionGrantsChanged`, written
//! in the system projection's.

use uuid::Uuid;

use super::{action_wire, Action, Holder, Lifetime, PickedBy};
use crate::core::grants::{GrantFile, GrantSource};

type Tx<'a, 'b> = &'a mut sqlx::Transaction<'b, sqlx::Postgres>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One granted permission, as its event carries it.
pub(crate) struct Granted<'a> {
    pub(crate) permission_id: Uuid,
    pub(crate) action: Action,
    pub(crate) breadth: Option<&'a str>,
    pub(crate) lifetime: Lifetime,
    pub(crate) holder: &'a Holder,
    pub(crate) request_tool_use_id: &'a str,
    pub(crate) pattern: Option<&'a str>,
    pub(crate) picked_by: &'a PickedBy,
}

/// Record a granted permission. A second live row for the same tool grant
/// line and holder is dropped, so a replay converges.
pub(crate) async fn write_granted(
    tx: Tx<'_, '_>,
    thread_id: Uuid,
    granted: Granted<'_>,
) -> Result<(), BoxError> {
    let (holder_kind, holder_id) = granted.holder.columns();
    let source = match granted.picked_by {
        PickedBy::Batch { .. } => "batch",
        PickedBy::Tap | PickedBy::Typed { .. } | PickedBy::Spoken { .. } => "card",
    };
    sqlx::query(
        "INSERT INTO permissions (permission_id, action, breadth, lifetime, holder_kind, \
           holder_id, granted_in_thread_id, request_tool_use_id, pattern, source) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING",
    )
    .bind(granted.permission_id)
    .bind(action_wire(granted.action)?)
    .bind(granted.breadth)
    .bind(granted.lifetime.wire())
    .bind(holder_kind)
    .bind(holder_id)
    .bind(thread_id)
    .bind(granted.request_tool_use_id)
    .bind(granted.pattern)
    .bind(source)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Make `grant_file`'s live standing rows equal `patterns`: end each row the
/// set dropped, and add a row for each line it gained. A thread that no longer
/// exists is not recorded as where a row was granted.
pub(crate) async fn write_standing_set(
    tx: Tx<'_, '_>,
    grant_file: GrantFile,
    patterns: &[String],
    source: GrantSource,
    granted_in_thread_id: Option<Uuid>,
) -> Result<(), BoxError> {
    let action = action_wire(super::tool_grants::lane_action(grant_file))?;
    sqlx::query(
        "UPDATE permissions SET ended_at = NOW(), end_reason = 'revoked' \
         WHERE action = $1 AND pattern IS NOT NULL AND holder_kind = 'workspace' \
           AND ended_at IS NULL AND NOT (pattern = ANY($2))",
    )
    .bind(&action)
    .bind(patterns)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO permissions (permission_id, action, lifetime, holder_kind, \
           granted_in_thread_id, pattern, source) \
         SELECT gen_random_uuid(), $1, 'standing', 'workspace', \
           (SELECT thread_id FROM thread_summaries WHERE thread_id = $3), line, $4 \
         FROM unnest($2::text[]) AS line \
         ON CONFLICT DO NOTHING",
    )
    .bind(&action)
    .bind(patterns)
    .bind(granted_in_thread_id)
    .bind(source.wire())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// End a live permission for `reason`, and say whether it was live. An ended
/// one keeps its first reason.
async fn end_live(tx: Tx<'_, '_>, permission_id: Uuid, reason: &str) -> Result<bool, BoxError> {
    let ended = sqlx::query(
        "UPDATE permissions SET ended_at = NOW(), end_reason = $2 \
         WHERE permission_id = $1 AND ended_at IS NULL",
    )
    .bind(permission_id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(ended.rows_affected() > 0)
}

/// End a live permission for `reason`. An ended one keeps its first reason.
pub(crate) async fn write_ended(
    tx: Tx<'_, '_>,
    permission_id: Uuid,
    reason: &str,
) -> Result<(), BoxError> {
    end_live(tx, permission_id, reason).await?;
    Ok(())
}

/// Spend a live permission. A spend of one that already ended, such as a
/// revoked one, fails, so its event rolls back and the action never runs.
pub(crate) async fn write_spent(tx: Tx<'_, '_>, permission_id: Uuid) -> Result<(), BoxError> {
    if end_live(tx, permission_id, "spent").await? {
        Ok(())
    } else {
        Err(format!("permission {permission_id} had already ended").into())
    }
}

/// The thread's change applied, so every *until the plan lands* permission it
/// holds ends.
pub(crate) async fn end_until_plan_lands(tx: Tx<'_, '_>, thread_id: Uuid) -> Result<(), BoxError> {
    sqlx::query(
        "UPDATE permissions SET ended_at = NOW(), end_reason = 'plan_landed' \
         WHERE holder_kind = 'thread' AND holder_id = $1 \
           AND lifetime = 'until_plan_lands' AND ended_at IS NULL",
    )
    .bind(thread_id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
