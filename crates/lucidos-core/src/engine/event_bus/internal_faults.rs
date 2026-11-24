use super::*;
use crate::internal_faults::FaultKind;

impl EventBus {
    /// Stable ids make an inbox replay after commit/exit harmless, including
    /// when a report was sent between the commit and the acknowledgement.
    pub(crate) async fn emit_internal_fault(
        &self,
        id: Uuid,
        kind: FaultKind,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        self.append_diagnostic_signal(id, SystemEvent::InternalFaultRecorded { kind }, 0, None)
            .await
    }

    pub(super) async fn append_diagnostic_signal(
        &self,
        id: Uuid,
        event: SystemEvent,
        depth: u32,
        trigger: Option<String>,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        let mut append = self.begin_append().await?;
        // All fault kinds and the report boundary share this transaction lock,
        // across buses and process restarts. No in-memory counter is authority.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(0x6469_6167_6661_756c_i64)
            .execute(&mut *append.tx)
            .await?;
        if let SystemEvent::InternalFaultRecorded { kind } = event {
            let fresh = sqlx::query(
                "INSERT INTO internal_fault_receipts (id) VALUES ($1) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .execute(&mut *append.tx)
            .await?
            .rows_affected()
                != 0;
            if !fresh {
                append.tx.rollback().await?;
                return Ok(None);
            }
            let duplicate: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events WHERE id = $1 OR
                 (event_type = 'InternalFaultRecorded' AND payload->'data'->>'kind' = $2
                  AND sequence > (SELECT COALESCE(MAX(sequence), 0) FROM events
                                  WHERE event_type = 'DiagnosticBundleSent')))",
            )
            .bind(id)
            .bind(kind.as_str())
            .fetch_one(&mut *append.tx)
            .await?;
            if duplicate {
                append.tx.commit().await?;
                return Ok(None);
            }
        }
        self.append_system_event(append, id, event, depth, trigger)
            .await
            .map(Some)
    }
}
