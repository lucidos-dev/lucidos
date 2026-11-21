use super::*;
use crate::diagnostic_bundles::Incoming;

impl EventBus {
    /// Store and announce one receipt atomically, across retries and restarts.
    /// The immutable event retains its identity after inbox expiry/deletion.
    pub async fn receive_diagnostic_bundle(
        &self,
        incoming: &Incoming,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let mut append = self.begin_append().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
            .bind(0x6469_6162_i32)
            .bind(incoming.id().to_string())
            .execute(&mut *append.tx)
            .await?;
        let event = SystemEvent::DiagnosticBundleReceived {
            bundle_id: incoming.id(),
            origin: incoming.origin().to_owned(),
            digest: incoming.digest().to_owned(),
            size_bytes: incoming.payload().len() as u64,
        };
        let existing: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
                .bind(incoming.id())
                .fetch_optional(&mut *append.tx)
                .await?;
        if let Some(existing) = existing {
            if existing["type"] != "DiagnosticBundleReceived"
                || existing["data"]["digest"] != incoming.digest()
                || existing["data"]["origin"] != incoming.origin()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "Diagnostic delivery identity conflict",
                )
                .into());
            }
            append.tx.rollback().await?;
            return Ok(incoming.id());
        }
        sqlx::query(
            "INSERT INTO diagnostic_bundles (id, origin, digest, payload) VALUES ($1, $2, $3, $4)",
        )
        .bind(incoming.id())
        .bind(incoming.origin())
        .bind(incoming.digest())
        .bind(incoming.payload())
        .execute(&mut *append.tx)
        .await?;
        // An arrival is ingress, even when fetch was run by a cron trigger.
        self.append_system_event(append, incoming.id(), event, 0, None)
            .await?;
        Ok(incoming.id())
    }

    /// Retention is expiry of previously announced ingress, not a new choice.
    pub async fn expire_diagnostic_bundles(
        &self,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        Ok(sqlx::query(
            "DELETE FROM diagnostic_bundles WHERE received <= now() - interval '90 days'",
        )
        .execute(self.pool())
        .await?
        .rows_affected())
    }
}
