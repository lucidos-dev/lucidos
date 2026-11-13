//! Durable no-replay floor derived from the verified external restore journal.
//! Installing this observation does not complete a restore or permit dispatch.
use super::{first_boot, store::Store, BoxError};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// SQL for the event sequence the last restore installed, or 0 when this
/// workspace was never restored. Work at or below it is archived. A macro, so
/// a SQL literal can splice it with `concat!`.
macro_rules! restore_floor_sql {
    () => {
        "COALESCE((SELECT event_sequence FROM snapshot_restore_boundary WHERE singleton), 0)"
    };
}
pub(crate) use restore_floor_sql;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Boundary {
    pub restore_id: uuid::Uuid,
    pub snapshot: String,
    pub restored_at: DateTime<Utc>,
    pub event_sequence: i64,
}

pub async fn read(pool: &PgPool) -> Result<Option<Boundary>, sqlx::Error> {
    sqlx::query_as("SELECT restore_id, snapshot, restored_at, event_sequence FROM snapshot_restore_boundary WHERE singleton")
        .fetch_optional(pool).await
}

/// Install under owner + archive locks, after migrations and before Engine is
/// constructed. The journal remains FilesRestored until every settlement has
/// succeeded. Repeating after a crash must retain the original event floor;
/// otherwise newly submitted work could be mistaken for archived work.
pub async fn install(
    pool: &PgPool,
    workspace: &std::path::Path,
    store: &Store,
) -> Result<Boundary, BoxError> {
    let (marker, _) =
        first_boot::pending_restore(workspace, store)?.ok_or("snapshot_restore_marker_required")?;
    let mut tx = pool.begin().await?;
    sqlx::query("LOCK TABLE snapshot_restore_boundary IN EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    // No engine dispatch may exist here. Lock out concurrent event appends as
    // well, so every row in the restored history is below the recorded floor.
    sqlx::query("LOCK TABLE events IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    let boundary: Boundary = sqlx::query_as(
        "INSERT INTO snapshot_restore_boundary (singleton, restore_id, snapshot, restored_at, event_sequence) \
         SELECT TRUE, $1, $2, $3, COALESCE(MAX(sequence), 0) FROM events \
         ON CONFLICT (singleton) DO UPDATE SET \
           event_sequence = CASE WHEN snapshot_restore_boundary.restore_id = EXCLUDED.restore_id \
             THEN snapshot_restore_boundary.event_sequence ELSE EXCLUDED.event_sequence END, \
           restore_id = EXCLUDED.restore_id, snapshot = EXCLUDED.snapshot, restored_at = EXCLUDED.restored_at \
         WHERE snapshot_restore_boundary.restore_id <> EXCLUDED.restore_id OR \
           (snapshot_restore_boundary.snapshot = EXCLUDED.snapshot AND snapshot_restore_boundary.restored_at = EXCLUDED.restored_at) \
         RETURNING restore_id, snapshot, restored_at, event_sequence")
        .bind(marker.restore_id).bind(marker.snapshot.as_str()).bind(marker.restored_at)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(boundary)
}

/// A failed read is not permission to replay an old scheduled slot.
pub async fn permits_slot(pool: &PgPool, slot: DateTime<Utc>) -> Result<bool, sqlx::Error> {
    Ok(read(pool)
        .await?
        .is_none_or(|boundary| slot > boundary.restored_at))
}
