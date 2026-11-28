use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::ResolvedModelSelection;
use crate::engine::session_claim::SpawnClaim;
use crate::engine::session_copy::{SessionCopy, SideQuestionFailure};
use crate::engine::thread_events::EngineReason;
use crate::engine::types::SpawnAgentThreadParams;
use crate::engine::{ApplyClaim, Implementer, LiveSessionInfo};
use crate::runtime::CodingAgent;
use crate::triggers::continue_key::FireTarget;
use tokio_util::sync::CancellationToken;

use super::{
    resume_anchor::{ChatResumeAnchor, ChatResumeFuture, ContinueChatOutcome},
    thread_events::{ActorMode, EventChannel, MessageOrigin, RetryOf, TriggerInvocation, WakeMode},
    types::{AppContext, ChatImage, InputAnswered, ProcessResult, StopReason, UrlContext},
    FollowUpUrgency, InjectedPrompt, InputTurn, LucidosEngine, PreEmittedOrigin,
};

/// INVARIANT: Which terminal event to emit when settling a stuck thread. The caller
/// knows what ended the turn. The shapes look identical in the projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleTerminal {
    /// INVARIANT: Nothing this request did explains the `running` row: it was already
    /// wedged when the button was pressed. A spawn that errored before
    /// `SessionStarted`, a client that raced a terminal broadcast, the startup
    /// sweep, the zombie watchdog. Engine cleanup, so `ResponseAborted`.
    StuckProjection,
    CanceledQuestion,
    UserStop,
}

#[async_trait::async_trait]
pub trait AgentHost: Send + Sync {
    async fn answer_coding_agent_command(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        request_id: Uuid,
        command: crate::engine::thread_events::CodingAgentCommand,
        agent: crate::runtime::CodingAgent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn spawn_held_message_release(&self, engine: &Arc<LucidosEngine>, thread_id: Uuid);

    fn spawn_successor_start(&self, engine: &Arc<LucidosEngine>, thread_id: Uuid);

    fn spawn_chat_resume(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        engine_note: String,
        channel: EventChannel,
        actor: Option<MessageOrigin>,
        anchor: ChatResumeAnchor,
    ) -> ChatResumeFuture;

    /// INVARIANT: Resume a chat or trigger thread: after an abort, or once the card its
    /// run parked on is answered. `grants` hold for that one trigger run.
    async fn continue_chat(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        grants: crate::scheduler::user_tasks::TriggerRunGrants,
    ) -> Result<ContinueChatOutcome, Box<dyn std::error::Error + Send + Sync>>;

    fn spawn_remote_start_answer(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        choice: Option<crate::engine::permission::PermissionChoice>,
        actor: Option<MessageOrigin>,
    );

    fn spawn_hardening(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    );

    fn spawn_merge_session(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        claim: ApplyClaim,
        description: &str,
    );

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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn stop_agent(
        &self,
        engine: &Arc<LucidosEngine>,
        reason: StopReason,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn interrupt_agent(
        &self,
        engine: &LucidosEngine,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;

    async fn stop_turn_for_hold(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> bool;

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
        hook_args: Option<&crate::core::event_hooks::HookArgs>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>>;

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
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>>;

    async fn process_orphan_chain(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        initial_orphans: Vec<InjectedPrompt>,
    );

    async fn abandon_background_tasks(&self, engine: &LucidosEngine, thread_id: Uuid, why: &str);

    async fn re_estimate_expected_wake(&self, engine: &LucidosEngine, thread_id: Uuid);

    async fn auto_apply_proposed_change(
        &self,
        engine: &Arc<LucidosEngine>,
        request_id: Uuid,
        actor: Option<MessageOrigin>,
    );

    fn spawn_apply_after_session_harden(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        change_id: Uuid,
        answered: oneshot::Receiver<InputAnswered>,
        actor: Option<MessageOrigin>,
    );

    async fn redecide_branch_work(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        origin: Option<MessageOrigin>,
        finished: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

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
        hook_args: crate::core::event_hooks::HookArgs,
    ) -> Result<(Uuid, tokio::task::JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>>;

    async fn run_agent_thread_spawn(
        &self,
        engine: &Arc<LucidosEngine>,
        params: SpawnAgentThreadParams,
        thread_id: Uuid,
    );

    fn spawn_engine_coding_turn_or_else(
        &self,
        engine: &Arc<LucidosEngine>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        reason: EngineReason,
        on_failure: Box<dyn FnOnce(&str) + Send + 'static>,
    );

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
    );

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
    ) -> Result<(Vec<String>, crate::engine::BuildRequest), Box<dyn std::error::Error + Send + Sync>>;

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
    );

    async fn session_copy(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
    ) -> Result<SessionCopy, SideQuestionFailure>;

    async fn settle_unclaimed_running_thread(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;

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
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>>;

    async fn continue_input_for_reason(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        reason: Option<&str>,
    ) -> String;

    async fn continue_retry_input(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        reason: Option<&str>,
    ) -> String;

    async fn resume_failed_input(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        message: &str,
    ) -> String;

    async fn installed_plugin_summaries(
        &self,
        engine: &LucidosEngine,
        pool: &sqlx::PgPool,
    ) -> Result<
        Vec<crate::core::plugin_marketplaces::InstalledPluginSummary>,
        Box<dyn std::error::Error + Send + Sync>,
    >;
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct NoAgentHost {
    pub held_message_release_thread: Option<Uuid>,
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl AgentHost for NoAgentHost {
    async fn answer_coding_agent_command(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: Uuid,
        _: crate::engine::thread_events::CodingAgentCommand,
        _: crate::runtime::CodingAgent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("coding agent commands are unavailable".into())
    }

    fn spawn_hardening(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: PathBuf,
        _: String,
        _: Option<Uuid>,
        _: Option<MessageOrigin>,
    ) {
        panic!("move this test to the agent crate");
    }

    fn spawn_merge_session(&self, _: &LucidosEngine, _: Uuid, _: ApplyClaim, _: &str) {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_merge_session_tier2(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: &ApplyClaim,
        _: SpawnClaim,
        _: &Path,
        _: &str,
        _: &str,
        _: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    fn spawn_successor_start(&self, _: &Arc<LucidosEngine>, _: Uuid) {}

    fn spawn_held_message_release(&self, _: &Arc<LucidosEngine>, thread_id: Uuid) {
        assert_eq!(
            self.held_message_release_thread,
            Some(thread_id),
            "unexpected held-message release in lower-layer test",
        );
    }

    fn spawn_chat_resume(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: String,
        _: EventChannel,
        _: Option<MessageOrigin>,
        _: ChatResumeAnchor,
    ) -> ChatResumeFuture {
        panic!("move this test to the agent crate");
    }

    async fn continue_chat(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: Option<MessageOrigin>,
        _: crate::scheduler::user_tasks::TriggerRunGrants,
    ) -> Result<ContinueChatOutcome, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    fn spawn_remote_start_answer(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: Option<crate::engine::permission::PermissionChoice>,
        _: Option<MessageOrigin>,
    ) {
        panic!("move this test to the agent crate");
    }

    async fn stop_agent(
        &self,
        _: &Arc<LucidosEngine>,
        _: StopReason,
        _: Option<Uuid>,
        _: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    async fn interrupt_agent(
        &self,
        _: &LucidosEngine,
        _: Option<Uuid>,
        _: Option<MessageOrigin>,
        _: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    async fn stop_turn_for_hold(
        &self,
        engine: &LucidosEngine,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> bool {
        engine.cancel_thread_for_hold(thread_id, actor)
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps(
        &self,
        _: &LucidosEngine,
        _: &str,
        _: Option<&str>,
        _: Option<AppContext>,
        _: Option<String>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&[ChatImage]>,
        _: Option<&str>,
        _: Option<bool>,
        _: Option<&str>,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: Option<&str>,
        _: Option<UrlContext>,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: Option<WakeMode>,
        _: ActorMode,
        _: Option<&str>,
        _: Option<CodingAgent>,
        _: Option<PreEmittedOrigin>,
        _: Option<&str>,
        _: Option<MessageOrigin>,
        _: Option<Uuid>,
        _: FollowUpUrgency,
        _: Option<InputTurn>,
        _: Option<&crate::core::event_hooks::HookArgs>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_trigger(
        &self,
        _: &LucidosEngine,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: TriggerInvocation,
        _: bool,
        _: Vec<String>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: Uuid,
        _: Option<RetryOf>,
        _: FireTarget,
        _: Option<String>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    async fn process_orphan_chain(&self, _: &Arc<LucidosEngine>, _: Uuid, _: Vec<InjectedPrompt>) {
        panic!("move this test to the agent crate");
    }

    async fn abandon_background_tasks(&self, _: &LucidosEngine, _: Uuid, _: &str) {
        panic!("move this test to the agent crate");
    }

    async fn re_estimate_expected_wake(&self, _: &LucidosEngine, _: Uuid) {
        panic!("move this test to the agent crate");
    }

    async fn auto_apply_proposed_change(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: Option<MessageOrigin>,
    ) {
        panic!("move this test to the agent crate");
    }

    fn spawn_apply_after_session_harden(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: Uuid,
        _: oneshot::Receiver<InputAnswered>,
        _: Option<MessageOrigin>,
    ) {
        panic!("move this test to the agent crate");
    }

    async fn redecide_branch_work(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: &str,
        _: &Path,
        _: Option<MessageOrigin>,
        _: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_thread(
        &self,
        _: &LucidosEngine,
        _: &str,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: Option<WakeMode>,
        _: Uuid,
        _: Option<Uuid>,
        _: Option<&str>,
        _: ResolvedModelSelection,
        _: Option<MessageOrigin>,
        _: crate::core::event_hooks::HookArgs,
    ) -> Result<(Uuid, tokio::task::JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    async fn run_agent_thread_spawn(
        &self,
        _: &Arc<LucidosEngine>,
        _: SpawnAgentThreadParams,
        _: Uuid,
    ) {
        panic!("move this test to the agent crate");
    }

    fn spawn_engine_coding_turn_or_else(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: String,
        _: Option<String>,
        _: EngineReason,
        _: Box<dyn FnOnce(&str) + Send + 'static>,
    ) {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_engine_coding_turn_as(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: String,
        _: Option<String>,
        _: MessageOrigin,
        _: Implementer,
        _: Box<dyn FnOnce(&str) + Send + 'static>,
    ) {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_now_success(
        &self,
        _: &Arc<LucidosEngine>,
        _: &ApplyClaim,
        _: Uuid,
        _: bool,
        _: bool,
        _: &str,
        _: &str,
        _: &Path,
        _: &Path,
        _: &str,
        _: Option<MessageOrigin>,
    ) -> Result<(Vec<String>, crate::engine::BuildRequest), Box<dyn std::error::Error + Send + Sync>>
    {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_in_place_conflict_recovery(
        &self,
        _: &Arc<LucidosEngine>,
        _: Uuid,
        _: ApplyClaim,
        _: LiveSessionInfo,
        _: String,
        _: PathBuf,
        _: bool,
        _: bool,
        _: Option<MessageOrigin>,
    ) {
        panic!("move this test to the agent crate");
    }

    async fn session_copy(
        &self,
        _: &LucidosEngine,
        _: Uuid,
    ) -> Result<SessionCopy, SideQuestionFailure> {
        panic!("move this test to the agent crate");
    }

    async fn settle_unclaimed_running_thread(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: Option<MessageOrigin>,
        _: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_direct_agent(
        &self,
        _: &LucidosEngine,
        _: &mut SpawnClaim,
        _: Uuid,
        _: &str,
        _: Option<&[ChatImage]>,
        _: Uuid,
        _: &[Uuid],
        _: Option<Uuid>,
        _: &CancellationToken,
        _: Option<&ApplyClaim>,
        _: Option<(PathBuf, String)>,
        _: Option<String>,
        _: Option<String>,
        _: Option<String>,
        _: Option<String>,
        _: Option<String>,
        _: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        panic!("move this test to the agent crate");
    }

    async fn continue_input_for_reason(
        &self,
        _: &LucidosEngine,
        _: Uuid,
        _: Option<&str>,
    ) -> String {
        panic!("move this test to the agent crate");
    }

    async fn continue_retry_input(&self, _: &LucidosEngine, _: Uuid, _: Option<&str>) -> String {
        panic!("move this test to the agent crate");
    }

    async fn resume_failed_input(&self, _: &LucidosEngine, _: Uuid, _: &str) -> String {
        panic!("move this test to the agent crate");
    }

    async fn installed_plugin_summaries(
        &self,
        _: &LucidosEngine,
        _: &sqlx::PgPool,
    ) -> Result<
        Vec<crate::core::plugin_marketplaces::InstalledPluginSummary>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        panic!("move this test to the agent crate");
    }
}
