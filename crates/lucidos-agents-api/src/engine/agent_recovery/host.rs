//! Adapter from lower engine services to the agent implementation.

use crate::engine::agent_recovery::recovery::AgentRecoveryExt;
use crate::engine::agent_session::apply_now::ApplyNowExt;
use crate::engine::agent_session::background_task::BackgroundTaskExt;
use crate::engine::agent_session::run_session::run::RunAgentSessionExt;
use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::agent_session::side_question::AgentSessionSideQuestionExt;
use crate::engine::agent_session::spawn_claim::SpawnClaimExt;
use crate::engine::chat::held_messages::HeldMessageDeliveryExt;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::chat::rerun::RerunExt;
use crate::engine::chat::spawn::ChatSpawnExt;
use crate::engine::claude_code::control::ControlExt;
use crate::engine::claude_code::merge_session::MergeSessionExt;
use crate::engine::claude_code::spawn::ClaudeCodeSpawnExt;
use crate::engine::turn_end::expected_wake_decision::ExpectedWakeDecisionExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::ResolvedModelSelection;
use crate::engine::session_claim::SpawnClaim;
use crate::engine::session_copy::{SessionCopy, SideQuestionFailure};
use crate::engine::thread_events::EngineReason;
use crate::engine::types::SpawnAgentThreadParams;
use crate::engine::{
    agent_host::{AgentHost, SettleTerminal},
    resume_anchor::{ChatResumeAnchor, ChatResumeFuture, ContinueChatOutcome},
    thread_events::{ActorMode, EventChannel, MessageOrigin, RetryOf, TriggerInvocation, WakeMode},
    types::{AppContext, ChatImage, InputAnswered, ProcessResult, StopReason, UrlContext},
    FollowUpUrgency, InjectedPrompt, InputTurn, LucidosEngine, PreEmittedOrigin,
};
use crate::engine::{ApplyClaim, Implementer, LiveSessionInfo};
use crate::runtime::CodingAgent;
use crate::triggers::continue_key::FireTarget;
use tokio_util::sync::CancellationToken;

pub struct EngineAgentHost;

#[async_trait::async_trait]
impl AgentHost for EngineAgentHost {
    fn spawn_hardening(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        engine.spawn_hardening_session(
            thread_id,
            worktree_path,
            branch_name,
            auto_apply_change_id,
            actor,
        );
    }

    fn spawn_merge_session(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        claim: ApplyClaim,
        description: &str,
    ) {
        engine.spawn_merge_session(thread_id, claim, description);
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_merge_session_tier2(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        claim: &ApplyClaim,
        thread_hold: SpawnClaim,
        worktree_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        engine
            .run_merge_session_tier2(
                thread_id,
                claim,
                thread_hold,
                worktree_path,
                branch_name,
                description,
                resume_token,
            )
            .await
    }

    fn spawn_held_message_release(&self, engine: &Arc<LucidosEngine>, thread_id: Uuid) {
        engine.spawn_held_message_release(thread_id);
    }

    fn spawn_chat_resume(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        engine_note: String,
        channel: EventChannel,
        actor: Option<MessageOrigin>,
        anchor: ChatResumeAnchor,
    ) -> ChatResumeFuture {
        engine.spawn_chat_resume(thread_id, engine_note, channel, actor, anchor)
    }

    fn spawn_remote_start_answer(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        choice: Option<crate::engine::permission::PermissionChoice>,
        actor: Option<MessageOrigin>,
    ) {
        use crate::engine::agent_session::spawn_gate::remote_start::RemoteStartGateExt;
        engine.spawn_remote_start_answer(thread_id, choice, actor);
    }

    async fn continue_chat(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        grants: crate::scheduler::user_tasks::TriggerRunGrants,
    ) -> Result<ContinueChatOutcome, Box<dyn std::error::Error + Send + Sync>> {
        engine.continue_chat(thread_id, actor, grants).await
    }

    async fn stop_agent(
        &self,
        engine: &Arc<LucidosEngine>,
        reason: StopReason,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        engine.stop_agent(reason, thread_id, actor).await
    }

    async fn interrupt_agent(
        &self,
        engine: &LucidosEngine,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        engine.interrupt_agent(thread_id, actor, terminal).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps(
        &self,
        engine: &LucidosEngine,
        user_message: &str,
        model_override: Option<&str>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
        provider_override: Option<&str>,
        images: Option<&[ChatImage]>,
        device_id: Option<&str>,
        use_coding_agent: Option<bool>,
        event_id: Option<&str>,
        thread_id: Option<Uuid>,
        conflict_change_id: Option<Uuid>,
        repo_id: Option<&str>,
        url_context: Option<UrlContext>,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        wake_mode: Option<WakeMode>,
        mode: ActorMode,
        cc_model: Option<&str>,
        coding_agent: Option<CodingAgent>,
        pre_emitted_origin: Option<PreEmittedOrigin>,
        title: Option<&str>,
        origin: Option<MessageOrigin>,
        voice_session_id: Option<Uuid>,
        urgency: FollowUpUrgency,
        input_turn: Option<InputTurn>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        engine
            .process_message_with_steps(
                user_message,
                model_override,
                app_context,
                file_context,
                reasoning_effort,
                provider_override,
                images,
                device_id,
                use_coding_agent,
                event_id,
                thread_id,
                conflict_change_id,
                repo_id,
                url_context,
                parent_thread_id,
                spawning_event_id,
                wake_mode,
                mode,
                cc_model,
                coding_agent,
                pre_emitted_origin,
                title,
                origin,
                voice_session_id,
                urgency,
                input_turn,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_trigger(
        &self,
        engine: &LucidosEngine,
        trigger_id: &str,
        trigger_name: &str,
        slug: &str,
        prompt: &str,
        invocation: TriggerInvocation,
        go_to_review: bool,
        mcp_servers: Vec<String>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
        provider: Option<&str>,
        queue_entry_id: Uuid,
        retry_of: Option<RetryOf>,
        target: FireTarget,
        actor_email: Option<String>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        engine
            .process_trigger(
                trigger_id,
                trigger_name,
                slug,
                prompt,
                invocation,
                go_to_review,
                mcp_servers,
                model,
                reasoning_effort,
                provider,
                queue_entry_id,
                retry_of,
                target,
                actor_email,
            )
            .await
    }

    async fn process_orphan_chain(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        initial_orphans: Vec<InjectedPrompt>,
    ) {
        crate::api::chat::process_orphan_chain(engine.clone(), thread_id, initial_orphans).await
    }

    async fn abandon_background_tasks(&self, engine: &LucidosEngine, thread_id: Uuid, why: &str) {
        engine.abandon_background_tasks(thread_id, why).await
    }

    async fn re_estimate_expected_wake(&self, engine: &LucidosEngine, thread_id: Uuid) {
        engine.re_estimate_expected_wake(thread_id).await
    }

    async fn auto_apply_proposed_change(
        &self,
        engine: &Arc<LucidosEngine>,
        request_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        engine.auto_apply_proposed_change(request_id, actor).await
    }

    fn spawn_apply_after_session_harden(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        change_id: Uuid,
        answered: oneshot::Receiver<InputAnswered>,
        actor: Option<MessageOrigin>,
    ) {
        engine.spawn_apply_after_session_harden(thread_id, change_id, answered, actor)
    }

    async fn redecide_branch_work(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        origin: Option<MessageOrigin>,
        finished: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        engine
            .redecide_branch_work(thread_id, branch_name, repo_root, origin, finished)
            .await
            .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_thread(
        &self,
        engine: &LucidosEngine,
        prompt: &str,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        wake_mode: Option<WakeMode>,
        child_thread_id: Uuid,
        pre_emitted_origin: Option<Uuid>,
        caller_title: Option<&str>,
        route: ResolvedModelSelection,
        origin: Option<MessageOrigin>,
    ) -> Result<(Uuid, tokio::task::JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>> {
        engine.spawn_thread(
            prompt,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            child_thread_id,
            pre_emitted_origin,
            caller_title,
            route,
            origin,
        )
    }

    async fn run_agent_thread_spawn(
        &self,
        engine: &Arc<LucidosEngine>,
        params: SpawnAgentThreadParams,
        thread_id: Uuid,
    ) {
        engine
            .clone()
            .run_agent_thread_spawn(params, thread_id)
            .await
    }

    fn spawn_engine_coding_turn_or_else(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        reason: EngineReason,
        on_failure: Box<dyn FnOnce(&str) + Send + 'static>,
    ) {
        engine.spawn_engine_coding_turn_or_else(thread_id, prompt, title, reason, on_failure)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_engine_coding_turn_as(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        origin: MessageOrigin,
        implementer: Implementer,
        on_failure: Box<dyn FnOnce(&str) + Send + 'static>,
    ) {
        engine.spawn_engine_coding_turn_as(
            thread_id,
            prompt,
            title,
            origin,
            implementer,
            on_failure,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_now_success(
        &self,
        engine: &Arc<LucidosEngine>,
        claim: &ApplyClaim,
        thread_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        pre_sha: &str,
        post_sha: &str,
        worktree_path: &Path,
        repo_root: &Path,
        branch_name: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(Vec<String>, crate::engine::BuildRequest), Box<dyn std::error::Error + Send + Sync>>
    {
        engine
            .apply_now_success(
                claim,
                thread_id,
                requires_restart,
                client_update,
                pre_sha,
                post_sha,
                worktree_path,
                repo_root,
                branch_name,
                actor,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_in_place_conflict_recovery(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        claim: ApplyClaim,
        session: LiveSessionInfo,
        branch_name: String,
        repo_root: PathBuf,
        requires_restart: bool,
        client_update: bool,
        actor: Option<MessageOrigin>,
    ) {
        engine.spawn_in_place_conflict_recovery(
            thread_id,
            claim,
            session,
            branch_name,
            repo_root,
            requires_restart,
            client_update,
            actor,
        )
    }

    async fn session_copy(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
    ) -> Result<SessionCopy, SideQuestionFailure> {
        engine.session_copy(thread_id).await
    }

    async fn settle_unclaimed_running_thread(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        engine
            .settle_unclaimed_running_thread(thread_id, actor, terminal)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_direct_agent(
        &self,
        engine: &LucidosEngine,
        claim: &mut SpawnClaim,
        request_id: Uuid,
        user_message: &str,
        user_images: Option<&[ChatImage]>,
        origin_id: Uuid,
        carried_event_ids: &[Uuid],
        spawning_event_id: Option<Uuid>,
        cancel_token: &CancellationToken,
        conflict: Option<&ApplyClaim>,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        resume_worktree_path: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        engine
            .run_direct_agent(
                claim,
                request_id,
                user_message,
                user_images,
                origin_id,
                carried_event_ids,
                spawning_event_id,
                cancel_token,
                conflict,
                recovery_worktree,
                repo_id,
                system_prompt_override,
                resume_session_id,
                cc_model,
                cc_reasoning_effort,
                resume_worktree_path,
            )
            .await
    }

    async fn continue_input_for_reason(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        reason: Option<&str>,
    ) -> String {
        crate::engine::agent_recovery::continue_input_for_reason(
            engine.pool(),
            engine.workspace_path(),
            thread_id,
            reason,
        )
        .await
    }

    async fn continue_retry_input(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        reason: Option<&str>,
    ) -> String {
        crate::engine::agent_recovery::continue_retry_input(
            engine.pool(),
            engine.workspace_path(),
            thread_id,
            reason,
        )
        .await
    }

    async fn resume_failed_input(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        message: &str,
    ) -> String {
        crate::engine::successor::resume_failed_input(engine.pool(), thread_id, message).await
    }

    async fn installed_plugin_summaries(
        &self,
        engine: &LucidosEngine,
        pool: &sqlx::PgPool,
    ) -> Result<
        Vec<crate::core::plugin_marketplaces::InstalledPluginSummary>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        crate::engine::tools::plugins::installed_plugin_summaries(pool, engine.workspace_path())
            .await
    }
}
