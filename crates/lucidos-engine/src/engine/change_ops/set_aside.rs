use super::*;
use crate::engine::types::ChangeClaim;

/// What an Apply of a set-aside change answers. Every apply path guards on
/// `pending`, so the user brings the change back first (ADR 0328).
pub(crate) const SET_ASIDE_APPLY_REFUSAL: &str =
    "This change is set aside. Bring it back before applying it.";

impl LucidosEngine {
    /// Keep a pending change for later: out of Review, attention and every
    /// bulk path, and no longer blocking Archive. The branch is untouched.
    ///
    /// A user-facing caller asks `change_action_refusal` first. This refuses
    /// only what no caller may do: a change that is not pending, or one an
    /// apply or discard is already working on.
    pub(crate) async fn set_aside_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;
        match change.status() {
            ChangeStatus::Pending => {}
            ChangeStatus::SetAside => return Ok(()),
            status => return Err(format!("Change is already {status}").into()),
        }
        if change.merge_worktree().is_some() {
            return Err(MERGE_OWNED_BY_RESOLVER_MESSAGE.into());
        }
        if let Some(thread_id) = change.thread_id {
            match self.change_claim_holder(thread_id).await {
                Some(ChangeClaim::Apply) => return Err("This change is being applied".into()),
                Some(ChangeClaim::Discard) => return Err("This change is being discarded".into()),
                None => {}
            }
        }
        self.emit_change_status(
            change.thread_id.unwrap_or(change_id),
            crate::engine::thread_events::ThreadEvent::ChangeSetAside {
                change_id: change_id.to_string(),
            },
            actor,
        )
        .await?;
        // A queued Apply All member that leaves `pending` must end its turn in
        // the batch, or the driver waits on an apply that never starts.
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
            change_id,
            crate::engine::apply_all_driver::WITHDRAWN_MEMBER_REASON.to_string(),
        ));
        self.broadcast_changes_updated().await;
        Ok(())
    }

    /// Return a set-aside change to pending, where Apply and Discard take it.
    pub(crate) async fn bring_back_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;
        match change.status() {
            ChangeStatus::SetAside => {}
            ChangeStatus::Pending => return Ok(()),
            status => return Err(format!("Change is already {status}").into()),
        }
        self.emit_change_status(
            change.thread_id.unwrap_or(change_id),
            crate::engine::thread_events::ThreadEvent::ChangeBroughtBack {
                change_id: change_id.to_string(),
            },
            actor,
        )
        .await?;
        self.broadcast_changes_updated().await;
        Ok(())
    }

    /// Emit a status event and wait for it to land. A failed emit is an
    /// error, never a move the projection did not see.
    async fn emit_change_status(
        &self,
        thread_id: Uuid,
        event: crate::engine::thread_events::ThreadEvent,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let name = event.event_type();
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event,
                meta: crate::engine::thread_events::EventMeta::with_actor(actor),
            })
            .await
            .map_err(|e| format!("could not record {name}: {e}"))?;
        Ok(())
    }
}
