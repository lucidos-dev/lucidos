use super::*;

impl LucidosEngine {
    /// If a pending change already exists for this branch, returns its ID instead of creating a duplicate.
    pub(crate) async fn propose_change(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
            incomplete,
        } = input;

        // If a pending change already exists for this branch, reuse its
        // change_id and re-emit `ChangeProposed`. The `needs_emit` guard
        // short-circuits when no field changed — without it, every CC
        // end-of-turn would re-emit identical events and inflate history.
        // `incomplete` participates in the dedup so a follow-up successful
        // turn against the same branch clears a prior failure tag (the
        // re-emit propagates `incomplete: false` to the projection row).
        let existing = self
            .changes()
            .get_pending_by_branch(branch_name)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("get_pending_by_branch({}): {}", branch_name, e).into()
            })?;
        let change_id = existing.as_ref().map(|c| c.id).unwrap_or_else(Uuid::new_v4);
        let needs_emit = existing.as_ref().is_none_or(|e| {
            e.description != description
                || e.files != files
                || e.requires_restart != requires_restart
                || e.hardened != hardened
                || e.incomplete != incomplete
        });

        if needs_emit {
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ChangeProposed {
                            change_id: change_id.to_string(),
                            description: Some(description.to_string()),
                            files: files.to_vec(),
                            requires_restart,
                            origin,
                            commit_sha: None,
                            branch_name: branch_name.to_string(),
                            repo_root: repo_root.to_string(),
                            hardened,
                            incomplete,
                            path: String::new(),
                            diff: String::new(),
                        },
                        meta: crate::engine::thread_events::EventMeta {
                            channel: Some(channel),
                            ..crate::engine::thread_events::EventMeta::NONE
                        },
                    },
                    "[Changes] ChangeProposed",
                )
                .await;
            if existing.is_some() {
                self.broadcast_changes_updated().await;
            }
        }
        Ok(change_id)
    }

    /// Emit the harden boundary event then queue AUTO_HARDEN_MESSAGE on a live
    /// agent session. Emit-before-send guarantees the panel sits above any
    /// CodingAgentTextStreamed events CC produces in response.
    ///
    /// The boundary event is stamped engine-origin internally (see
    /// `emit_missing_hardening_detected`), so callers don't pass an actor.
    pub(crate) async fn request_hardening_in_session(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_missing_hardening_detected(thread_id).await;
        msg_tx
            .send(crate::engine::AgentUserInput {
                text: crate::engine::claude_code::AUTO_HARDEN_MESSAGE.to_string(),
                images: None,
                origin_event_id: None,
                kind: crate::engine::AgentInputKind::User,
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        Ok(())
    }
}
