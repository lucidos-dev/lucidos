use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::webhook_ingress::{AddressProbe, Family, FamilyVerdict};
use crate::core::webhook_refusal::{RefusalCause, Resolution};
use crate::core::{AppKind, AuthType};
use crate::engine::provider_events::{ChatGptPlanSignOutReason, ClaudeGatewaySignOutReason};
use crate::engine::thread_events::notification::Tap;
use crate::engine::thread_events::WidgetParams;
use crate::engine::thread_events::{ArrangedTile, Dashboard, MessageOrigin};

const CHATGPT_PLAN_AGGREGATE_ID: &str = "chatgpt-plan";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ArtifactWriter {
    Person,
    Thread,
    App { app_id: String },
    Engine,
    Trigger { trigger_id: String },
}

impl ArtifactWriter {
    pub fn is_workspace_leaf(&self) -> bool {
        !matches!(self, Self::Trigger { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubThreadCardKind {
    Question,
    PermissionRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SystemEvent {
    InternalFaultRecorded {
        kind: crate::internal_faults::FaultKind,
    },
    /// INVARIANT: The owner-approved diagnostic bundle was delivered. This is the durable
    /// boundary for the internal-fault count; failed sends never reset it.
    DiagnosticBundleSent {
        destination: String,
        report_code: Option<String>,
        size_bytes: u64,
        sources: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DiagnosticBundleReceived {
        bundle_id: Uuid,
        origin: String,
        digest: String,
        size_bytes: u64,
    },
    NotificationCreated {
        id: String,
        title: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(default)]
        tap: Tap,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audience: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        push_group: Option<crate::engine::thread_events::notification::PushGroup>,
    },
    NotificationRead {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    NotificationsAllRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PreferencesChanged {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The user answered a *release notice*, by acting on it or by reading it.
    ///
    /// The cursor behind it is a silent preference, so this is the only durable
    /// trace of the answer. It also closes the modal on the user's other
    /// devices. That is why the resolve announces, rather than leaning on the
    /// client that made it.
    ReleaseNoticeResolved {
        notice_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    MemoryRebuildProgress {
        processed: usize,
        total: usize,
        percent: usize,
    },
    EmbeddingModelStatusChanged {
        model_id: String,
        load_state: crate::engine::thread_events::EmbeddingModelLoadState,
    },
    TreeBackfillProgressed {
        progress: crate::engine::thread_events::tree_backfill::BackfillProgress,
    },
    TreeBackfillCompleted {
        total: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spent_usd: Option<f64>,
    },
    TreeBackfillReset {},
    TreeBackfillStarted {
        estimate: crate::engine::thread_events::tree_backfill::StartEstimate,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cap_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TreeBackfillCapReached {
        spent_usd: f64,
        cap_usd: f64,
    },
    TreeBackfillCapRaised {
        cap_usd: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TreeBackfillEnded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spent_usd: Option<f64>,
    },
    /// INVARIANT: Broadcast only, so it is never read back from a row. That is why its
    /// change lists skip deserializing.
    ChangesUpdated {
        #[serde(skip_deserializing)]
        pending: Vec<crate::core::changes::Change>,
        #[serde(skip_deserializing)]
        set_aside: Vec<crate::core::changes::Change>,
        #[serde(skip_deserializing)]
        applied: Vec<crate::core::changes::Change>,
        total_pending: usize,
        restart_required: bool,
        apply_estimates: crate::engine::thread_events::apply::ApplyEstimates,
        #[serde(default)]
        landing_lane: crate::engine::thread_events::landing_lane::LaneSnapshot,
        #[serde(default)]
        main_check: crate::engine::thread_events::main_check::MainCheckSnapshot,
    },
    SnapshotProgress {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        job_id: uuid::Uuid,
        action: crate::core::snapshot::jobs::Action,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Target-authorized restore, persisted before the gateway stops the engine.
    SnapshotRestoreRequested {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        restore_id: uuid::Uuid,
        actor: MessageOrigin,
    },
    /// INVARIANT: Verified restored log after no-replay settlement. Completion notification
    /// and external journal completion must follow before dispatch is admitted.
    WorkspaceRestored {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        recovery: crate::core::snapshot::handle::Handle,
        restore_id: uuid::Uuid,
        retained_worktrees: Vec<String>,
        actor: MessageOrigin,
    },
    /// INVARIANT: A verified local capture was published. Persisted; no paths or key material.
    SnapshotCreated {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        job_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    SnapshotCaptureFailed {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        job_id: uuid::Uuid,
        reason: crate::core::snapshot::jobs::Failure,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    SnapshotChecked {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        job_id: uuid::Uuid,
        passed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    SnapshotKeepChanged {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        keep: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    SnapshotDeleted {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
        reason: crate::core::snapshot::events::DeletionReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    BackupProgress {
        phase: String,
        progress: usize,
        total: usize,
    },
    /// INVARIANT: A backup run finished successfully. PERSISTED (unlike `BackupProgress`):
    /// the events table is the durable history of every backup run (start /
    /// finish / size), queried by `core::backup::load_recent_runs` and surfaced
    /// by the `get_backup_status` tool.
    BackupCompleted {
        filename: String,
        size_bytes: u64,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
    },
    /// INVARIANT: A backup run failed. PERSISTED — see `BackupCompleted`.
    BackupFailed {
        error: String,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
    },
    BackupKeyRevealed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    BackupRecipientSet {
        recipient: String,
        own_key: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ProxyConfigRejected {
        rejected: Vec<crate::engine::provider_rejection::RejectedProvider>,
    },
    ModelSpendCapReached {
        provider: String,
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
        held_until: chrono::DateTime<chrono::Utc>,
    },
    ModelSpendCapHoldEnded {
        provider: String,
        cause: crate::llm::spend_cap::SpendCapHoldEndCause,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ModelNotServedObserved {
        model: String,
        provider: String,
        purpose: crate::engine::thread_events::ContextPurpose,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        moved_to: Option<String>,
    },
    ThreadsDeleted {
        thread_ids: Vec<Uuid>,
        event_count: i64,
        memory_count: i64,
        worktrees_removed: usize,
        #[serde(default)]
        widgets_removed: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    RecoveryProgress {
        completed: usize,
        total: usize,
    },
    MemoryExtractionFailed {
        source: crate::engine::thread_events::MemorySource,
        model: String,
        reason: String,
    },
    /// INVARIANT: The user corrected a long-term memory. The only durable record of the
    /// correction: every memory rebuild replays these, so a wrong fact the
    /// extractor finds again in its source events is removed again.
    MemoryCorrected {
        search_query: String,
        wrong_fact: String,
        removed: Vec<crate::engine::thread_events::CorrectedMemory>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        correction: Option<String>,
        recorded_at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    Toast {
        message: String,
        level: String,
    },
    ArtifactImported {
        artifact_path: String,
        source_type: String,
        source_detail: String,
        commit_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer: Option<ArtifactWriter>,
    },
    TriggerCreated {
        trigger_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerUpdated {
        trigger_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerDeleted {
        trigger_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerEnabled {
        trigger_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerDisabled {
        trigger_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerExecuted {
        trigger_id: String,
        payload: serde_json::Value,
    },
    TriggerGroupCreated {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerGroupRenamed {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerGroupReordered {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A trigger group was deleted. Only emitted when the group has no members —
    /// the HTTP handler rejects the delete with 409 otherwise so the LLM (or user)
    /// can self-correct. Payload: `{ group_id }`.
    TriggerGroupDeleted {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EventHookArmed {
        slug: String,
        /// INVARIANT: The hooked event's wire name, as the armed definition names it.
        event_type: String,
        content_hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EventHookDisarmed {
        slug: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EventHookRemoved {
        slug: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EventHookRefused {
        slug: String,
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<uuid::Uuid>,
        cause: crate::core::event_hooks::RefusalCause,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Domain event emitted via emit_event (e.g. SlideTextEdited, SleepImported).
    /// Persisted through EventBus with the inner `event_type` as the stored event type.
    /// Broadcast so the trigger subscriber can fire matching event-based triggers.
    /// `depth` tracks event-trigger recursion (A fires trigger → emits event → fires trigger…).
    /// `transient: true` skips the events table write — used for high-churn
    /// coordination signals (heartbeats, presenter↔remote state) that should
    /// reach SSE consumers but don't belong in the audit log.
    DomainEvent {
        event_type: String,
        payload: serde_json::Value,
        #[serde(skip_serializing)]
        depth: u32,
        #[serde(skip_serializing)]
        transient: bool,
        /// INVARIANT: Who emitted the domain event, as the engine established it. Every
        /// live emit names one, through `LucidosEngine::emit_domain_event`.
        /// `None` is left to the backfill replay of a row the engine wrote.
        /// `to_payload` merges it into the wire payload and drops any `actor`
        /// the caller wrote there.
        #[serde(skip_serializing)]
        actor: Option<MessageOrigin>,
    },
    DashboardTileAdded {
        dashboard: Dashboard,
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<WidgetParams>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<crate::core::apps::WidgetSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DashboardTileRemoved {
        dashboard: Dashboard,
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<WidgetParams>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: One dashboard's tiles in order, each at its size. The newest wins,
    /// whole; a tile no longer on the dashboard is ignored.
    DashboardArranged {
        dashboard: Dashboard,
        tiles: Vec<ArrangedTile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: App list-refresh hints. `AppCreated` and `AppUpdated` are
    /// broadcast-only, like `AppUiRefreshRequested`: the `appsList` is a
    /// disk-scan projection and these SSE signals keep it live. The durable
    /// record of an app change is the git commit.
    AppCreated {
        app_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    AppUpdated {
        app_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    AppDeleted {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<AppKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    AppOpened {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    KnowhowRead {
        knowhow_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: An app's iframe-bundled files changed in a way the open iframes need
    /// to pick up. Emitted by the Apply path of an app coding-agent thread
    /// when any merged file under `data/apps/<id>/` is an HTML / CSS / JS /
    /// `manifest.json` / static asset (image, font, …). Transient — the SDK
    /// in any open iframe of `app_id` listens via SSE and reloads its src.
    /// Not persisted: the event is a UI signal, not an audit record (the
    /// matching `ChangeApplied` already carries the file list).
    AppUiRefreshRequested {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A *frontend-only* Apply's in-process served-client advance was DEFERRED
    /// because an engine version change is pending (a mixed change applied but
    /// not yet *Switched*) — see `engine::frontend_refresh` INV-A. The rebuilt
    /// `dist/` already holds a client built for the NEW engine, so it can't be
    /// served on the still-running old one; the change ships when the user
    /// Switches. This is the page-facing signal that lets the frontend tell the
    /// user their just-applied change is queued rather than ignored. Transient
    /// (never persisted — a pure UI hint, like `AppUiRefreshRequested`) and
    /// dev-only by construction (`refresh_served_frontend_after_rebuild`
    /// early-returns in packaged / headless before the emit path runs).
    /// `sent_at_ms` drives the page-side freshness gate (drop a late SSE-queue
    /// flush that arrives after the Switch already happened).
    FrontendUpdateDeferred {
        sent_at_ms: i64,
    },
    /// INVARIANT: A *frontend-only* Apply rebuilt fine but the engine's served client can
    /// never advance to it, because the `dist/` it serves is not the `dist/` the
    /// build-watch republishes. Distinct from `FrontendUpdateDeferred`, which
    /// means "queued, arrives on Switch" — here nothing is coming, so the two
    /// must not share a message.
    ///
    /// The known cause is a stack pinned to a coding-agent worktree
    /// (`served_in_worktree`), where the served `dist/` is frozen at the commit
    /// that worktree was cut from while the shared checkout rebuilds elsewhere —
    /// the 2026-07-26 incident, where every frontend Apply silently did nothing
    /// for hours. `served_in_worktree: false` means the same stranding from some
    /// other cause (no build-watch running, a `dist/` nobody rebuilds), which
    /// wants different advice, so the flag is carried rather than assumed.
    ///
    /// Transient (never persisted — a pure UI signal like
    /// `FrontendUpdateDeferred`) and dev-only by construction
    /// (`refresh_served_frontend_after_rebuild` early-returns when packaged).
    /// See `docs/plans/2026-07-26-worktree-pinned-stack-guard.md`.
    FrontendUpdateStranded {
        served_dir: String,
        served_in_worktree: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_error: Option<String>,
        /// INVARIANT: Whether the build-watch is positively known to have STOPPED.
        ///
        /// Read from the pidfile beside that status file, because the status
        /// alone cannot say: it records the last COMPLETED build, so a watch
        /// that died leaves a healthy one behind. True means nothing will
        /// republish until the stack is relaunched, which is the one case the
        /// message must not describe as recoverable. False also covers "could
        /// not tell", so this never over-claims.
        #[serde(default)]
        build_watch_stopped: bool,
        sent_at_ms: i64,
    },
    /// INVARIANT: A dev engine advanced its boot-pinned served-frontend snapshot to the
    /// checkout-shared `dist/`. Emitted on every swap, by the engine that applied
    /// a *frontend-only* change and by a peer workspace's periodic sync
    /// (`engine::frontend_refresh`,
    /// `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`).
    /// INV-A-gated: only emitted when no engine version change is pending, so the
    /// newer client is compatible with the running binary. Transient and dev-only
    /// by construction (both paths no-op packaged / headless). The connected
    /// client re-runs `syncClientUpdateFromBuild` to surface the Refresh badge and
    /// toast. `sent_at_ms` is informational: the handler is idempotent and
    /// self-correcting, so no page-side freshness gate is needed.
    ServedFrontendAdvanced {
        sent_at_ms: i64,
    },
    /// INVARIANT: Transient, dev-only notification carrying the gateway preview path.
    /// Clients resolve it against their own origin, including through trusted relays.
    FrontendPreviewStarted {
        thread_id: uuid::Uuid,
        path: String,
        sent_at_ms: i64,
    },
    FrontendPreviewStopped {
        thread_id: uuid::Uuid,
        sent_at_ms: i64,
    },
    EngineVersionActivated {
        build_id: String,
        engine_version: String,
        release: String,
        release_dirty: bool,
        frontend_source_commit: Option<String>,
    },
    ServedFrontendCommitChanged {
        previous_source_commit: Option<String>,
        source_commit: String,
    },
    EngineBuildCompleted {
        outcome: String,
        duration_ms: u64,
        built_head: Option<String>,
    },
    EngineBuildStateChanged {
        state: String,
        sent_at_ms: i64,
    },
    EngineVersionStatusChanged {
        sent_at_ms: i64,
    },
    FrontendRefreshStateChanged {
        sent_at_ms: i64,
    },
    FrontendBuildWatchStateChanged {
        sent_at_ms: i64,
    },
    ArtifactCreated {
        artifact_path: String,
        commit: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer: Option<ArtifactWriter>,
    },
    ArtifactUpdated {
        artifact_path: String,
        commit: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writer: Option<ArtifactWriter>,
    },
    ArtifactDeleted {
        artifact_path: String,
        commit: String,
    },
    LanguageSet {
        language: String,
    },
    TimezoneSet {
        timezone: String,
    },
    RepositoryImported {
        url: String,
        branch: String,
        destination: String,
        file_count: usize,
        skipped_count: usize,
        commit: String,
        files: Vec<String>,
    },
    TriggerCompleted {
        trigger_id: String,
        trigger_name: String,
        result_summary: String,
    },
    /// INVARIANT: Cron slots of one trigger passed with no run, past the catch-up grace.
    /// One per trigger per gap, with a matching inbox notice. PERSISTED, so a
    /// trigger can react to it, and its `last_slot` keeps the next boot from
    /// reporting the same gap. Engine-driven, so it carries no actor.
    TriggerSlotsSkipped {
        trigger_id: String,
        trigger_name: String,
        first_slot: chrono::DateTime<chrono::Utc>,
        last_slot: chrono::DateTime<chrono::Utc>,
        count: u32,
    },
    ChangeDiscarded {
        change_id: String,
    },
    DeviceVisible {
        device_id: String,
    },
    DeviceHidden {
        device_id: String,
    },
    /// INVARIANT: SSE-only, never persisted. The engine asks every connected page what
    /// it does with a new notification. See `system-knowhow/notifications.md`
    /// §3 (protocol, freshness gate) and §4 (the row matrix).
    ///
    /// It carries the toast content. A page puts the toast on screen, marks
    /// the row read, or holds it for the badge, and only THEN pongs which. The
    /// engine suppresses the OS push only on such a pong. So a page that hides
    /// after answering still leaves the reader the toast it answered for.
    /// `sent_at_ms` lets the page drop a check that arrives past its deadline.
    PresenceCheck {
        notification_id: Uuid,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default)]
        tap: Tap,
        deadline_ms: u32,
        sent_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audience: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    NativePushRequested {
        notification_id: Uuid,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default)]
        tap: Tap,
        sent_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audience: Option<Vec<String>>,
    },
    NativePushDismissRequested {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notification_id: Option<Uuid>,
        sent_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audience: Option<Vec<String>>,
    },
    UsersChanged,
    PairingRequestsChanged,
    /// INVARIANT: A plugin was installed (or updated — overwrite=true reuses this variant).
    /// `manifest` carries the full parsed manifest so future fields are additive.
    /// `files` are paths under `data/` so a future tracked-uninstall can derive
    /// ownership without a schema change.
    ///
    /// `id` is the plugin id, at the top level where every sibling `Plugin*`
    /// frame carries it. It is also inside `manifest.manifest.id`, two layers
    /// down, which is where `InstalledRecord` still reads it and where rows
    /// written before this field carry it alone. A subscription filtering on
    /// the nested path went dead once, unseen, which is what this field ends.
    ///
    /// `files_removed` lists paths the previous version recorded and this one
    /// dropped, which the update deleted. Empty for a fresh install.
    ///
    /// `affected_triggers` lists the triggers this plugin does not own that
    /// relied on a removed path. A fire of one reads it back to warn the agent.
    PluginInstalled {
        #[serde(default)]
        id: String,
        manifest: serde_json::Value,
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files_removed: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        affected_triggers: Vec<crate::engine::thread_events::affected_trigger::AffectedTrigger>,
        installed_at: String,
        source_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginUninstalled {
        id: String,
        version: String,
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files_deleted: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files_missing: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        affected_triggers: Vec<crate::engine::thread_events::affected_trigger::AffectedTrigger>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: An install met files the user had locally edited, and says what it did
    /// with each. `merged` kept their edit alongside upstream's. `conflicted`
    /// could not. `replaced` was never mergeable: a trigger projection, a
    /// binary, or the panel's keep control switched off. `restored` is a file
    /// the user had deleted that upstream still ships, so it came back.
    /// `removed` is an edited file the new version no longer ships.
    ///
    /// Everything in `merged` survives in the file. Everything in `conflicted`,
    /// `replaced` and `removed` has a copy in `saved_paths`. Nothing in
    /// `restored` does, because a deletion has no content to save.
    ///
    /// Emitted right after the `PluginInstalled` it belongs to, and only when
    /// the install met at least one edited file. It exists because the outcome
    /// is otherwise unrecoverable. The Modified badge is derived from git plus
    /// disk on each read. Once the commits are written, nothing on disk records
    /// which files merged and which lost.
    ///
    /// `saved_paths` are `data/`-relative copies of every discarded edit,
    /// written under `data/artifacts/` before the overwrite. `commit` is the
    /// commit holding the merged working tree plus those copies.
    PluginLocalChangesMerged {
        id: String,
        version: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        merged: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        conflicted: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        replaced: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        restored: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        removed: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        saved_paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginInstallCanceled {
        id: String,
        version: String,
        source: String,
        source_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginUninstallCanceled {
        id: String,
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginMarketplaceRegistered {
        marketplace_id: String,
        name: String,
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginMarketplaceRemoved {
        marketplace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PluginCatalogScanStarted {},
    PluginCatalogScanned {
        failed: bool,
    },
    PinnedAppPinned {
        app_id: String,
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    PinnedAppUnpinned {
        app_id: String,
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DeviceRegistered {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DeviceRenamed {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DevicePushChanged {
        device_id: String,
        push_enabled: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DeviceDeleted {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DeviceHandedOver {
        device_id: String,
        old_device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    RepositoryAdded {
        repo_id: String,
        name: String,
        root_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    RepositoryRemoved {
        repo_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A credential entry was created. **Never** carry the auth value here —
    /// the payload reaches every SSE subscriber on every connected device.
    /// The service identifier is the only public field.
    CredentialCreated {
        service_name: String,
        auth_type: AuthType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A credential's auth value was updated. Same secrecy contract as
    /// `CredentialCreated` — only the service id is broadcast.
    CredentialUpdated {
        service_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    CredentialDeleted {
        service_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<crate::core::credentials::CredentialMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    CredentialRevealed {
        service_name: String,
        auth_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EnvironmentVariableSet {
        name: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    EnvironmentVariableDeleted {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ModelCreated {
        id: String,
        label: String,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ModelUpdated {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ModelDeleted {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A webhook was created. It opens a publicly reachable endpoint that emits
    /// a pinned domain event, so its birth belongs on the timeline. The payload
    /// carries the pinned `event_type` and whether a signature is configured,
    /// never the token and never a secret.
    WebhookCreated {
        webhook_id: String,
        name: String,
        event_type: String,
        signed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    WebhookUpdated {
        webhook_id: String,
        name: String,
        event_type: String,
        enabled: bool,
        signed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    WebhookDeleted {
        webhook_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: An address family stopped answering on the public webhook path.
    ///
    /// Emitted on the edge, after two consecutive cycles see the same failure.
    /// The engine reports and never repairs: a workspace trigger decides what
    /// to do, and `docs/adr/0143-webhook-ingress-probed-per-address-family.md`
    /// pins this payload for it. No actor, because nobody asked for it.
    WebhookIngressDegraded {
        webhook_id: String,
        webhook_name: String,
        host: String,
        port: u16,
        /// INVARIANT: Which families are down. A healthy sibling family does not save
        /// them: senders reach one family each, and a dead one is an outage.
        degraded_families: Vec<Family>,
        families: Vec<FamilyVerdict>,
        addresses: Vec<AddressProbe>,
    },
    WebhookIngressRecovered {
        webhook_id: String,
        webhook_name: String,
        host: String,
        port: u16,
        recovered_families: Vec<Family>,
        families: Vec<FamilyVerdict>,
        addresses: Vec<AddressProbe>,
        down_since: String,
        down_secs: i64,
    },
    /// An enabled webhook has no public route the monitor can probe.
    WebhookIngressUnverified {
        webhook_id: String,
        webhook_name: String,
    },
    /// A probe round ran, or the workspace stopped expecting webhook deliveries.
    WebhookIngressVerified {
        webhook_id: String,
        webhook_name: String,
    },
    /// INVARIANT: This computer slept while a webhook was on, so deliveries sent in that
    /// time never arrived.
    ///
    /// One per *sleep spell*, on the first clean ingress round after it, and
    /// only for a spell of 30 minutes or more. A night of short dark wakes is
    /// one spell. It blames no funnel and no family, because none failed
    /// (`docs/adr/0367-a-sleeping-host-is-not-a-dead-ingress.md`).
    WebhookDeliveriesSleptThrough {
        webhook_id: String,
        webhook_name: String,
        host: String,
        port: u16,
        /// INVARIANT: How long the computer slept, summed over the spell. Read off the
        /// gap between the wall clock and the monotonic clock.
        slept_secs: u64,
    },
    WebhookDeliveriesRefused {
        webhook_id: String,
        webhook_name: String,
        enabled: bool,
        cause: RefusalCause,
        refusals: i64,
        /// INVARIANT: The run's breakdown by reason, keyed by `DeliveryRefusal::key`.
        ///
        /// The whole evidence rather than the last line of it. A single
        /// diagnostic probe adds one to its own reason and erases nothing.
        reasons: std::collections::BTreeMap<String, i64>,
        /// INVARIANT: When the run started, RFC 3339.
        refusing_since: String,
        refusing_secs: i64,
    },
    /// INVARIANT: A webhook stopped turning away the deliveries it receives.
    ///
    /// `resolution` says which way out it took. Every way a hook can leave the
    /// refusing state has one, so a declaration cannot strand.
    WebhookDeliveriesRecovered {
        webhook_id: String,
        webhook_name: String,
        cause: RefusalCause,
        resolution: Resolution,
        refusing_since: String,
        refusing_secs: i64,
    },
    /// INVARIANT: An MCP server was registered, or an existing one re-registered with a
    /// new command / args / env. The `mcp_servers` table drives which external
    /// tools the agent can call, so a registration changes the agent's own tool
    /// surface: it belongs on the timeline even though no UI lists it yet.
    ///
    /// `created` is true when no server held the id before. Only such a
    /// registration can be undone: a re-registration's old env is sealed.
    McpServerRegistered {
        server_id: String,
        name: String,
        #[serde(default)]
        created: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    McpServerUpdated {
        server_id: String,
        auto_approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Which of a server's tools are switched off changed. `disabled_tools` is
    /// the resulting set in full, by WIRE name, not a delta: the user picks a
    /// selection, and one event that states it is easier to read back than a
    /// pair of added/removed lists.
    ///
    /// Its own variant rather than a field on `McpServerUpdated`, which is an
    /// auto-approve change. A disabled tool leaves every request, so this moves
    /// the agent's tool surface the way `McpServerRemoved` does for a whole
    /// server.
    ///
    /// `previous` is the set before, which Undo restores. `None` on older rows.
    McpServerDisabledToolsChanged {
        server_id: String,
        disabled_tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    McpServerRemoved {
        server_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    McpServerStopped {
        server_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Someone approved an MCP server's current launch line: its command,
    /// args, env and spawn directory. With the command guard on, a start of a
    /// line nobody approved is refused. Emitted by
    /// `McpServerStore::approve_launch`, only for a new approval. Carries no
    /// line, since plain env values are sealed at rest.
    McpLaunchApproved {
        server_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: One lane's standing tool grants changed: the *permission grants* the
    /// `permissions` table holds for the workspace, one row per pattern.
    ///
    /// `patterns` carries the full resulting set. Legacy imports only add
    /// rows for its missing patterns. Other changes also end omitted rows.
    ///
    /// A unique index stamps the import per lane. New rows keep the
    /// source and the thread where the grant was made.
    ///
    /// A subscriber can scope this event to one lane with a condition on
    /// `grant_file`.
    PermissionGrantsChanged {
        grant_file: crate::core::GrantFile,
        patterns: Vec<String>,
        #[serde(default)]
        source: crate::core::grants::GrantSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        granted_in_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    OAuthAccountConnected {
        account_id: String,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    OAuthAccountDeleted {
        account_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ClaudeGatewaySignedIn {
        issuer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ClaudeGatewaySignedOut {
        issuer: String,
        reason: ClaudeGatewaySignOutReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ClaudeGatewaySignInStarted {
        issuer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ClaudeGatewaySignInCanceled {
        issuer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ClaudeGatewaySignInFailed {
        issuer: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The *gateway managed settings* `issuer` sets for the signed-in user
    /// changed. `policy` is false when the gateway now has none for them.
    /// `telemetry` says whether the policy asks for usage telemetry, and
    /// `available_models` is its model allowlist. Carries no other setting,
    /// since one can hold a collector's credential.
    ClaudeGatewayManagedSettingsChanged {
        issuer: String,
        policy: bool,
        telemetry: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_models: Option<Vec<String>>,
    },
    ChatGptPlanSignedIn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ChatGptPlanSignedOut {
        reason: ChatGptPlanSignOutReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ChatGptPlanSignInStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ChatGptPlanSignInCanceled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ChatGptPlanSignInFailed {
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The ChatGPT plan answered that its usage limit for Lucidos is spent.
    /// Plan requests fail fast, with no request sent, until `paused_until`.
    /// Emitted once per pause. `paused_until` is Lucidos's own pause, never
    /// OpenAI's reset time.
    ChatGptPlanLimitReached {
        paused_until: chrono::DateTime<chrono::Utc>,
    },
    DataFileWritten {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DataFileDeleted {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: A `POST /api/v1/data/edit` ran one or more in-place operations
    /// (JSON-path or text find/replace). `operations_count` records how many
    /// op-blocks were applied; the per-op detail isn't broadcast because the
    /// values may carry user data we don't want to fan out to every SSE
    /// subscriber. The agent's `edit_file` emits it too, with no actor and a
    /// count of 1, for a workspace path outside `artifacts/`.
    DataFileEdited {
        path: String,
        operations_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ThreadComposeChanged {
        id: Uuid,
        #[serde(default)]
        restorations: Vec<crate::core::store::ComposeRestoration>,
        text: String,
        #[serde(default)]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// INVARIANT: Per-draft dropdown selections (target/scope, coding agent, Lucidos
        /// model + reasoning, coding-agent model + reasoning) as a partial
        /// `ComposeSelectionOverride`-shaped object. `None` = the PUT didn't
        /// touch the selection (COALESCE-preserve); receivers hydrate their
        /// per-draft store from it under the same origin/focus guards as text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<serde_json::Value>,
        /// INVARIANT: The thread's *compose epoch* after this change (`docs/glossary.md`):
        /// advanced by each submission or Stop restoration.
        /// Carried on every broadcast so a device always holds a value its next
        /// compose PUT can be fenced against, including the device whose own
        /// write is in flight. `#[serde(default)]` reads a pre-epoch payload as
        /// 0, which is also what a never-submitted thread stores.
        #[serde(default)]
        compose_epoch: i64,
        /// INVARIANT: The *compose revision*: the row's `summary_version` right after the
        /// statement that produced this report. A receiver ignores a frame at
        /// or below the revision it already holds.
        #[serde(default)]
        summary_version: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_device_id: Option<String>,
    },
    /// INVARIANT: Apply All started. Persists the full list of change IDs so the
    /// engine can resume the batch after a mid-flight conflict or an engine
    /// restart — the driver looks up the in-memory `ApplyAllRegistry`
    /// (recovered from this event on startup) when each member resolves.
    ApplyAllBatchStarted {
        batch_id: Uuid,
        change_ids: Vec<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Every member of an `ApplyAllBatchStarted` has resolved (applied or
    /// failed). Closes the batch so projections and UI know the run is
    /// over. Emitted by the driver only — the canonical signal that batch
    /// state can be dropped from memory.
    ApplyAllBatchCompleted {
        batch_id: Uuid,
        applied: Vec<Uuid>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failed: Vec<crate::engine::thread_events::apply::ApplyFailure>,
    },
    StandingApplyArmed {
        thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    StandingApplyFired {
        thread_id: Uuid,
        change_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    StandingApplyDropped {
        thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    LaneBatchStarted {
        batch_id: Uuid,
        members: Vec<crate::engine::thread_events::landing_lane::LaneMember>,
        base_sha: String,
        lane_thread_id: Uuid,
    },
    LaneBatchLanded {
        batch_id: Uuid,
        pre_sha: String,
        post_sha: String,
    },
    HardenCertificateCarried {
        batch_id: Uuid,
        from_sha: String,
        to_sha: String,
    },
    LaneBatchFailed {
        batch_id: Uuid,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The lander must publish failure results before closing the batch.
    LaneBatchFailureRequested {
        batch_id: Uuid,
        reason: String,
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The lane batch's catch-up changed its hardened head, so it hardens
    /// again. Until it lands or fails, or the cap passes, the lane queues
    /// clean arrivals instead of landing them, so `main` holds still.
    LaneArrivalsHeld {
        batch_id: Uuid,
    },
    LaneArrivalsReleased {
        batch_id: Uuid,
        cause: crate::engine::thread_events::landing_lane::LaneHoldRelease,
        arrivals: Vec<Uuid>,
    },
    MainPushFailed {
        repo: String,
        reason: String,
    },
    MainFollowRefused {
        repo: String,
        reason: String,
    },
    MainFollowed {
        repo: String,
        from_sha: String,
        to_sha: String,
    },
    RemoteChangesLanded {
        repo: String,
        from_sha: String,
        to_sha: String,
    },
    MainCheckStarted {
        check_id: Uuid,
        from_sha: String,
        to_sha: String,
    },
    MainCheckRunLinked {
        check_id: Uuid,
        run_url: String,
        branch: String,
    },
    MainCheckPassed {
        check_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_url: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        flaky: Vec<crate::engine::thread_events::main_check::FailedTest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flaky_fix_thread_id: Option<Uuid>,
    },
    MainCheckFailed {
        check_id: Uuid,
        summary: String,
        #[serde(default)]
        tail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failures: Vec<crate::engine::thread_events::main_check::FailedTest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_url: Option<String>,
        fix_thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        superseded_by_fix: Option<Uuid>,
    },
    MainCheckRepairStarted {
        check_id: Uuid,
        fix_thread_id: Uuid,
        started_by: crate::engine::thread_events::main_check::MainRepairStartedBy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    MainCheckAbandoned {
        check_id: Uuid,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_url: Option<String>,
    },
    MainCheckSuperseded {
        check_id: Uuid,
        to_sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_url: Option<String>,
    },
    MainCheckRedClosed {
        check_id: Uuid,
        reason: crate::engine::thread_events::main_check::RedCloseReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: Bash supervisor (`scripts/lib/engine_supervisor.sh`) wrapped an
    /// engine instance that died unexpectedly and is spawning its replacement.
    /// Graceful exits and the platform's SIGINT/SIGUSR1 stops are excluded. The
    /// supervisor writes a sidecar JSON file at
    /// `<workspace>/.lucidos/engine.last-death.json` before respawning;
    /// the next engine reads + emits + deletes the file at startup, so
    /// the audit timeline records the respawn even though it happens
    /// while the engine is dead. `supervisor_pid` distinguishes which
    /// bash supervisor instance handled the respawn — useful when
    /// multiple supervisors briefly coexist (restart races).
    EngineSupervisorRespawned {
        old_pid: u32,
        exit_code: i32,
        died_at: chrono::DateTime<chrono::Utc>,
        supervisor_pid: u32,
    },
    EmailSent {
        account: String,
        to: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cc: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bcc: Vec<String>,
        subject: String,
        attachment_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ProxyModulesReloaded {
        count: usize,
        names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    HandshakeScriptApproved {
        path: String,
        source: crate::core::handshake_approvals::ApprovalSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    HandshakeScriptScopeBound {
        path: String,
        base_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    HandshakeScriptInjectsBound {
        path: String,
        injects: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    CredentialScopeInferred {
        service_name: String,
        base_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: An ordinary composer input was durably accepted, before execution.
    /// Aggregate `thread`, identity `thread_id`. The immutable request and
    /// compose acknowledgement survive process loss. Recovery must retain
    /// user priority. This event is not a MessageReceived.
    OrdinarySubmissionAccepted {
        thread_id: Uuid,
        message_event_id: Uuid,
        entry_id: Uuid,
        intent: Box<crate::engine::event_bus::OrdinarySubmissionIntent>,
        request: Box<crate::engine::thread_events::queue::ThreadQueueRequest>,
        compose: Option<crate::core::store::ComposeReport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    DraftSubmissionAccepted {
        receipt: crate::core::draft_submission::DraftSubmissionReceipt,
        snapshot: crate::core::store::ComposeReport,
        request: Box<crate::engine::thread_events::queue::ThreadQueueRequest>,
        authority: crate::core::draft_submission::DraftSubmissionAuthority,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An opening addressed to one ancestor. The original card stays in its source thread.
    SubThreadCardOpened {
        ancestor_thread_id: Uuid,
        child_thread_id: Uuid,
        source_thread_id: Uuid,
        source_event_id: Uuid,
        card_kind: SubThreadCardKind,
    },
    ThreadQueued {
        entry_id: Uuid,
        kind: crate::engine::thread_events::queue::ThreadQueueKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        summary: String,
        request: serde_json::Value,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        requeued: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ThreadQueueAdmitted {
        entry_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ThreadQueueDropped {
        entry_id: Uuid,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: An admitted Thread Queue entry's work finished (any outcome — the
    /// thread's own terminal events carry success/failure). Frees the
    /// capacity slot; projection deletes the row.
    ThreadQueueCompleted {
        entry_id: Uuid,
    },
    ThreadQueueChanged {},
    /// INVARIANT: The *capacity policy* governing the Thread Queue changed. Persisted —
    /// the latest event IS the stored policy (reconstructed at boot); no
    /// separate config table.
    CapacityPolicyChanged {
        policy: crate::engine::thread_events::queue::CapacityPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// INVARIANT: The user started Disk Usage's recommended cleanup. PERSISTED. The pass
    /// runs past any HTTP request, so it ends in exactly one
    /// `RecommendedCleanupCompleted` or `RecommendedCleanupFailed`.
    RecommendedCleanupStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    RecommendedCleanupProgress {
        done: u32,
        total: u32,
    },
    RecommendedCleanupCompleted {
        removed_count: u32,
        cleaned_count: u32,
        freed_bytes: u64,
    },
    RecommendedCleanupFailed {
        error: String,
    },
}

impl SystemEvent {
    pub const SUB_THREAD_CARD_OPENED: &str = "SubThreadCardOpened";
    pub const SUB_THREAD_CARD_AGGREGATE: &str = "sub_thread_card";

    pub fn artifact_change(
        file_exists: bool,
        artifact_path: String,
        commit: String,
        source: Option<String>,
        writer_thread_id: Option<uuid::Uuid>,
        writer: ArtifactWriter,
    ) -> Self {
        if file_exists {
            Self::ArtifactUpdated {
                artifact_path,
                commit,
                source,
                writer_thread_id,
                writer: Some(writer),
            }
        } else {
            Self::ArtifactCreated {
                artifact_path,
                commit,
                source,
                writer_thread_id,
                writer: Some(writer),
            }
        }
    }

    pub const PERSISTED_TYPE_NAMES: &'static [&'static str] = &[
        "EngineBuildCompleted",
        "EngineVersionActivated",
        "ServedFrontendCommitChanged",
        "NotificationCreated",
        "PreferencesChanged",
        "ReleaseNoticeResolved",
        "ArtifactImported",
        "ArtifactCreated",
        "ArtifactUpdated",
        "ArtifactDeleted",
        "AppDeleted",
        "AppOpened",
        "DashboardTileAdded",
        "DashboardTileRemoved",
        "DashboardArranged",
        "KnowhowRead",
        "RepositoryImported",
        "TriggerCreated",
        "TriggerUpdated",
        "TriggerDeleted",
        "TriggerEnabled",
        "TriggerDisabled",
        "TriggerExecuted",
        "TriggerGroupCreated",
        "TriggerGroupRenamed",
        "TriggerGroupReordered",
        "TriggerGroupDeleted",
        "EventHookArmed",
        "EventHookDisarmed",
        "EventHookRemoved",
        "EventHookRefused",
        "TriggerCompleted",
        "TriggerSlotsSkipped",
        "LanguageSet",
        "TimezoneSet",
        "ChangeDiscarded",
        "PluginInstalled",
        "PluginLocalChangesMerged",
        "PluginUninstalled",
        "PluginInstallCanceled",
        "PluginUninstallCanceled",
        "PluginMarketplaceRegistered",
        "PluginMarketplaceRemoved",
        "PinnedAppPinned",
        "PinnedAppUnpinned",
        "DeviceRegistered",
        "DeviceRenamed",
        "DevicePushChanged",
        "DeviceDeleted",
        "DeviceHandedOver",
        "RepositoryAdded",
        "RepositoryRemoved",
        "CredentialCreated",
        "CredentialUpdated",
        "CredentialDeleted",
        "CredentialRevealed",
        "EnvironmentVariableSet",
        "EnvironmentVariableDeleted",
        "ModelCreated",
        "ModelUpdated",
        "ModelDeleted",
        "WebhookCreated",
        "WebhookUpdated",
        "WebhookDeleted",
        "WebhookIngressDegraded",
        "WebhookIngressRecovered",
        "WebhookIngressUnverified",
        "WebhookIngressVerified",
        "WebhookDeliveriesSleptThrough",
        "WebhookDeliveriesRefused",
        "WebhookDeliveriesRecovered",
        "McpServerRegistered",
        "McpServerUpdated",
        "McpServerDisabledToolsChanged",
        "McpServerRemoved",
        "McpServerStopped",
        "McpLaunchApproved",
        "PermissionGrantsChanged",
        "OAuthAccountConnected",
        "OAuthAccountDeleted",
        "ClaudeGatewaySignedIn",
        "ClaudeGatewaySignedOut",
        "ClaudeGatewaySignInStarted",
        "ClaudeGatewaySignInCanceled",
        "ClaudeGatewaySignInFailed",
        "ClaudeGatewayManagedSettingsChanged",
        "ChatGptPlanSignedIn",
        "ChatGptPlanSignedOut",
        "ChatGptPlanSignInStarted",
        "ChatGptPlanSignInCanceled",
        "ChatGptPlanSignInFailed",
        "ChatGptPlanLimitReached",
        "DataFileWritten",
        "DataFileDeleted",
        "DataFileEdited",
        "ApplyAllBatchStarted",
        "ApplyAllBatchCompleted",
        "StandingApplyArmed",
        "StandingApplyFired",
        "StandingApplyDropped",
        "LaneBatchStarted",
        "LaneBatchLanded",
        "HardenCertificateCarried",
        "LaneBatchFailed",
        "LaneBatchFailureRequested",
        "LaneArrivalsHeld",
        "LaneArrivalsReleased",
        "MainPushFailed",
        "MainFollowRefused",
        "MainFollowed",
        "RemoteChangesLanded",
        "MainCheckStarted",
        "MainCheckRunLinked",
        "MainCheckPassed",
        "MainCheckFailed",
        "MainCheckRepairStarted",
        "MainCheckAbandoned",
        "MainCheckSuperseded",
        "MainCheckRedClosed",
        "EngineSupervisorRespawned",
        "InternalFaultRecorded",
        "DiagnosticBundleSent",
        "DiagnosticBundleReceived",
        "EmailSent",
        "ProxyModulesReloaded",
        "HandshakeScriptApproved",
        "HandshakeScriptScopeBound",
        "HandshakeScriptInjectsBound",
        "CredentialScopeInferred",
        "OrdinarySubmissionAccepted",
        "DraftSubmissionAccepted",
        "SubThreadCardOpened",
        "ThreadQueued",
        "ThreadQueueAdmitted",
        "ThreadQueueDropped",
        "ThreadQueueCompleted",
        "CapacityPolicyChanged",
        "SnapshotRestoreRequested",
        "WorkspaceRestored",
        "SnapshotCreated",
        "SnapshotCaptureFailed",
        "SnapshotChecked",
        "SnapshotKeepChanged",
        "SnapshotDeleted",
        "BackupCompleted",
        "BackupFailed",
        "BackupKeyRevealed",
        "BackupRecipientSet",
        "ProxyConfigRejected",
        "ModelSpendCapReached",
        "ModelSpendCapHoldEnded",
        "ModelNotServedObserved",
        "ThreadsDeleted",
        "MemoryCorrected",
        "MemoryExtractionFailed",
        "RecommendedCleanupStarted",
        "RecommendedCleanupCompleted",
        "RecommendedCleanupFailed",
        "TreeBackfillCompleted",
        "TreeBackfillReset",
        "TreeBackfillStarted",
        "TreeBackfillCapReached",
        "TreeBackfillCapRaised",
        "TreeBackfillEnded",
    ];

    pub fn is_persisted(&self) -> bool {
        match self {
            Self::DomainEvent { transient, .. } => !transient,
            _ => Self::is_persisted_type_name(self.event_type()),
        }
    }

    /// INVARIANT: [`Self::is_persisted`] for a bare wire name, which is all a validator or
    /// a stored row has in hand.
    ///
    /// This is NOT the emit guard. Waiting on a name and being allowed to POST
    /// it are separate permissions: [`Self::is_reserved_type_name`] answers the
    /// second and stays as strict as it is.
    pub fn is_persisted_type_name(name: &str) -> bool {
        Self::PERSISTED_TYPE_NAMES.contains(&name)
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            Self::NotificationCreated { .. } => "NotificationCreated",
            Self::NotificationRead { .. } => "NotificationRead",
            Self::NotificationsAllRead { .. } => "NotificationsAllRead",
            Self::PreferencesChanged { .. } => "PreferencesChanged",
            Self::ReleaseNoticeResolved { .. } => "ReleaseNoticeResolved",
            Self::MemoryRebuildProgress { .. } => "MemoryRebuildProgress",
            Self::EmbeddingModelStatusChanged { .. } => "EmbeddingModelStatusChanged",
            Self::TreeBackfillProgressed { .. } => "TreeBackfillProgressed",
            Self::TreeBackfillCompleted { .. } => "TreeBackfillCompleted",
            Self::TreeBackfillReset { .. } => "TreeBackfillReset",
            Self::TreeBackfillStarted { .. } => "TreeBackfillStarted",
            Self::TreeBackfillCapReached { .. } => "TreeBackfillCapReached",
            Self::TreeBackfillCapRaised { .. } => "TreeBackfillCapRaised",
            Self::TreeBackfillEnded { .. } => "TreeBackfillEnded",
            Self::ChangesUpdated { .. } => "ChangesUpdated",
            Self::SnapshotProgress { .. } => "SnapshotProgress",
            Self::SnapshotRestoreRequested { .. } => "SnapshotRestoreRequested",
            Self::WorkspaceRestored { .. } => "WorkspaceRestored",
            Self::SnapshotCreated { .. } => "SnapshotCreated",
            Self::SnapshotCaptureFailed { .. } => "SnapshotCaptureFailed",
            Self::SnapshotChecked { .. } => "SnapshotChecked",
            Self::SnapshotKeepChanged { .. } => "SnapshotKeepChanged",
            Self::SnapshotDeleted { .. } => "SnapshotDeleted",
            Self::BackupProgress { .. } => "BackupProgress",
            Self::BackupCompleted { .. } => "BackupCompleted",
            Self::BackupFailed { .. } => "BackupFailed",
            Self::BackupKeyRevealed { .. } => "BackupKeyRevealed",
            Self::BackupRecipientSet { .. } => "BackupRecipientSet",
            Self::ProxyConfigRejected { .. } => "ProxyConfigRejected",
            Self::ModelSpendCapReached { .. } => "ModelSpendCapReached",
            Self::ModelSpendCapHoldEnded { .. } => "ModelSpendCapHoldEnded",
            Self::ModelNotServedObserved { .. } => "ModelNotServedObserved",
            Self::ThreadsDeleted { .. } => "ThreadsDeleted",
            Self::RecoveryProgress { .. } => "RecoveryProgress",
            Self::MemoryCorrected { .. } => "MemoryCorrected",
            Self::MemoryExtractionFailed { .. } => "MemoryExtractionFailed",
            Self::Toast { .. } => "Toast",
            Self::ArtifactImported { .. } => "ArtifactImported",
            Self::TriggerCreated { .. } => "TriggerCreated",
            Self::TriggerUpdated { .. } => "TriggerUpdated",
            Self::TriggerDeleted { .. } => "TriggerDeleted",
            Self::TriggerEnabled { .. } => "TriggerEnabled",
            Self::TriggerDisabled { .. } => "TriggerDisabled",
            Self::TriggerExecuted { .. } => "TriggerExecuted",
            Self::TriggerGroupCreated { .. } => "TriggerGroupCreated",
            Self::TriggerGroupRenamed { .. } => "TriggerGroupRenamed",
            Self::EventHookArmed { .. } => "EventHookArmed",
            Self::EventHookDisarmed { .. } => "EventHookDisarmed",
            Self::EventHookRemoved { .. } => "EventHookRemoved",
            Self::EventHookRefused { .. } => "EventHookRefused",
            Self::TriggerGroupReordered { .. } => "TriggerGroupReordered",
            Self::TriggerGroupDeleted { .. } => "TriggerGroupDeleted",
            Self::AppCreated { .. } => "AppCreated",
            Self::AppUpdated { .. } => "AppUpdated",
            Self::AppDeleted { .. } => "AppDeleted",
            Self::AppOpened { .. } => "AppOpened",
            Self::DashboardTileAdded { .. } => "DashboardTileAdded",
            Self::DashboardTileRemoved { .. } => "DashboardTileRemoved",
            Self::DashboardArranged { .. } => "DashboardArranged",
            Self::KnowhowRead { .. } => "KnowhowRead",
            Self::AppUiRefreshRequested { .. } => "AppUiRefreshRequested",
            Self::FrontendUpdateDeferred { .. } => "FrontendUpdateDeferred",
            Self::FrontendUpdateStranded { .. } => "FrontendUpdateStranded",
            Self::ServedFrontendAdvanced { .. } => "ServedFrontendAdvanced",
            Self::FrontendPreviewStarted { .. } => "FrontendPreviewStarted",
            Self::FrontendPreviewStopped { .. } => "FrontendPreviewStopped",
            Self::EngineBuildCompleted { .. } => "EngineBuildCompleted",
            Self::EngineVersionActivated { .. } => "EngineVersionActivated",
            Self::ServedFrontendCommitChanged { .. } => "ServedFrontendCommitChanged",
            Self::EngineBuildStateChanged { .. } => "EngineBuildStateChanged",
            Self::EngineVersionStatusChanged { .. } => "EngineVersionStatusChanged",
            Self::FrontendRefreshStateChanged { .. } => "FrontendRefreshStateChanged",
            Self::FrontendBuildWatchStateChanged { .. } => "FrontendBuildWatchStateChanged",
            Self::DomainEvent { .. } => "DomainEvent",
            Self::ArtifactCreated { .. } => "ArtifactCreated",
            Self::ArtifactUpdated { .. } => "ArtifactUpdated",
            Self::ArtifactDeleted { .. } => "ArtifactDeleted",
            Self::LanguageSet { .. } => "LanguageSet",
            Self::TimezoneSet { .. } => "TimezoneSet",
            Self::RepositoryImported { .. } => "RepositoryImported",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::TriggerSlotsSkipped { .. } => "TriggerSlotsSkipped",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::DeviceVisible { .. } => "DeviceVisible",
            Self::DeviceHidden { .. } => "DeviceHidden",
            Self::PresenceCheck { .. } => "PresenceCheck",
            Self::NativePushRequested { .. } => "NativePushRequested",
            Self::NativePushDismissRequested { .. } => "NativePushDismissRequested",
            Self::UsersChanged => "UsersChanged",
            Self::PairingRequestsChanged => "PairingRequestsChanged",
            Self::PluginInstalled { .. } => "PluginInstalled",
            Self::PluginLocalChangesMerged { .. } => "PluginLocalChangesMerged",
            Self::PluginUninstalled { .. } => "PluginUninstalled",
            Self::PluginInstallCanceled { .. } => "PluginInstallCanceled",
            Self::PluginUninstallCanceled { .. } => "PluginUninstallCanceled",
            Self::PluginMarketplaceRegistered { .. } => "PluginMarketplaceRegistered",
            Self::PluginMarketplaceRemoved { .. } => "PluginMarketplaceRemoved",
            Self::PluginCatalogScanStarted {} => "PluginCatalogScanStarted",
            Self::PluginCatalogScanned { .. } => "PluginCatalogScanned",
            Self::ThreadComposeChanged { .. } => "ThreadComposeChanged",
            Self::PinnedAppPinned { .. } => "PinnedAppPinned",
            Self::PinnedAppUnpinned { .. } => "PinnedAppUnpinned",
            Self::DeviceRegistered { .. } => "DeviceRegistered",
            Self::DeviceRenamed { .. } => "DeviceRenamed",
            Self::DevicePushChanged { .. } => "DevicePushChanged",
            Self::DeviceDeleted { .. } => "DeviceDeleted",
            Self::DeviceHandedOver { .. } => "DeviceHandedOver",
            Self::RepositoryAdded { .. } => "RepositoryAdded",
            Self::RepositoryRemoved { .. } => "RepositoryRemoved",
            Self::CredentialCreated { .. } => "CredentialCreated",
            Self::CredentialUpdated { .. } => "CredentialUpdated",
            Self::CredentialDeleted { .. } => "CredentialDeleted",
            Self::CredentialRevealed { .. } => "CredentialRevealed",
            Self::EnvironmentVariableSet { .. } => "EnvironmentVariableSet",
            Self::EnvironmentVariableDeleted { .. } => "EnvironmentVariableDeleted",
            Self::ModelCreated { .. } => "ModelCreated",
            Self::ModelUpdated { .. } => "ModelUpdated",
            Self::ModelDeleted { .. } => "ModelDeleted",
            Self::WebhookCreated { .. } => "WebhookCreated",
            Self::WebhookUpdated { .. } => "WebhookUpdated",
            Self::WebhookDeleted { .. } => "WebhookDeleted",
            Self::WebhookIngressDegraded { .. } => "WebhookIngressDegraded",
            Self::WebhookIngressRecovered { .. } => "WebhookIngressRecovered",
            Self::WebhookIngressUnverified { .. } => "WebhookIngressUnverified",
            Self::WebhookIngressVerified { .. } => "WebhookIngressVerified",
            Self::WebhookDeliveriesSleptThrough { .. } => "WebhookDeliveriesSleptThrough",
            Self::WebhookDeliveriesRefused { .. } => "WebhookDeliveriesRefused",
            Self::WebhookDeliveriesRecovered { .. } => "WebhookDeliveriesRecovered",
            Self::McpServerRegistered { .. } => "McpServerRegistered",
            Self::McpServerUpdated { .. } => "McpServerUpdated",
            Self::McpServerDisabledToolsChanged { .. } => "McpServerDisabledToolsChanged",
            Self::McpServerRemoved { .. } => "McpServerRemoved",
            Self::McpServerStopped { .. } => "McpServerStopped",
            Self::McpLaunchApproved { .. } => "McpLaunchApproved",
            Self::PermissionGrantsChanged { .. } => "PermissionGrantsChanged",
            Self::OAuthAccountConnected { .. } => "OAuthAccountConnected",
            Self::OAuthAccountDeleted { .. } => "OAuthAccountDeleted",
            Self::ClaudeGatewaySignedIn { .. } => "ClaudeGatewaySignedIn",
            Self::ClaudeGatewaySignedOut { .. } => "ClaudeGatewaySignedOut",
            Self::ClaudeGatewaySignInStarted { .. } => "ClaudeGatewaySignInStarted",
            Self::ClaudeGatewaySignInCanceled { .. } => "ClaudeGatewaySignInCanceled",
            Self::ClaudeGatewaySignInFailed { .. } => "ClaudeGatewaySignInFailed",
            Self::ClaudeGatewayManagedSettingsChanged { .. } => {
                "ClaudeGatewayManagedSettingsChanged"
            }
            Self::ChatGptPlanSignedIn { .. } => "ChatGptPlanSignedIn",
            Self::ChatGptPlanSignedOut { .. } => "ChatGptPlanSignedOut",
            Self::ChatGptPlanSignInStarted { .. } => "ChatGptPlanSignInStarted",
            Self::ChatGptPlanSignInCanceled { .. } => "ChatGptPlanSignInCanceled",
            Self::ChatGptPlanSignInFailed { .. } => "ChatGptPlanSignInFailed",
            Self::ChatGptPlanLimitReached { .. } => "ChatGptPlanLimitReached",
            Self::DataFileWritten { .. } => "DataFileWritten",
            Self::DataFileDeleted { .. } => "DataFileDeleted",
            Self::DataFileEdited { .. } => "DataFileEdited",
            Self::ApplyAllBatchStarted { .. } => "ApplyAllBatchStarted",
            Self::ApplyAllBatchCompleted { .. } => "ApplyAllBatchCompleted",
            Self::StandingApplyArmed { .. } => "StandingApplyArmed",
            Self::StandingApplyFired { .. } => "StandingApplyFired",
            Self::StandingApplyDropped { .. } => "StandingApplyDropped",
            Self::LaneBatchStarted { .. } => "LaneBatchStarted",
            Self::LaneBatchLanded { .. } => "LaneBatchLanded",
            Self::HardenCertificateCarried { .. } => "HardenCertificateCarried",
            Self::LaneBatchFailed { .. } => "LaneBatchFailed",
            Self::LaneBatchFailureRequested { .. } => "LaneBatchFailureRequested",
            Self::LaneArrivalsHeld { .. } => "LaneArrivalsHeld",
            Self::LaneArrivalsReleased { .. } => "LaneArrivalsReleased",
            Self::MainPushFailed { .. } => "MainPushFailed",
            Self::MainFollowRefused { .. } => "MainFollowRefused",
            Self::MainFollowed { .. } => "MainFollowed",
            Self::RemoteChangesLanded { .. } => "RemoteChangesLanded",
            Self::MainCheckStarted { .. } => "MainCheckStarted",
            Self::MainCheckRunLinked { .. } => "MainCheckRunLinked",
            Self::MainCheckPassed { .. } => "MainCheckPassed",
            Self::MainCheckFailed { .. } => "MainCheckFailed",
            Self::MainCheckRepairStarted { .. } => "MainCheckRepairStarted",
            Self::MainCheckAbandoned { .. } => "MainCheckAbandoned",
            Self::MainCheckSuperseded { .. } => "MainCheckSuperseded",
            Self::MainCheckRedClosed { .. } => "MainCheckRedClosed",
            Self::InternalFaultRecorded { .. } => "InternalFaultRecorded",
            Self::DiagnosticBundleSent { .. } => "DiagnosticBundleSent",
            Self::DiagnosticBundleReceived { .. } => "DiagnosticBundleReceived",
            Self::EngineSupervisorRespawned { .. } => "EngineSupervisorRespawned",
            Self::EmailSent { .. } => "EmailSent",
            Self::ProxyModulesReloaded { .. } => "ProxyModulesReloaded",
            Self::HandshakeScriptApproved { .. } => "HandshakeScriptApproved",
            Self::HandshakeScriptScopeBound { .. } => "HandshakeScriptScopeBound",
            Self::HandshakeScriptInjectsBound { .. } => "HandshakeScriptInjectsBound",
            Self::CredentialScopeInferred { .. } => "CredentialScopeInferred",
            Self::OrdinarySubmissionAccepted { .. } => "OrdinarySubmissionAccepted",
            Self::DraftSubmissionAccepted { .. } => "DraftSubmissionAccepted",
            Self::SubThreadCardOpened { .. } => Self::SUB_THREAD_CARD_OPENED,
            Self::ThreadQueued { .. } => "ThreadQueued",
            Self::ThreadQueueAdmitted { .. } => "ThreadQueueAdmitted",
            Self::ThreadQueueDropped { .. } => "ThreadQueueDropped",
            Self::ThreadQueueCompleted { .. } => "ThreadQueueCompleted",
            Self::ThreadQueueChanged {} => "ThreadQueueChanged",
            Self::CapacityPolicyChanged { .. } => "CapacityPolicyChanged",
            Self::RecommendedCleanupStarted { .. } => "RecommendedCleanupStarted",
            Self::RecommendedCleanupProgress { .. } => "RecommendedCleanupProgress",
            Self::RecommendedCleanupCompleted { .. } => "RecommendedCleanupCompleted",
            Self::RecommendedCleanupFailed { .. } => "RecommendedCleanupFailed",
        }
    }

    /// INVARIANT: The name this event is filed under: its `events.event_type` column, and
    /// the name a subscriber writes in an `on:` entry.
    ///
    /// Identical to [`Self::event_type`] for every variant but `DomainEvent`,
    /// which is a transport wrapper. A domain event is stored and matched under
    /// the inner name the workspace chose, never the literal `"DomainEvent"`.
    ///
    /// Use this wherever a name is compared against a row or a subscription.
    /// Reserve [`Self::event_type`] for the Rust variant's own name.
    pub fn stored_event_type(&self) -> &str {
        match self {
            Self::DomainEvent { event_type, .. } => event_type.as_str(),
            _ => self.event_type(),
        }
    }

    /// INVARIANT: Wire-format `type` names that the engine emits as system frames
    /// (every `SystemEvent` variant plus the `ThreadEvent` wrapper). The
    /// `emit_event` HTTP API rejects these so untrusted apps cannot forge
    /// frames such as a fake `NotificationCreated`. Keep in sync with the
    /// `SystemEvent` enum — the `reserved_type_names_match_event_type`
    /// test catches drift.
    pub const RESERVED_TYPE_NAMES: &'static [&'static str] = &[
        "NotificationCreated",
        "NotificationRead",
        "NotificationsAllRead",
        "PreferencesChanged",
        "ReleaseNoticeResolved",
        "MemoryRebuildProgress",
        "EmbeddingModelStatusChanged",
        "TreeBackfillProgressed",
        "TreeBackfillCompleted",
        "TreeBackfillReset",
        "TreeBackfillStarted",
        "TreeBackfillCapReached",
        "TreeBackfillCapRaised",
        "TreeBackfillEnded",
        "ChangesUpdated",
        "SnapshotProgress",
        "SnapshotRestoreRequested",
        "WorkspaceRestored",
        "SnapshotCreated",
        "SnapshotCaptureFailed",
        "SnapshotChecked",
        "SnapshotKeepChanged",
        "SnapshotDeleted",
        "BackupProgress",
        "BackupCompleted",
        "BackupFailed",
        "BackupKeyRevealed",
        "BackupRecipientSet",
        "ProxyConfigRejected",
        "ModelSpendCapReached",
        "ModelSpendCapHoldEnded",
        "ModelNotServedObserved",
        "ThreadsDeleted",
        "RecoveryProgress",
        "MemoryCorrected",
        "MemoryExtractionFailed",
        "Toast",
        "ArtifactImported",
        "TriggerCreated",
        "TriggerUpdated",
        "TriggerDeleted",
        "TriggerEnabled",
        "TriggerDisabled",
        "TriggerExecuted",
        "TriggerGroupCreated",
        "TriggerGroupRenamed",
        "TriggerGroupReordered",
        "TriggerGroupDeleted",
        "EventHookArmed",
        "EventHookDisarmed",
        "EventHookRemoved",
        "EventHookRefused",
        "AppCreated",
        "AppUpdated",
        "AppDeleted",
        "AppOpened",
        "DashboardTileAdded",
        "DashboardTileRemoved",
        "DashboardArranged",
        "KnowhowRead",
        "AppUiRefreshRequested",
        "FrontendUpdateDeferred",
        "FrontendUpdateStranded",
        "ServedFrontendAdvanced",
        "FrontendPreviewStarted",
        "FrontendPreviewStopped",
        "EngineBuildCompleted",
        "EngineVersionActivated",
        "ServedFrontendCommitChanged",
        "EngineBuildStateChanged",
        "EngineVersionStatusChanged",
        "FrontendRefreshStateChanged",
        "FrontendBuildWatchStateChanged",
        "DomainEvent",
        "ArtifactCreated",
        "ArtifactUpdated",
        "ArtifactDeleted",
        "LanguageSet",
        "TimezoneSet",
        "RepositoryImported",
        "TriggerCompleted",
        "TriggerSlotsSkipped",
        "ChangeDiscarded",
        "DeviceVisible",
        "DeviceHidden",
        "PresenceCheck",
        "NativePushRequested",
        "NativePushDismissRequested",
        "UsersChanged",
        "PairingRequestsChanged",
        "PluginInstalled",
        "PluginLocalChangesMerged",
        "PluginUninstalled",
        "PluginInstallCanceled",
        "PluginUninstallCanceled",
        "PluginMarketplaceRegistered",
        "PluginMarketplaceRemoved",
        "PluginCatalogScanStarted",
        "PluginCatalogScanned",
        "ThreadComposeChanged",
        "PinnedAppPinned",
        "PinnedAppUnpinned",
        "DeviceRegistered",
        "DeviceRenamed",
        "DevicePushChanged",
        "DeviceDeleted",
        "DeviceHandedOver",
        "RepositoryAdded",
        "RepositoryRemoved",
        "CredentialCreated",
        "CredentialUpdated",
        "CredentialDeleted",
        "CredentialRevealed",
        "EnvironmentVariableSet",
        "EnvironmentVariableDeleted",
        "ModelCreated",
        "ModelUpdated",
        "ModelDeleted",
        "WebhookCreated",
        "WebhookUpdated",
        "WebhookDeleted",
        "WebhookIngressDegraded",
        "WebhookIngressRecovered",
        "WebhookIngressUnverified",
        "WebhookIngressVerified",
        "WebhookDeliveriesSleptThrough",
        "WebhookDeliveriesRefused",
        "WebhookDeliveriesRecovered",
        "McpServerRegistered",
        "McpServerUpdated",
        "McpServerDisabledToolsChanged",
        "McpServerRemoved",
        "McpServerStopped",
        "McpLaunchApproved",
        "PermissionGrantsChanged",
        "OAuthAccountConnected",
        "OAuthAccountDeleted",
        "ClaudeGatewaySignedIn",
        "ClaudeGatewaySignedOut",
        "ClaudeGatewaySignInStarted",
        "ClaudeGatewaySignInCanceled",
        "ClaudeGatewaySignInFailed",
        "ClaudeGatewayManagedSettingsChanged",
        "ChatGptPlanSignedIn",
        "ChatGptPlanSignedOut",
        "ChatGptPlanSignInStarted",
        "ChatGptPlanSignInCanceled",
        "ChatGptPlanSignInFailed",
        "ChatGptPlanLimitReached",
        "DataFileWritten",
        "DataFileDeleted",
        "DataFileEdited",
        "ApplyAllBatchStarted",
        "ApplyAllBatchCompleted",
        "StandingApplyArmed",
        "StandingApplyFired",
        "StandingApplyDropped",
        "LaneBatchStarted",
        "LaneBatchLanded",
        "HardenCertificateCarried",
        "LaneBatchFailed",
        "LaneBatchFailureRequested",
        "LaneArrivalsHeld",
        "LaneArrivalsReleased",
        "MainPushFailed",
        "MainFollowRefused",
        "MainFollowed",
        "RemoteChangesLanded",
        "MainCheckStarted",
        "MainCheckRunLinked",
        "MainCheckPassed",
        "MainCheckFailed",
        "MainCheckRepairStarted",
        "MainCheckAbandoned",
        "MainCheckSuperseded",
        "MainCheckRedClosed",
        "EngineSupervisorRespawned",
        "InternalFaultRecorded",
        "DiagnosticBundleSent",
        "DiagnosticBundleReceived",
        "EmailSent",
        "ProxyModulesReloaded",
        "HandshakeScriptApproved",
        "HandshakeScriptScopeBound",
        "HandshakeScriptInjectsBound",
        "CredentialScopeInferred",
        "OrdinarySubmissionAccepted",
        "DraftSubmissionAccepted",
        "SubThreadCardOpened",
        "ThreadQueued",
        "ThreadQueueAdmitted",
        "ThreadQueueDropped",
        "ThreadQueueCompleted",
        "ThreadQueueChanged",
        "CapacityPolicyChanged",
        "RecommendedCleanupStarted",
        "RecommendedCleanupProgress",
        "RecommendedCleanupCompleted",
        "RecommendedCleanupFailed",
        "ThreadEvent",
    ];

    pub fn is_reserved_type_name(name: &str) -> bool {
        Self::RESERVED_TYPE_NAMES.contains(&name)
    }

    pub fn aggregate(&self) -> &str {
        match self {
            Self::DiagnosticBundleReceived { .. } => "diagnostic_bundle",
            Self::NotificationCreated { .. }
            | Self::NotificationRead { .. }
            | Self::NotificationsAllRead { .. }
            | Self::NativePushRequested { .. }
            | Self::NativePushDismissRequested { .. } => "notification",
            Self::UsersChanged => "user",
            Self::PairingRequestsChanged => "pairing",
            Self::PreferencesChanged { .. }
            | Self::LanguageSet { .. }
            | Self::TimezoneSet { .. } => "preference",
            Self::ReleaseNoticeResolved { .. } => "release_notice",
            Self::ChangesUpdated { .. } | Self::ChangeDiscarded { .. } => "change",
            Self::SnapshotProgress { .. }
            | Self::SnapshotRestoreRequested { .. }
            | Self::WorkspaceRestored { .. }
            | Self::SnapshotCreated { .. }
            | Self::SnapshotCaptureFailed { .. }
            | Self::SnapshotChecked { .. }
            | Self::SnapshotKeepChanged { .. }
            | Self::SnapshotDeleted { .. } => "snapshot",
            Self::MemoryRebuildProgress { .. }
            | Self::EmbeddingModelStatusChanged { .. }
            | Self::BackupProgress { .. }
            | Self::BackupCompleted { .. }
            | Self::BackupFailed { .. }
            | Self::BackupKeyRevealed { .. }
            | Self::BackupRecipientSet { .. }
            | Self::ProxyConfigRejected { .. }
            | Self::ModelSpendCapReached { .. }
            | Self::ModelSpendCapHoldEnded { .. }
            | Self::ModelNotServedObserved { .. }
            | Self::ThreadsDeleted { .. }
            | Self::RecoveryProgress { .. }
            | Self::RecommendedCleanupStarted { .. }
            | Self::RecommendedCleanupProgress { .. }
            | Self::RecommendedCleanupCompleted { .. }
            | Self::RecommendedCleanupFailed { .. }
            | Self::Toast { .. } => "ops",
            Self::ArtifactImported { .. }
            | Self::ArtifactCreated { .. }
            | Self::ArtifactUpdated { .. }
            | Self::ArtifactDeleted { .. }
            | Self::RepositoryImported { .. } => "artifact",
            Self::TriggerCreated { .. }
            | Self::TriggerUpdated { .. }
            | Self::TriggerDeleted { .. }
            | Self::TriggerEnabled { .. }
            | Self::TriggerDisabled { .. }
            | Self::TriggerExecuted { .. }
            | Self::TriggerCompleted { .. }
            | Self::TriggerSlotsSkipped { .. } => "trigger",
            Self::TriggerGroupCreated { .. }
            | Self::TriggerGroupRenamed { .. }
            | Self::TriggerGroupReordered { .. }
            | Self::TriggerGroupDeleted { .. } => "trigger_group",
            Self::EventHookArmed { .. }
            | Self::EventHookDisarmed { .. }
            | Self::EventHookRemoved { .. } => crate::core::event_hooks::HOOK_AGGREGATE,
            Self::EventHookRefused { .. } => crate::core::event_hooks::REFUSAL_AGGREGATE,
            Self::AppCreated { .. }
            | Self::AppUpdated { .. }
            | Self::AppDeleted { .. }
            | Self::AppOpened { .. }
            | Self::AppUiRefreshRequested { .. } => "app",
            Self::DashboardTileAdded { .. }
            | Self::DashboardTileRemoved { .. }
            | Self::DashboardArranged { .. } => "dashboard",
            Self::KnowhowRead { .. } => "knowhow",
            Self::DomainEvent { .. } => "domain",
            Self::DeviceVisible { .. } | Self::DeviceHidden { .. } => "device_presence",
            Self::PresenceCheck { .. } => "presence",
            Self::PluginInstalled { .. }
            | Self::PluginLocalChangesMerged { .. }
            | Self::PluginUninstalled { .. }
            | Self::PluginInstallCanceled { .. }
            | Self::PluginUninstallCanceled { .. } => "plugin",
            Self::PluginMarketplaceRegistered { .. }
            | Self::PluginMarketplaceRemoved { .. }
            | Self::PluginCatalogScanStarted {}
            | Self::PluginCatalogScanned { .. } => "plugin_marketplace",
            Self::ThreadComposeChanged { .. }
            | Self::DraftSubmissionAccepted { .. }
            | Self::OrdinarySubmissionAccepted { .. } => "thread",
            Self::MemoryExtractionFailed { .. }
            | Self::MemoryCorrected { .. }
            | Self::TreeBackfillProgressed { .. }
            | Self::TreeBackfillCompleted { .. }
            | Self::TreeBackfillReset { .. }
            | Self::TreeBackfillStarted { .. }
            | Self::TreeBackfillCapReached { .. }
            | Self::TreeBackfillCapRaised { .. }
            | Self::TreeBackfillEnded { .. } => "memory",
            Self::PinnedAppPinned { .. } | Self::PinnedAppUnpinned { .. } => "pinned_app",
            Self::DeviceRegistered { .. }
            | Self::DeviceRenamed { .. }
            | Self::DevicePushChanged { .. }
            | Self::DeviceDeleted { .. }
            | Self::DeviceHandedOver { .. } => "device",
            Self::RepositoryAdded { .. } | Self::RepositoryRemoved { .. } => "repository",
            Self::CredentialCreated { .. }
            | Self::CredentialUpdated { .. }
            | Self::CredentialDeleted { .. }
            | Self::CredentialRevealed { .. }
            | Self::CredentialScopeInferred { .. } => "credential",
            Self::EnvironmentVariableSet { .. } | Self::EnvironmentVariableDeleted { .. } => {
                "environment_variable"
            }
            Self::ModelCreated { .. } | Self::ModelUpdated { .. } | Self::ModelDeleted { .. } => {
                "model"
            }
            Self::WebhookCreated { .. }
            | Self::WebhookUpdated { .. }
            | Self::WebhookDeleted { .. }
            | Self::WebhookIngressDegraded { .. }
            | Self::WebhookIngressRecovered { .. }
            | Self::WebhookIngressUnverified { .. }
            | Self::WebhookIngressVerified { .. }
            | Self::WebhookDeliveriesSleptThrough { .. }
            | Self::WebhookDeliveriesRefused { .. }
            | Self::WebhookDeliveriesRecovered { .. } => "webhook",
            Self::McpServerRegistered { .. }
            | Self::McpServerUpdated { .. }
            | Self::McpServerDisabledToolsChanged { .. }
            | Self::McpServerRemoved { .. }
            | Self::McpServerStopped { .. }
            | Self::McpLaunchApproved { .. } => "mcp_server",
            Self::PermissionGrantsChanged { .. } => "permission_grant",
            Self::OAuthAccountConnected { .. } | Self::OAuthAccountDeleted { .. } => {
                "oauth_account"
            }
            Self::ClaudeGatewaySignedIn { .. }
            | Self::ClaudeGatewaySignedOut { .. }
            | Self::ClaudeGatewaySignInStarted { .. }
            | Self::ClaudeGatewaySignInCanceled { .. }
            | Self::ClaudeGatewaySignInFailed { .. }
            | Self::ClaudeGatewayManagedSettingsChanged { .. } => "claude_gateway",
            Self::ChatGptPlanSignedIn { .. }
            | Self::ChatGptPlanSignedOut { .. }
            | Self::ChatGptPlanSignInStarted { .. }
            | Self::ChatGptPlanSignInCanceled { .. }
            | Self::ChatGptPlanSignInFailed { .. }
            | Self::ChatGptPlanLimitReached { .. } => "chatgpt_plan",
            Self::DataFileWritten { .. }
            | Self::DataFileDeleted { .. }
            | Self::DataFileEdited { .. } => "data_file",
            Self::ApplyAllBatchStarted { .. } | Self::ApplyAllBatchCompleted { .. } => {
                "apply_all_batch"
            }
            Self::StandingApplyArmed { .. }
            | Self::StandingApplyFired { .. }
            | Self::StandingApplyDropped { .. } => "standing_apply",
            Self::LaneBatchStarted { .. }
            | Self::LaneBatchLanded { .. }
            | Self::HardenCertificateCarried { .. }
            | Self::LaneBatchFailureRequested { .. }
            | Self::LaneBatchFailed { .. }
            | Self::LaneArrivalsHeld { .. }
            | Self::LaneArrivalsReleased { .. }
            | Self::RemoteChangesLanded { .. } => "landing_lane",
            Self::MainPushFailed { .. } => "main_push",
            Self::MainFollowRefused { .. } | Self::MainFollowed { .. } => "main_follow",
            Self::MainCheckStarted { .. }
            | Self::MainCheckRunLinked { .. }
            | Self::MainCheckPassed { .. }
            | Self::MainCheckRepairStarted { .. }
            | Self::MainCheckFailed { .. }
            | Self::MainCheckAbandoned { .. }
            | Self::MainCheckSuperseded { .. }
            | Self::MainCheckRedClosed { .. } => "main_check",
            Self::InternalFaultRecorded { .. }
            | Self::DiagnosticBundleSent { .. }
            | Self::EngineSupervisorRespawned { .. }
            | Self::FrontendUpdateDeferred { .. }
            | Self::FrontendUpdateStranded { .. }
            | Self::ServedFrontendAdvanced { .. }
            | Self::FrontendPreviewStarted { .. }
            | Self::FrontendPreviewStopped { .. }
            | Self::EngineBuildCompleted { .. }
            | Self::EngineVersionActivated { .. }
            | Self::ServedFrontendCommitChanged { .. }
            | Self::EngineBuildStateChanged { .. }
            | Self::EngineVersionStatusChanged { .. }
            | Self::FrontendRefreshStateChanged { .. }
            | Self::FrontendBuildWatchStateChanged { .. } => "engine",
            Self::EmailSent { .. } => "email",
            Self::ProxyModulesReloaded { .. } => "proxy_modules",
            Self::HandshakeScriptApproved { .. }
            | Self::HandshakeScriptScopeBound { .. }
            | Self::HandshakeScriptInjectsBound { .. } => "handshake_script",
            Self::SubThreadCardOpened { .. } => Self::SUB_THREAD_CARD_AGGREGATE,
            Self::ThreadQueued { .. }
            | Self::ThreadQueueAdmitted { .. }
            | Self::ThreadQueueDropped { .. }
            | Self::ThreadQueueCompleted { .. }
            | Self::ThreadQueueChanged {} => "thread_queue",
            Self::CapacityPolicyChanged { .. } => "capacity_policy",
        }
    }

    pub fn aggregate_id(&self) -> String {
        match self {
            Self::DiagnosticBundleReceived { bundle_id, .. } => bundle_id.to_string(),
            Self::SnapshotProgress { handle, .. }
            | Self::SnapshotRestoreRequested { handle, .. }
            | Self::WorkspaceRestored { handle, .. }
            | Self::SnapshotCreated { handle, .. }
            | Self::SnapshotCaptureFailed { handle, .. }
            | Self::SnapshotChecked { handle, .. }
            | Self::SnapshotKeepChanged { handle, .. }
            | Self::SnapshotDeleted { handle, .. } => handle.as_str().to_owned(),

            Self::NotificationCreated { id, .. } | Self::NotificationRead { id, .. } => id.clone(),
            Self::ArtifactImported { artifact_path, .. }
            | Self::ArtifactCreated { artifact_path, .. }
            | Self::ArtifactUpdated { artifact_path, .. }
            | Self::ArtifactDeleted { artifact_path, .. } => artifact_path.clone(),
            Self::RepositoryImported { destination, .. } => destination.clone(),
            Self::TriggerCreated { trigger_id, .. }
            | Self::TriggerUpdated { trigger_id, .. }
            | Self::TriggerDeleted { trigger_id, .. }
            | Self::TriggerEnabled { trigger_id, .. }
            | Self::TriggerDisabled { trigger_id, .. }
            | Self::TriggerExecuted { trigger_id, .. } => trigger_id.clone(),
            Self::TriggerCompleted { trigger_id, .. }
            | Self::TriggerSlotsSkipped { trigger_id, .. } => trigger_id.clone(),
            Self::TriggerGroupCreated { group_id, .. }
            | Self::TriggerGroupRenamed { group_id, .. }
            | Self::TriggerGroupReordered { group_id, .. }
            | Self::TriggerGroupDeleted { group_id, .. } => group_id.clone(),
            Self::EventHookArmed { slug, .. }
            | Self::EventHookDisarmed { slug, .. }
            | Self::EventHookRemoved { slug, .. }
            | Self::EventHookRefused { slug, .. } => slug.clone(),
            Self::AppCreated { app_id, .. }
            | Self::AppUpdated { app_id, .. }
            | Self::AppDeleted { app_id, .. }
            | Self::AppOpened { app_id, .. }
            | Self::AppUiRefreshRequested { app_id, .. } => app_id.clone(),
            Self::DashboardTileAdded { dashboard, .. }
            | Self::DashboardTileRemoved { dashboard, .. }
            | Self::DashboardArranged { dashboard, .. } => dashboard.as_str().to_string(),
            Self::KnowhowRead { knowhow_id, .. } => knowhow_id.clone(),
            Self::DomainEvent { event_type, .. } => event_type.clone(),
            Self::ChangeDiscarded { change_id } => change_id.clone(),
            Self::DeviceVisible { device_id } | Self::DeviceHidden { device_id } => {
                device_id.clone()
            }
            Self::PresenceCheck {
                notification_id, ..
            }
            | Self::NativePushRequested {
                notification_id, ..
            } => notification_id.to_string(),
            Self::NativePushDismissRequested {
                notification_id, ..
            } => notification_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "all".to_string()),
            Self::UsersChanged => "all".to_string(),
            Self::PairingRequestsChanged => "all".to_string(),
            Self::PluginInstalled { id, .. } => id.clone(),
            Self::PluginLocalChangesMerged { id, .. } => id.clone(),
            Self::PluginUninstalled { id, .. } => id.clone(),
            Self::PluginInstallCanceled { id, .. } => id.clone(),
            Self::PluginUninstallCanceled { id, .. } => id.clone(),
            Self::PluginMarketplaceRegistered { marketplace_id, .. }
            | Self::PluginMarketplaceRemoved { marketplace_id, .. } => marketplace_id.clone(),
            Self::ThreadComposeChanged { id, .. } => id.to_string(),
            Self::PinnedAppPinned {
                app_id, device_id, ..
            }
            | Self::PinnedAppUnpinned {
                app_id, device_id, ..
            } => {
                format!("{}@{}", app_id, device_id)
            }
            Self::DeviceRegistered { device_id, .. }
            | Self::DeviceRenamed { device_id, .. }
            | Self::DevicePushChanged { device_id, .. }
            | Self::DeviceDeleted { device_id, .. }
            | Self::DeviceHandedOver { device_id, .. } => device_id.clone(),
            Self::RepositoryAdded { repo_id, .. } | Self::RepositoryRemoved { repo_id, .. } => {
                repo_id.clone()
            }
            Self::CredentialCreated { service_name, .. }
            | Self::CredentialUpdated { service_name, .. }
            | Self::CredentialDeleted { service_name, .. }
            | Self::CredentialRevealed { service_name, .. }
            | Self::CredentialScopeInferred { service_name, .. } => service_name.clone(),
            Self::EnvironmentVariableSet { name, .. }
            | Self::EnvironmentVariableDeleted { name, .. } => name.clone(),
            Self::ReleaseNoticeResolved { notice_id, .. } => notice_id.clone(),
            Self::ModelCreated { id, .. }
            | Self::ModelUpdated { id, .. }
            | Self::ModelDeleted { id, .. } => id.clone(),
            Self::WebhookCreated { webhook_id, .. }
            | Self::WebhookUpdated { webhook_id, .. }
            | Self::WebhookDeleted { webhook_id, .. }
            | Self::WebhookIngressDegraded { webhook_id, .. }
            | Self::WebhookIngressRecovered { webhook_id, .. }
            | Self::WebhookIngressUnverified { webhook_id, .. }
            | Self::WebhookIngressVerified { webhook_id, .. }
            | Self::WebhookDeliveriesSleptThrough { webhook_id, .. }
            | Self::WebhookDeliveriesRefused { webhook_id, .. }
            | Self::WebhookDeliveriesRecovered { webhook_id, .. } => webhook_id.clone(),
            Self::McpServerRegistered { server_id, .. }
            | Self::McpServerUpdated { server_id, .. }
            | Self::McpServerDisabledToolsChanged { server_id, .. }
            | Self::McpServerRemoved { server_id, .. }
            | Self::McpServerStopped { server_id, .. }
            | Self::McpLaunchApproved { server_id, .. } => server_id.clone(),
            Self::PermissionGrantsChanged { grant_file, .. } => grant_file.file_name().to_string(),
            Self::OAuthAccountConnected { account_id, .. }
            | Self::OAuthAccountDeleted { account_id, .. } => account_id.clone(),
            Self::ClaudeGatewaySignedIn { issuer, .. }
            | Self::ClaudeGatewaySignedOut { issuer, .. }
            | Self::ClaudeGatewaySignInStarted { issuer, .. }
            | Self::ClaudeGatewaySignInCanceled { issuer, .. }
            | Self::ClaudeGatewaySignInFailed { issuer, .. }
            | Self::ClaudeGatewayManagedSettingsChanged { issuer, .. } => issuer.clone(),
            Self::ChatGptPlanSignedIn { .. }
            | Self::ChatGptPlanSignedOut { .. }
            | Self::ChatGptPlanSignInStarted { .. }
            | Self::ChatGptPlanSignInCanceled { .. }
            | Self::ChatGptPlanSignInFailed { .. }
            | Self::ChatGptPlanLimitReached { .. } => CHATGPT_PLAN_AGGREGATE_ID.to_string(),
            Self::DataFileWritten { path, .. }
            | Self::DataFileDeleted { path, .. }
            | Self::DataFileEdited { path, .. }
            | Self::HandshakeScriptApproved { path, .. }
            | Self::HandshakeScriptScopeBound { path, .. }
            | Self::HandshakeScriptInjectsBound { path, .. } => path.clone(),
            Self::ApplyAllBatchStarted { batch_id, .. }
            | Self::ApplyAllBatchCompleted { batch_id, .. } => batch_id.to_string(),
            Self::StandingApplyArmed { thread_id, .. }
            | Self::StandingApplyFired { thread_id, .. }
            | Self::StandingApplyDropped { thread_id, .. } => thread_id.to_string(),
            Self::LaneBatchStarted { batch_id, .. }
            | Self::LaneBatchLanded { batch_id, .. }
            | Self::HardenCertificateCarried { batch_id, .. }
            | Self::LaneBatchFailureRequested { batch_id, .. }
            | Self::LaneBatchFailed { batch_id, .. }
            | Self::LaneArrivalsHeld { batch_id }
            | Self::LaneArrivalsReleased { batch_id, .. } => batch_id.to_string(),
            Self::MainPushFailed { repo, .. }
            | Self::MainFollowRefused { repo, .. }
            | Self::MainFollowed { repo, .. }
            | Self::RemoteChangesLanded { repo, .. } => repo.clone(),
            Self::MainCheckStarted { check_id, .. }
            | Self::MainCheckRunLinked { check_id, .. }
            | Self::MainCheckPassed { check_id, .. }
            | Self::MainCheckRepairStarted { check_id, .. }
            | Self::MainCheckFailed { check_id, .. }
            | Self::MainCheckAbandoned { check_id, .. }
            | Self::MainCheckSuperseded { check_id, .. }
            | Self::MainCheckRedClosed { check_id, .. } => check_id.to_string(),
            Self::EngineSupervisorRespawned { supervisor_pid, .. } => supervisor_pid.to_string(),
            Self::FrontendPreviewStarted { thread_id, .. }
            | Self::FrontendPreviewStopped { thread_id, .. } => thread_id.to_string(),
            Self::ModelSpendCapReached { provider, .. }
            | Self::ModelSpendCapHoldEnded { provider, .. } => provider.clone(),
            Self::EmailSent { account, .. } => account.clone(),
            Self::OrdinarySubmissionAccepted { thread_id, .. } => thread_id.to_string(),
            Self::DraftSubmissionAccepted { receipt, .. } => receipt.thread_id.to_string(),
            Self::SubThreadCardOpened {
                ancestor_thread_id, ..
            } => ancestor_thread_id.to_string(),
            Self::ThreadQueued { entry_id, .. }
            | Self::ThreadQueueAdmitted { entry_id, .. }
            | Self::ThreadQueueDropped { entry_id, .. }
            | Self::ThreadQueueCompleted { entry_id } => entry_id.to_string(),
            _ => "global".into(),
        }
    }

    pub fn to_payload(&self) -> serde_json::Value {
        match self {
            // INVARIANT: TriggerExecuted is engine-driven and carries no actor field, so
            // its raw payload is the wire shape. The CRUD variants below DO
            // carry an `actor` (stamped by the HTTP handlers via
            // `require_user_actor`); they must merge it in so the persisted row
            // and SSE frame attribute the change — same contract as the
            // TriggerGroup* variants.
            Self::TriggerExecuted { payload, .. } => payload.clone(),
            Self::TriggerCreated { payload, actor, .. }
            | Self::TriggerUpdated { payload, actor, .. }
            | Self::TriggerDeleted { payload, actor, .. }
            | Self::TriggerEnabled { payload, actor, .. }
            | Self::TriggerDisabled { payload, actor, .. } => merge_actor(payload.clone(), actor),
            Self::TriggerGroupCreated { payload, actor, .. }
            | Self::TriggerGroupRenamed { payload, actor, .. }
            | Self::TriggerGroupReordered { payload, actor, .. }
            | Self::TriggerGroupDeleted { payload, actor, .. } => {
                merge_actor(payload.clone(), actor)
            }
            Self::DomainEvent { payload, actor, .. } => domain_payload(payload.clone(), actor),
            // INVARIANT: This value is bound straight into the `events` INSERT, so a
            // silent `Null` here persists an event with no content: no
            // projection, no SSE detail, no trigger match, and nothing in the
            // log saying why. Same reasoning as the actor drop in
            // `merge_actor` below.
            _ => match serde_json::to_value(self) {
                Ok(v) => v,
                Err(e) => {
                    crate::log_legacy!(
                        "[EventBus] WARNING: {} payload would not serialize ({}), persisting it empty",
                        self.event_type(),
                        e
                    );
                    serde_json::Value::Null
                }
            },
        }
    }
}

impl SystemEvent {
    pub fn from_stored(
        payload: serde_json::Value,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(serde_json::from_value(payload)?)
    }

    /// INVARIANT: The payload key holding who emitted an event. A domain event's payload
    /// is written by its caller, so this key is the one field in it the engine
    /// owns.
    pub const ACTOR_KEY: &'static str = "actor";
}

fn domain_payload(
    mut payload: serde_json::Value,
    actor: &Option<MessageOrigin>,
) -> serde_json::Value {
    if let Some(obj) = payload.as_object_mut() {
        obj.remove(SystemEvent::ACTOR_KEY);
    }
    merge_actor(payload, actor)
}

/// INVARIANT: Inject an `actor` key into the payload object when an actor is present.
/// No-op for non-object payloads (domain events with primitive/array payloads)
/// since `actor` is conventionally a top-level object field; callers that need
/// to attribute a non-object payload should wrap it themselves. The drop is
/// logged so it's visible in the engine log instead of silently swallowing
/// the audit attribution.
fn merge_actor(mut payload: serde_json::Value, actor: &Option<MessageOrigin>) -> serde_json::Value {
    if let Some(a) = actor {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                SystemEvent::ACTOR_KEY.to_string(),
                serde_json::to_value(a).unwrap_or(serde_json::Value::Null),
            );
        } else {
            crate::log_legacy!(
                "[EventBus] WARNING: DomainEvent actor dropped — payload is not a JSON object (got: {}). \
                 Wrap primitive/array payloads in an object so the actor can attach.",
                match &payload {
                    serde_json::Value::Null => "null",
                    serde_json::Value::Bool(_) => "bool",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::Object(_) => unreachable!(),
                }
            );
        }
    }
    payload
}

#[cfg(test)]
#[path = "event_bus_system_event_tests.rs"]
mod tests;
