use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::types::ChangeClaim;

/// What Bring back answers when it cannot read an incomplete row's branch.
const BRING_BACK_UNREADABLE: &str =
    "Lucidos could not read this change's branch work, so it stays set aside. Try again.";

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
        emit_change_status(
            &self.event_bus,
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
    ///
    /// Incomplete work never becomes a pending change (ADR 0400). Such a row
    /// is withdrawn instead, and its branch decided again as its last turn
    /// ended: proposed under a fresh id, or withheld with the real reason.
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
        if change.incomplete {
            let thread_id = change
                .thread_id
                .ok_or("A set-aside change with no thread")?;
            // Read the branch before withdrawing, so a git failure leaves the
            // row where the user can still bring it back. A branch with no
            // work left reads fine: its row goes, and nothing is proposed.
            let repo_root = Path::new(&change.repo_root);
            if let Err(e) =
                crate::engine::git_ops::branch_changed_files_checked(repo_root, &change.branch_name)
                    .await
            {
                log!("[Changes] Bring back of {}: {}", change_id, e);
                return Err(BRING_BACK_UNREADABLE.into());
            }
            let finished = withdraw_for_redecision(
                &self.pool,
                &self.event_bus,
                thread_id,
                change_id,
                actor.clone(),
            )
            .await?;
            self.redecide_branch_work(thread_id, &change.branch_name, repo_root, actor, finished)
                .await?;
            self.broadcast_changes_updated().await;
            return Ok(());
        }
        emit_change_status(
            &self.event_bus,
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
}

/// Withdraw an incomplete set-aside change so its branch can be decided again
/// (ADR 0400), and answer whether the thread's last turn finished. The archive
/// net flags every row it sets aside as incomplete, so the row itself cannot
/// say.
pub(crate) async fn withdraw_for_redecision(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    change_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    emit_change_status(
        bus,
        thread_id,
        crate::engine::thread_events::ThreadEvent::ChangeWithdrawn {
            change_id: change_id.to_string(),
        },
        actor,
    )
    .await?;
    Ok(
        crate::engine::agent_recovery::last_turn_end(pool, thread_id).await
            == Some(crate::engine::agent_recovery::TurnEnd::Finished),
    )
}

/// Emit a status event and wait for it to land. A failed emit is an error,
/// never a move the projection did not see.
async fn emit_change_status(
    bus: &EventBus,
    thread_id: Uuid,
    event: crate::engine::thread_events::ThreadEvent,
    actor: Option<MessageOrigin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let name = event.event_type();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event,
        meta: crate::engine::thread_events::EventMeta::with_actor(actor),
    })
    .await
    .map_err(|e| format!("could not record {name}: {e}"))?;
    Ok(())
}
