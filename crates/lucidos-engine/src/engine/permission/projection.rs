//! The `permissions` table: a projection of `PermissionGranted`,
//! `PermissionSpent`, `PermissionRevoked` and `ChangeApplied`, written in the
//! thread projection's transaction.

use uuid::Uuid;

use super::{action_wire, Action, Holder, Lifetime};

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
}

pub(crate) async fn write_granted(
    tx: Tx<'_, '_>,
    thread_id: Uuid,
    granted: Granted<'_>,
) -> Result<(), BoxError> {
    let (holder_kind, holder_id) = granted.holder.columns();
    sqlx::query(
        "INSERT INTO permissions (permission_id, action, breadth, lifetime, holder_kind, \
           holder_id, granted_in_thread_id, request_tool_use_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (permission_id) DO NOTHING",
    )
    .bind(granted.permission_id)
    .bind(action_wire(granted.action)?)
    .bind(granted.breadth)
    .bind(granted.lifetime.wire())
    .bind(holder_kind)
    .bind(holder_id)
    .bind(thread_id)
    .bind(granted.request_tool_use_id)
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
