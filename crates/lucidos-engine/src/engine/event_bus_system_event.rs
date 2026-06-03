//! `SystemEvent` — the typed enum of every non-thread-scoped event that flows
//! through `EventBus`, plus its variant-discriminator match-tables
//! (`event_type`, `aggregate`, `aggregate_id`, `is_persisted`, `to_payload`).

use serde::Serialize;
use uuid::Uuid;

use crate::core::AuthType;
use crate::engine::thread_events::MessageOrigin;
use crate::scheduler::notifications::Tap;

/// System events — broadcast to SSE, selectively persisted.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum SystemEvent {
    NotificationCreated {
        id: String,
        title: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        /// Originating thread, when the notification has one. The notification
        /// modal renders an "Open thread" button when set so the user can jump
        /// to the conversation that produced the alert. Engine already passes
        /// the same value to push payloads for deep-linking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// Specific event UUID inside `thread_id` to scroll/pulse on land.
        /// Lets a notification deep-link to the exact event it was raised for
        /// (e.g. the `UserQuestionAsked` row the user should answer). Ignored
        /// when `thread_id` is unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        /// Where a tap should land. Defaults to `Tap::Modal` (open the inbox).
        /// `Tap::Navigate { to }` deep-links via the same router the
        /// `navigate_ui` LLM tool uses. See [`Tap`] for the wire shape.
        #[serde(default)]
        tap: Tap,
        /// Who emitted the notification. Set by HTTP handlers via
        /// `user_actor_resolved`; engine-internal sources (LLM tool, scheduler,
        /// worktree cleanup, backup) pass `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
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
    MemoryRebuildProgress {
        processed: usize,
        total: usize,
        percent: usize,
    },
    ChangesUpdated {
        pending: Vec<crate::core::changes::Change>,
        applied: Vec<crate::core::changes::Change>,
        total_pending: usize,
        restart_required: bool,
    },
    BackupProgress {
        phase: String,
        progress: usize,
        total: usize,
    },
    BackupCompleted {
        filename: String,
        size_bytes: u64,
    },
    BackupFailed {
        error: String,
    },
    /// Restore progress tick. Transient (SSE-only). Mirrors the authoritative
    /// `engine.restore_state` so a live client and a `GET /backup/restore-status`
    /// refetch render the identical phase/percent.
    RestoreProgress {
        workspace_name: String,
        phase: String,
        progress: usize,
        total: usize,
    },
    /// Restore finished — the new workspace is ready to start. Transient.
    RestoreCompleted {
        workspace_name: String,
        workspace_path: String,
    },
    /// Restore failed. Transient; `error` is the user-facing message.
    RestoreFailed {
        workspace_name: String,
        error: String,
    },
    RecoveryProgress {
        completed: usize,
        total: usize,
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
    /// A user-visible folder that organizes triggers in the panel was created.
    /// Pure label — has no schedule, runs no code, and does not coordinate trigger
    /// firing. Each trigger may belong to at most one group via `group_id`;
    /// ungrouped triggers render under an implicit "Ungrouped" section in the UI.
    /// Payload: `{ group_id, name, order }`.
    TriggerGroupCreated {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// The display name of a trigger group changed. Payload: `{ group_id, name }`.
    TriggerGroupRenamed {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// The sort position of a trigger group changed. Payload: `{ group_id, order }`.
    /// Batch reorders emit one event per moved group.
    TriggerGroupReordered {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A trigger group was deleted. Only emitted when the group has no members —
    /// the HTTP handler rejects the delete with 409 otherwise so the LLM (or user)
    /// can self-correct. Payload: `{ group_id }`.
    TriggerGroupDeleted {
        group_id: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// Domain event emitted via emit_event (e.g. SlideTextEdited, SleepImported).
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
        /// Who emitted the domain event. Stamped by the HTTP handler via
        /// `user_actor_resolved` so the UI can attribute the event to the
        /// originating device/workspace; engine-internal sources (LLM tool,
        /// scheduler) pass `None`. Merged into the wire payload by
        /// `to_payload` and `to_sse_json` so persisted rows and SSE frames
        /// carry the same `actor` key as every other actor-bearing event.
        #[serde(skip_serializing)]
        actor: Option<MessageOrigin>,
    },
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
        actor: Option<MessageOrigin>,
    },
    /// An app's iframe-bundled files changed in a way the open iframes need
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
    ArtifactCreated {
        artifact_path: String,
        commit: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    ArtifactUpdated {
        artifact_path: String,
        commit: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
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
    ChangeDiscarded {
        change_id: String,
    },
    /// A device has at least one visible Lucidos tab (any view, not scoped to
    /// a thread). Transient — projection lives in `device_presence`. Used by
    /// cross-device push suppression: if any device is visible we skip the
    /// push to ALL devices and rely on the active device's `NotificationCreated`
    /// SSE channel to render the in-app toast.
    DeviceVisible { device_id: String },
    /// A device has no more visible Lucidos tabs (last one hidden / blurred /
    /// unloaded). Transient — removes the projection row.
    DeviceHidden { device_id: String },
    /// SSE-only, never persisted. Pure pong trigger — see
    /// `system-knowhow/notifications.md` §3 (protocol, freshness gate). The
    /// page answers with a pong; the engine then decides whether to fan out
    /// the OS push OR instead emit [`Self::NotificationToastRequested`] (§4).
    /// It deliberately carries NO toast content: the in-app toast is no
    /// longer rendered on PresenceCheck receipt (that raced the push
    /// decision and produced a toast-plus-push duplicate on slow links).
    /// `sent_at_ms` lets the page drop a late ping (iOS PWA SSE-queue flush)
    /// so a stale pong doesn't arrive after the engine already decided.
    PresenceCheck {
        notification_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
        deadline_ms: u32,
        sent_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// SSE-only, never persisted. The engine emits this AFTER the
    /// PresenceCheck (§3) resolves to "an active device exists → suppress the
    /// OS push": it tells active pages to render the in-app toast (§4).
    /// Because it is emitted ONLY on the push-suppressed branch (and the OS
    /// push is emitted ONLY on the complementary branch), a device can never
    /// receive both a toast and a push for the same notification — they are
    /// mutually exclusive by the engine's single decision, not by a page-side
    /// timing race. Carries the toast content so the page renders without a
    /// re-fetch; `sent_at_ms` drives the page-side freshness gate (drop a
    /// toast that flushes late after an iOS PWA resume).
    NotificationToastRequested {
        notification_id: Uuid,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        /// Defaults to `Tap::Modal` (open inbox) so omitting it stays safe.
        #[serde(default)]
        tap: Tap,
        sent_at_ms: i64,
    },
    /// A plugin was installed (or updated — overwrite=true reuses this variant).
    /// `manifest` carries the full parsed manifest so future fields are additive.
    /// `files` are paths under `data/` so a future tracked-uninstall can derive
    /// ownership without a schema change.
    PluginInstalled {
        manifest: serde_json::Value,
        files: Vec<String>,
        installed_at: String,
        source_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A plugin was uninstalled. From v2 onwards the engine actually deletes
    /// the recorded files from `data/` (gated by the user's confirm in the
    /// uninstall panel). `files` is the full list recorded at install time,
    /// `files_deleted` is the subset actually removed this turn, and
    /// `files_missing` is the subset that had already been removed by hand
    /// before this uninstall fired. The two new fields default to empty so
    /// old DB rows from the v1 guide-only flow deserialize unchanged.
    PluginUninstalled {
        id: String,
        version: String,
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files_deleted: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files_missing: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A pending plugin install (raised by `install_plugin` / `update_plugin`)
    /// was canceled by the user from the install panel. Audit trail only —
    /// no file writes happen on cancel; the staged temp dir is dropped.
    PluginInstallCanceled {
        id: String,
        version: String,
        source: String,
        source_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A pending plugin uninstall was canceled from the uninstall panel.
    /// Audit trail only — no files are touched on cancel; the pending entry
    /// is just dropped.
    PluginUninstallCanceled {
        id: String,
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// Compose state on a thread changed (text, images, or mode). Broadcast
    /// to SSE for cross-device sync; intentionally NOT persisted to the events
    /// table — the `thread_summaries` row holds the current state, and
    /// keystroke history isn't audit-worthy. Receivers reconcile via the
    /// `origin_device_id` echo check + "don't clobber my focused textarea"
    /// guard described in `docs/plans/2026-05-03-threads-as-drafts-design.md`.
    /// A device pinned an app to its home / dock surface. Audit-worthy because
    /// the pinned set is what powers the app launcher on that device.
    PinnedAppPinned {
        app_id: String,
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A device unpinned an app. Symmetric to `PinnedAppPinned`.
    PinnedAppUnpinned {
        app_id: String,
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A device registered with the engine for the first time. Persisted so
    /// the timeline shows when each device became part of the workspace; the
    /// `devices` table is the projection.
    DeviceRegistered {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A device was renamed by the user. `name: None` clears back to the
    /// `device-<short>` fallback.
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
    /// A device row was deleted (typically the user revoked it from settings).
    DeviceDeleted {
        device_id: String,
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
        actor: Option<MessageOrigin>,
    },
    /// A credential entry was created. **Never** carry the auth value here —
    /// the payload reaches every SSE subscriber on every connected device.
    /// The service identifier is the only public field.
    CredentialCreated {
        service_name: String,
        auth_type: AuthType,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A credential's auth value was updated. Same secrecy contract as
    /// `CredentialCreated` — only the service id is broadcast.
    CredentialUpdated {
        service_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A credential entry was removed.
    CredentialDeleted {
        service_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An OAuth account row was removed (revoke / delete from settings).
    OAuthAccountDeleted {
        account_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A `PUT /api/v1/data/*path` write committed (file created or replaced).
    /// `commit` is the resulting git sha when the path lives under `artifacts/`
    /// (manager-committed); empty for non-artifact paths that bypass the
    /// artifact-manager commit dance.
    DataFileWritten {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A `DELETE /api/v1/data/*path` removed a file.
    DataFileDeleted {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A `POST /api/v1/data/edit` ran one or more in-place operations
    /// (JSON-path or text find/replace). `operations_count` records how many
    /// op-blocks were applied; the per-op detail isn't broadcast because the
    /// values may carry user data we don't want to fan out to every SSE
    /// subscriber.
    DataFileEdited {
        path: String,
        operations_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ThreadComposeChanged {
        id: Uuid,
        text: String,
        /// Content-addressed sha256 hashes of compose-draft image blobs.
        /// Cross-device sync transmits ~80 bytes per attached image instead
        /// of inflating each base64 payload over SSE on every keystroke.
        #[serde(default)]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_device_id: Option<String>,
    },
    /// Apply All started. Persists the full list of change IDs so the
    /// engine can resume the batch after a mid-flight conflict or an engine
    /// restart — the driver looks up the in-memory `ApplyAllRegistry`
    /// (recovered from this event on startup) when each member resolves.
    ApplyAllBatchStarted {
        /// Engine-assigned batch identifier — used as `aggregate_id` so the
        /// projection groups all events for one batch together.
        batch_id: Uuid,
        /// Change IDs to apply, in pending-list order. The driver picks
        /// `next_pending` from this list.
        change_ids: Vec<Uuid>,
        /// Who clicked Apply All. Stamps each per-change `apply_change` the
        /// driver makes so the resulting `ChangeApplied` chip reads as the
        /// user, not "Lucidos Engine".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// Every member of an `ApplyAllBatchStarted` has resolved (applied or
    /// failed). Closes the batch so projections and UI know the run is
    /// over. Emitted by the driver only — the canonical signal that batch
    /// state can be dropped from memory.
    ApplyAllBatchCompleted {
        batch_id: Uuid,
        applied: Vec<Uuid>,
        /// Per-change failures in the order they landed. Empty when the
        /// whole batch succeeded.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failed: Vec<crate::engine::apply_all_batches::ApplyFailure>,
    },
    /// Bash supervisor (`scripts/lib/engine_supervisor.sh`) wrapped an
    /// engine instance that died with a non-graceful exit code (anything
    /// other than 0 / 130 / 138) and is spawning its replacement. The
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
    /// Outbound email was sent successfully via SMTP. Carries only
    /// envelope metadata (account, recipients, subject, attachment
    /// count) — the message body is user data and stays out of the
    /// audit trail. Emitted by the `/api/v1/email/send` HTTP handler so
    /// the timeline records who sent what, when.
    EmailSent {
        /// Email account that performed the send (looked up in `email_accounts`).
        account: String,
        /// Primary recipients.
        to: Vec<String>,
        /// CC recipients. Empty when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cc: Vec<String>,
        /// BCC recipients. Empty when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bcc: Vec<String>,
        /// Subject line. Body intentionally omitted (user data).
        subject: String,
        /// How many files were attached. Body and contents intentionally omitted.
        attachment_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// The on-disk WASM auth-modules directory was re-scanned and the
    /// engine's compiled-module map was swapped atomically. Persisted
    /// because the reload changes runtime behavior of every subsequent
    /// proxy call — a post-mortem of a suddenly-broken sign-handshake
    /// reads this row to see when the surface changed and to what.
    ProxyModulesReloaded {
        /// Number of modules now loaded after the swap.
        count: usize,
        /// Sorted list of module names now loaded.
        names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
}

impl SystemEvent {
    /// Build an ArtifactCreated or ArtifactUpdated event based on whether the file existed.
    pub fn artifact_change(
        file_exists: bool,
        artifact_path: String,
        commit: String,
        source: Option<String>,
    ) -> Self {
        if file_exists {
            Self::ArtifactUpdated {
                artifact_path,
                commit,
                source,
            }
        } else {
            Self::ArtifactCreated {
                artifact_path,
                commit,
                source,
            }
        }
    }

    pub fn is_persisted(&self) -> bool {
        matches!(
            self,
            |Self::NotificationCreated { .. }| Self::PreferencesChanged { .. }
                | Self::ArtifactImported { .. }
                | Self::ArtifactCreated { .. }
                | Self::ArtifactUpdated { .. }
                | Self::ArtifactDeleted { .. }
                | Self::RepositoryImported { .. }
                | Self::TriggerCreated { .. }
                | Self::TriggerUpdated { .. }
                | Self::TriggerDeleted { .. }
                | Self::TriggerEnabled { .. }
                | Self::TriggerDisabled { .. }
                | Self::TriggerExecuted { .. }
                | Self::TriggerGroupCreated { .. }
                | Self::TriggerGroupRenamed { .. }
                | Self::TriggerGroupReordered { .. }
                | Self::TriggerGroupDeleted { .. }
                | Self::TriggerCompleted { .. }
                | Self::LanguageSet { .. }
                | Self::TimezoneSet { .. }
                | Self::ChangeDiscarded { .. }
                | Self::DomainEvent {
                    transient: false,
                    ..
                }
                | Self::PluginInstalled { .. }
                | Self::PluginUninstalled { .. }
                | Self::PluginInstallCanceled { .. }
                | Self::PluginUninstallCanceled { .. }
                | Self::PinnedAppPinned { .. }
                | Self::PinnedAppUnpinned { .. }
                | Self::DeviceRegistered { .. }
                | Self::DeviceRenamed { .. }
                | Self::DevicePushChanged { .. }
                | Self::DeviceDeleted { .. }
                | Self::RepositoryAdded { .. }
                | Self::RepositoryRemoved { .. }
                | Self::CredentialCreated { .. }
                | Self::CredentialUpdated { .. }
                | Self::CredentialDeleted { .. }
                | Self::OAuthAccountDeleted { .. }
                | Self::DataFileWritten { .. }
                | Self::DataFileDeleted { .. }
                | Self::DataFileEdited { .. }
                | Self::ApplyAllBatchStarted { .. }
                | Self::ApplyAllBatchCompleted { .. }
                | Self::EngineSupervisorRespawned { .. }
                | Self::EmailSent { .. }
                | Self::ProxyModulesReloaded { .. }
        )
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            Self::NotificationCreated { .. } => "NotificationCreated",
            Self::NotificationRead { .. } => "NotificationRead",
            Self::NotificationsAllRead { .. } => "NotificationsAllRead",
            Self::PreferencesChanged { .. } => "PreferencesChanged",
            Self::MemoryRebuildProgress { .. } => "MemoryRebuildProgress",
            Self::ChangesUpdated { .. } => "ChangesUpdated",
            Self::BackupProgress { .. } => "BackupProgress",
            Self::BackupCompleted { .. } => "BackupCompleted",
            Self::BackupFailed { .. } => "BackupFailed",
            Self::RestoreProgress { .. } => "RestoreProgress",
            Self::RestoreCompleted { .. } => "RestoreCompleted",
            Self::RestoreFailed { .. } => "RestoreFailed",
            Self::RecoveryProgress { .. } => "RecoveryProgress",
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
            Self::TriggerGroupReordered { .. } => "TriggerGroupReordered",
            Self::TriggerGroupDeleted { .. } => "TriggerGroupDeleted",
            Self::AppCreated { .. } => "AppCreated",
            Self::AppUpdated { .. } => "AppUpdated",
            Self::AppDeleted { .. } => "AppDeleted",
            Self::AppUiRefreshRequested { .. } => "AppUiRefreshRequested",
            Self::DomainEvent { .. } => "DomainEvent",
            Self::ArtifactCreated { .. } => "ArtifactCreated",
            Self::ArtifactUpdated { .. } => "ArtifactUpdated",
            Self::ArtifactDeleted { .. } => "ArtifactDeleted",
            Self::LanguageSet { .. } => "LanguageSet",
            Self::TimezoneSet { .. } => "TimezoneSet",
            Self::RepositoryImported { .. } => "RepositoryImported",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::DeviceVisible { .. } => "DeviceVisible",
            Self::DeviceHidden { .. } => "DeviceHidden",
            Self::PresenceCheck { .. } => "PresenceCheck",
            Self::NotificationToastRequested { .. } => "NotificationToastRequested",
            Self::PluginInstalled { .. } => "PluginInstalled",
            Self::PluginUninstalled { .. } => "PluginUninstalled",
            Self::PluginInstallCanceled { .. } => "PluginInstallCanceled",
            Self::PluginUninstallCanceled { .. } => "PluginUninstallCanceled",
            Self::ThreadComposeChanged { .. } => "ThreadComposeChanged",
            Self::PinnedAppPinned { .. } => "PinnedAppPinned",
            Self::PinnedAppUnpinned { .. } => "PinnedAppUnpinned",
            Self::DeviceRegistered { .. } => "DeviceRegistered",
            Self::DeviceRenamed { .. } => "DeviceRenamed",
            Self::DevicePushChanged { .. } => "DevicePushChanged",
            Self::DeviceDeleted { .. } => "DeviceDeleted",
            Self::RepositoryAdded { .. } => "RepositoryAdded",
            Self::RepositoryRemoved { .. } => "RepositoryRemoved",
            Self::CredentialCreated { .. } => "CredentialCreated",
            Self::CredentialUpdated { .. } => "CredentialUpdated",
            Self::CredentialDeleted { .. } => "CredentialDeleted",
            Self::OAuthAccountDeleted { .. } => "OAuthAccountDeleted",
            Self::DataFileWritten { .. } => "DataFileWritten",
            Self::DataFileDeleted { .. } => "DataFileDeleted",
            Self::DataFileEdited { .. } => "DataFileEdited",
            Self::ApplyAllBatchStarted { .. } => "ApplyAllBatchStarted",
            Self::ApplyAllBatchCompleted { .. } => "ApplyAllBatchCompleted",
            Self::EngineSupervisorRespawned { .. } => "EngineSupervisorRespawned",
            Self::EmailSent { .. } => "EmailSent",
            Self::ProxyModulesReloaded { .. } => "ProxyModulesReloaded",
        }
    }

    /// Wire-format `type` names that the engine emits as system frames
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
        "MemoryRebuildProgress",
        "ChangesUpdated",
        "BackupProgress",
        "BackupCompleted",
        "BackupFailed",
        "RestoreProgress",
        "RestoreCompleted",
        "RestoreFailed",
        "RecoveryProgress",
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
        "AppCreated",
        "AppUpdated",
        "AppDeleted",
        "AppUiRefreshRequested",
        "DomainEvent",
        "ArtifactCreated",
        "ArtifactUpdated",
        "ArtifactDeleted",
        "LanguageSet",
        "TimezoneSet",
        "RepositoryImported",
        "TriggerCompleted",
        "ChangeDiscarded",
        "DeviceVisible",
        "DeviceHidden",
        "PresenceCheck",
        "NotificationToastRequested",
        "PluginInstalled",
        "PluginUninstalled",
        "PluginInstallCanceled",
        "PluginUninstallCanceled",
        "ThreadComposeChanged",
        "PinnedAppPinned",
        "PinnedAppUnpinned",
        "DeviceRegistered",
        "DeviceRenamed",
        "DevicePushChanged",
        "DeviceDeleted",
        "RepositoryAdded",
        "RepositoryRemoved",
        "CredentialCreated",
        "CredentialUpdated",
        "CredentialDeleted",
        "OAuthAccountDeleted",
        "DataFileWritten",
        "DataFileDeleted",
        "DataFileEdited",
        "ApplyAllBatchStarted",
        "ApplyAllBatchCompleted",
        "EngineSupervisorRespawned",
        "EmailSent",
        "ProxyModulesReloaded",
        "ThreadEvent",
    ];

    pub fn is_reserved_type_name(name: &str) -> bool {
        Self::RESERVED_TYPE_NAMES.contains(&name)
    }

    pub fn aggregate(&self) -> &str {
        match self {
            Self::NotificationCreated { .. }
            | Self::NotificationRead { .. }
            | Self::NotificationsAllRead { .. }
            | Self::NotificationToastRequested { .. } => "notification",
            Self::PreferencesChanged { .. }
            | Self::LanguageSet { .. }
            | Self::TimezoneSet { .. } => "preference",
            Self::ChangesUpdated { .. } | Self::ChangeDiscarded { .. } => "change",
            Self::MemoryRebuildProgress { .. }
            | Self::BackupProgress { .. }
            | Self::BackupCompleted { .. }
            | Self::BackupFailed { .. }
            | Self::RestoreProgress { .. }
            | Self::RestoreCompleted { .. }
            | Self::RestoreFailed { .. }
            | Self::RecoveryProgress { .. }
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
            | Self::TriggerCompleted { .. } => "trigger",
            Self::TriggerGroupCreated { .. }
            | Self::TriggerGroupRenamed { .. }
            | Self::TriggerGroupReordered { .. }
            | Self::TriggerGroupDeleted { .. } => "trigger_group",
            Self::AppCreated { .. }
            | Self::AppUpdated { .. }
            | Self::AppDeleted { .. }
            | Self::AppUiRefreshRequested { .. } => "app",
            Self::DomainEvent { .. } => "domain",
            Self::DeviceVisible { .. } | Self::DeviceHidden { .. } => "device_presence",
            Self::PresenceCheck { .. } => "presence",
            Self::PluginInstalled { .. }
            | Self::PluginUninstalled { .. }
            | Self::PluginInstallCanceled { .. }
            | Self::PluginUninstallCanceled { .. } => "plugin",
            Self::ThreadComposeChanged { .. } => "thread",
            Self::PinnedAppPinned { .. } | Self::PinnedAppUnpinned { .. } => "pinned_app",
            Self::DeviceRegistered { .. }
            | Self::DeviceRenamed { .. }
            | Self::DevicePushChanged { .. }
            | Self::DeviceDeleted { .. } => "device",
            Self::RepositoryAdded { .. } | Self::RepositoryRemoved { .. } => "repository",
            Self::CredentialCreated { .. }
            | Self::CredentialUpdated { .. }
            | Self::CredentialDeleted { .. } => "credential",
            Self::OAuthAccountDeleted { .. } => "oauth_account",
            Self::DataFileWritten { .. }
            | Self::DataFileDeleted { .. }
            | Self::DataFileEdited { .. } => "data_file",
            Self::ApplyAllBatchStarted { .. } | Self::ApplyAllBatchCompleted { .. } => {
                "apply_all_batch"
            }
            Self::EngineSupervisorRespawned { .. } => "engine",
            Self::EmailSent { .. } => "email",
            Self::ProxyModulesReloaded { .. } => "proxy_modules",
        }
    }

    pub fn aggregate_id(&self) -> String {
        match self {
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
            Self::TriggerCompleted { trigger_id, .. } => trigger_id.clone(),
            Self::TriggerGroupCreated { group_id, .. }
            | Self::TriggerGroupRenamed { group_id, .. }
            | Self::TriggerGroupReordered { group_id, .. }
            | Self::TriggerGroupDeleted { group_id, .. } => group_id.clone(),
            Self::AppCreated { app_id, .. }
            | Self::AppUpdated { app_id, .. }
            | Self::AppDeleted { app_id, .. }
            | Self::AppUiRefreshRequested { app_id, .. } => app_id.clone(),
            Self::DomainEvent { event_type, .. } => event_type.clone(),
            Self::ChangeDiscarded { change_id } => change_id.clone(),
            Self::DeviceVisible { device_id } | Self::DeviceHidden { device_id } => {
                device_id.clone()
            }
            Self::PresenceCheck {
                notification_id, ..
            }
            | Self::NotificationToastRequested {
                notification_id, ..
            } => notification_id.to_string(),
            // Raw manifest is nested one layer in — see `InstalledRecord` for the path.
            Self::PluginInstalled { manifest, .. } => manifest
                .get("manifest")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            Self::PluginUninstalled { id, .. } => id.clone(),
            Self::PluginInstallCanceled { id, .. } => id.clone(),
            Self::PluginUninstallCanceled { id, .. } => id.clone(),
            Self::ThreadComposeChanged { id, .. } => id.to_string(),
            Self::PinnedAppPinned {
                app_id, device_id, ..
            }
            | Self::PinnedAppUnpinned {
                app_id, device_id, ..
            } => {
                // Composite id: a single app can be pinned independently on
                // many devices, so the (app_id, device_id) pair is what
                // identifies the row, not just app_id.
                format!("{}@{}", app_id, device_id)
            }
            Self::DeviceRegistered { device_id, .. }
            | Self::DeviceRenamed { device_id, .. }
            | Self::DevicePushChanged { device_id, .. }
            | Self::DeviceDeleted { device_id, .. } => device_id.clone(),
            Self::RepositoryAdded { repo_id, .. } | Self::RepositoryRemoved { repo_id, .. } => {
                repo_id.clone()
            }
            Self::CredentialCreated { service_name, .. }
            | Self::CredentialUpdated { service_name, .. }
            | Self::CredentialDeleted { service_name, .. } => service_name.clone(),
            Self::OAuthAccountDeleted { account_id, .. } => account_id.clone(),
            Self::DataFileWritten { path, .. }
            | Self::DataFileDeleted { path, .. }
            | Self::DataFileEdited { path, .. } => path.clone(),
            Self::ApplyAllBatchStarted { batch_id, .. }
            | Self::ApplyAllBatchCompleted { batch_id, .. } => batch_id.to_string(),
            Self::EngineSupervisorRespawned { supervisor_pid, .. } => supervisor_pid.to_string(),
            Self::EmailSent { account, .. } => account.clone(),
            _ => "global".into(),
        }
    }

    pub fn to_payload(&self) -> serde_json::Value {
        match self {
            Self::TriggerCreated { payload, .. }
            | Self::TriggerUpdated { payload, .. }
            | Self::TriggerDeleted { payload, .. }
            | Self::TriggerEnabled { payload, .. }
            | Self::TriggerDisabled { payload, .. }
            | Self::TriggerExecuted { payload, .. } => payload.clone(),
            Self::TriggerGroupCreated { payload, actor, .. }
            | Self::TriggerGroupRenamed { payload, actor, .. }
            | Self::TriggerGroupReordered { payload, actor, .. }
            | Self::TriggerGroupDeleted { payload, actor, .. } => {
                merge_actor(payload.clone(), actor)
            }
            Self::DomainEvent { payload, actor, .. } => merge_actor(payload.clone(), actor),
            _ => serde_json::to_value(self).unwrap_or_default(),
        }
    }
}

/// Inject an `actor` key into the payload object when an actor is present.
/// No-op for non-object payloads (domain events with primitive/array payloads)
/// since `actor` is conventionally a top-level object field; callers that need
/// to attribute a non-object payload should wrap it themselves. The drop is
/// logged so it's visible in the engine log instead of silently swallowing
/// the audit attribution.
fn merge_actor(mut payload: serde_json::Value, actor: &Option<MessageOrigin>) -> serde_json::Value {
    if let Some(a) = actor {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "actor".to_string(),
                serde_json::to_value(a).unwrap_or(serde_json::Value::Null),
            );
        } else {
            crate::log!(
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
