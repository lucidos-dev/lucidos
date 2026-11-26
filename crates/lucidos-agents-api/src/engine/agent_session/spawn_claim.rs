use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::claude_code::control::ControlExt;
use uuid::Uuid;

use crate::engine::session_claim::SpawnClaim;

#[easy_ext::ext(SpawnClaimExt)]
pub(crate) impl crate::engine::LucidosEngine {
    /// INVARIANT: Settle a `running` projection that no run holds. The settle holds the
    /// claim, so no spawn can start between its check and its terminal. A run
    /// that holds the thread owns the turn, and nothing is settled.
    async fn settle_unclaimed_running_thread(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
        terminal: crate::engine::claude_code::SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Some(_claim) = self.claim_thread_for_spawn(thread_id, None).await? else {
            return Ok(false);
        };
        // INVARIANT: A user's Stop between successor sessions ends real pending work.
        // Hold the claim across this read and its terminal, excluding a spawn.
        let terminal = if terminal == crate::engine::claude_code::SettleTerminal::UserStop
            && !crate::engine::successor::between_successor_sessions(&self.pool, thread_id).await?
        {
            crate::engine::claude_code::SettleTerminal::StuckProjection
        } else {
            terminal
        };
        crate::engine::claude_code::settle_stuck_running_thread(
            &self.pool,
            &self.event_bus,
            thread_id,
            actor,
            terminal,
        )
        .await
    }

    async fn settle_canceled_startup(
        &self,
        claim: &SpawnClaim,
        request_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
    ) -> crate::engine::types::ProcessResult {
        let thread_id = claim.thread_id;
        log_legacy!(
            "[AgentSession] A Stop cancelled the spawn for thread {} before its agent took a turn",
            thread_id
        );
        let shutting_down = claim
            .entry
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire);
        if !self.session_is_shutting_down(shutting_down) {
            let mut meta = meta.clone();
            if let Some(actor) = self.take_session_cancel_actor(thread_id).await {
                meta.actor = Some(actor);
            }
            crate::engine::thread_events::emit_response_canceled(
                &self.event_bus,
                &self.pool,
                thread_id,
                crate::engine::thread_events::CancelCause::UserStop,
                String::new(),
                vec![],
                None,
                None,
                meta,
                "[AgentSession] ResponseCanceled (stopped during startup)",
            )
            .await;
        }
        crate::engine::types::ProcessResult {
            response: String::new(),
            steps: vec![],
            images: vec![],
            request_id,
            thread_id,
            proposed_change: false,
            auto_apply: false,
            orphaned_injections: vec![],
        }
    }
}
#[cfg(test)]
#[path = "spawn_claim_tests.rs"]
mod tests;
