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
    /// The user answered a *release notice*, by acting on it or by reading it.
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
    /// The background embedding-model loader moved on: a download progress
    /// frame, or a transition between downloading / loading / ready / waiting /
    /// failed. TRANSIENT (never persisted) and high-frequency during a cold
    /// first run, like `MemoryRebuildProgress` and `BackupProgress`.
    ///
    /// Fields mirror `memory::EmbeddingModelStatus` exactly, because
    /// `GET /api/v1/memory/embedding-model-status` serves the same shape as a
    /// snapshot: a client that loads mid-download must read what the stream
    /// would have told it. Pinned by
    /// `embedding_model_status_event_matches_the_rest_snapshot`.
    EmbeddingModelStatusChanged {
        model_id: String,
        load_state: crate::memory::EmbeddingModelLoadState,
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
    /// A backup run finished successfully. PERSISTED (unlike `BackupProgress`):
    /// the events table is the durable history of every backup run (start /
    /// finish / size), queried by `core::backup::load_recent_runs` and surfaced
    /// by the `get_backup_status` tool.
    BackupCompleted {
        filename: String,
        size_bytes: u64,
        /// When the backup pipeline started (RFC 3339).
        started_at: chrono::DateTime<chrono::Utc>,
        /// When it finished (RFC 3339). Duration = `finished_at - started_at`.
        finished_at: chrono::DateTime<chrono::Utc>,
    },
    /// A backup run failed. PERSISTED — see `BackupCompleted`.
    BackupFailed {
        error: String,
        /// When the backup pipeline started (RFC 3339).
        started_at: chrono::DateTime<chrono::Utc>,
        /// When it failed (RFC 3339).
        finished_at: chrono::DateTime<chrono::Utc>,
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
    /// App list-refresh hints. Broadcast-only (`is_persisted` = false), like
    /// `AppUiRefreshRequested`: the `appsList` is a disk-scan projection and
    /// these SSE signals keep it live; the durable record of an app change is
    /// the git commit (and `ChangeApplied` for coding-agent applies).
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
    /// A *frontend-only* Apply's in-process served-client advance was DEFERRED
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
    /// A *frontend-only* Apply rebuilt fine but the engine's served client can
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
        /// Absolute path of the `dist/` this engine serves from.
        served_dir: String,
        /// Whether that path lies inside a coding-agent worktree.
        served_in_worktree: bool,
        /// What the build-watch last said went wrong, when it said anything.
        ///
        /// Read from its `.build-watch/status.json`. A failing build is the
        /// other way an Apply strands, and until this field existed the message
        /// could only guess at "is the build-watch running?" while the answer
        /// sat in a log file. `None` when the status is missing, unreadable, or
        /// reports a healthy build.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_error: Option<String>,
        sent_at_ms: i64,
    },
    /// A dev engine advanced its boot-pinned served-frontend snapshot to the
    /// checkout-shared `dist/` after ANOTHER workspace's *frontend-only* Apply
    /// moved it — the applying engine advances only its OWN snapshot, so a peer
    /// workspace would otherwise silently serve a stale client with no badge (see
    /// `engine::frontend_refresh::spawn_served_frontend_sync`,
    /// `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`).
    /// INV-A-gated: only emitted when the running engine's source still matches
    /// HEAD (no engine version change pending), so the newer client is compatible
    /// with the running binary. Transient (never persisted — a pure UI signal like
    /// `FrontendUpdateDeferred`) and dev-only by construction
    /// (`spawn_served_frontend_sync` no-ops packaged / headless). The connected
    /// client re-runs `syncClientUpdateFromBuild` to surface the Refresh
    /// badge/toast. `sent_at_ms` is informational — the handler is idempotent and
    /// self-correcting, so no page-side freshness gate is needed.
    ServedFrontendAdvanced {
        sent_at_ms: i64,
    },
    /// The supervised Vite dev server showing a coding-agent worktree's frontend
    /// came up (`engine::frontend_preview`). Carries the **port**, never a URL:
    /// the same workspace is reached at `localhost` from the laptop and at a
    /// Tailscale name from the phone, and only the page knows which of those the
    /// user is on, so it composes the href from its own `location`. Transient
    /// (never persisted, a pure UI signal like `ServedFrontendAdvanced`) and
    /// dev-only by construction (`start_frontend_preview` refuses when packaged).
    FrontendPreviewStarted {
        thread_id: uuid::Uuid,
        port: u16,
        sent_at_ms: i64,
    },
    /// The frontend preview stopped: explicitly, because its worktree was
    /// reclaimed, or because another thread took the single slot. Transient and
    /// dev-only for the same reasons as `FrontendPreviewStarted`.
    FrontendPreviewStopped {
        thread_id: uuid::Uuid,
        sent_at_ms: i64,
    },
    /// The engine's dev background-rebuild `build_state` transitioned
    /// (`idle`|`building`|`ready`|`failed`) — emitted from `trigger_background_rebuild`
    /// at build start (`building`) and completion (`ready`/`failed`, latest
    /// generation only). A pure UI POKE so the connected client learns of a build
    /// over the live SSE stream instead of waiting on the throttled 4s
    /// version-status poll (which iOS suspends on a backgrounded PWA, so the
    /// transient `building` window was never seen — the spinner badge never
    /// showed). The frontend handler re-runs the authoritative `checkEngineVersion`
    /// GET rather than trusting `state` directly, so a stale/duplicate poke can
    /// only trigger a harmless re-check, never a false spin. Transient (never
    /// persisted — like `ServedFrontendAdvanced`) and dev-only by construction
    /// (`trigger_background_rebuild` no-ops packaged). `sent_at_ms` is
    /// informational — the handler is idempotent, so no page-side freshness gate.
    EngineBuildStateChanged {
        state: String,
        sent_at_ms: i64,
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
    DeviceVisible {
        device_id: String,
    },
    /// A device has no more visible Lucidos tabs (last one hidden / blurred /
    /// unloaded). Transient — removes the projection row.
    DeviceHidden {
        device_id: String,
    },
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
    /// SSE-only, never persisted. The desktop counterpart of the OS push:
    /// emitted on the *push-allowed* branch (the complement of
    /// [`Self::NotificationToastRequested`]) so a connected Tauri desktop app —
    /// which embeds a WKWebView and can NOT receive Web Push / service-worker
    /// pushes — renders a NATIVE macOS notification via
    /// `tauri-plugin-notification`. Broadcast: browser / PWA clients ignore it
    /// (they already get, or are getting, the real web push), and only a Tauri
    /// client that is not currently active acts on it. Because it lives on the
    /// same branch as the web-push fan-out and the opposite branch from the
    /// in-app toast, a device can never receive both a native banner and a
    /// toast for one notification — the engine's single `push_allowed` decision
    /// keeps them mutually exclusive (see `system-knowhow/notifications.md`
    /// §1, §4). Carries the same content as the toast event so the page renders
    /// without a re-fetch; `sent_at_ms` drives the page-side freshness gate.
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
        /// Defaults to `Tap::Modal` (open inbox) so omitting it stays safe.
        #[serde(default)]
        tap: Tap,
        sent_at_ms: i64,
    },
    /// SSE-only, never persisted. The cross-device counterpart of a *read*: when
    /// a notification is read on one device, the engine broadcasts this so a
    /// connected Tauri desktop app REMOVES the already-delivered native macOS
    /// banner(s) for it — `UNUserNotificationCenter.removeDeliveredNotifications`
    /// (single) / `removeAllDeliveredNotifications` (all). This is the macOS-only
    /// half of cross-device dismiss: the open web can't silently remove a Web
    /// Push banner (Safari revokes a subscription after 3 silent pushes), so
    /// browser / PWA pages ignore this event — see
    /// `docs/plans/2026-05-18-cross-device-notification-dismiss-design.md` and
    /// `system-knowhow/notifications.md` §4. `notification_id = None` means
    /// "dismiss all" (the mark-all-read path); `Some(id)` dismisses one.
    /// `sent_at_ms` drives the page-side freshness gate, which drops a late
    /// SSE-queue flush — bounding (not fully eliminating, since
    /// `removeAllDeliveredNotifications` is blunt) the window in which a delayed
    /// dismiss-all could clear a banner for a notification created after an
    /// all-read.
    NativePushDismissRequested {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notification_id: Option<Uuid>,
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
    /// An install met files the user had locally edited, and says what it did
    /// with each. `merged` kept their edit alongside upstream's. `conflicted`
    /// could not. `replaced` was never mergeable: a trigger projection, a
    /// binary, or the panel's keep control switched off. `restored` is a file
    /// the user had deleted that upstream still ships, so it came back.
    ///
    /// Everything in `merged` survives in the file. Everything in `conflicted`
    /// and `replaced` has a copy in `saved_paths`. Nothing in `restored` does,
    /// because a deletion has no content to save.
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
        saved_paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
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
    /// A plugin marketplace was registered, or an existing one re-registered
    /// under a new name. One variant covers both because registration is an
    /// **upsert**: `core::plugin_marketplaces::add_marketplace` keys on a hash
    /// of the canonical source, so re-registering the same source rewrites the
    /// existing entry's `name` (and its raw `source` string, which can differ
    /// by a `.git` suffix or owner/repo casing while hashing the same) instead
    /// of adding a second one. That is the rename path, and a `Renamed` variant
    /// would misdescribe the source-only case. Same shape as `RepositoryAdded`
    /// and `McpServerRegistered`, which are upserts for the same reason. The
    /// payload carries the resulting `name` + `source`, so a reader sees what
    /// the marketplace is now rather than what changed.
    ///
    /// Emitted from `engine::tools::plugins::marketplaces`, the single write
    /// path both the HTTP handler and the `plugins` tool go through, so the
    /// Plugins panel and Settings, Marketplaces refresh live from any origin.
    PluginMarketplaceRegistered {
        marketplace_id: String,
        name: String,
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A plugin marketplace was unregistered. Its already-installed plugins
    /// stay on disk (the installed list is a separate projection), they just
    /// stop appearing in the catalog scan.
    PluginMarketplaceRemoved {
        marketplace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
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
    /// One device's whole row moved to a new id, keeping its push subscription
    /// and its preferences. Emitted when a browser stops minting its own id and
    /// takes the *workspace gateway*'s instead. `device_id` is where it landed.
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
    /// A credential's plaintext was handed to a caller, through the Settings
    /// copy buttons or the edit form's prefill.
    ///
    /// Same secrecy contract as its siblings: the service and its type, never
    /// the value. What it adds is that a read is now on the record. The origin
    /// check in front of it is defense in depth, not a boundary (ADR 0117). So
    /// a reveal that should not have happened leaves a row somebody can find.
    ///
    /// `auth_type` is a plain `String` here, unlike `CredentialCreated`'s typed
    /// field. The reveal route reads it off the stored row as its wire
    /// spelling, and never parses it back.
    CredentialRevealed {
        service_name: String,
        auth_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A user-managed environment variable was created or updated (upsert).
    /// Unlike credentials, these are **not** secret — the `value` is carried so
    /// the settings UI can refresh live without an extra fetch, and it may
    /// appear in logs / the event store. That is the whole point of the feature.
    EnvironmentVariableSet {
        name: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A user-managed environment variable was removed.
    EnvironmentVariableDeleted {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A chat-model registry entry was created (user-added via Settings →
    /// Models). The id is the request value (e.g. `claude-fable-5`); `provider`
    /// is the backend that serves it (`vertex` / `anthropic` / `openai`).
    ModelCreated {
        id: String,
        label: String,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A model registry entry was edited (label/provider/sort/enabled, or a
    /// builtin toggled enabled/disabled). Drives the model-registry reload.
    ModelUpdated {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A user-added model registry entry was removed (builtins are not
    /// deletable, only disable-able).
    ModelDeleted {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A webhook was created. It opens a publicly reachable endpoint that emits
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
    /// A webhook's configuration changed. `enabled` is carried because turning
    /// one off is the thing a reader most often wants to date.
    WebhookUpdated {
        webhook_id: String,
        name: String,
        event_type: String,
        enabled: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A webhook was deleted, so its URL answers nothing from now on.
    WebhookDeleted {
        webhook_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An MCP server was registered, or an existing one re-registered with a
    /// new command / args / env. The `mcp_servers` table drives which external
    /// tools the agent can call, so a registration changes the agent's own tool
    /// surface: it belongs on the timeline even though no UI lists it yet.
    McpServerRegistered {
        server_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An MCP server's settings changed without re-registering it (today: the
    /// auto-approve flag, which decides whether its tool calls prompt).
    McpServerUpdated {
        server_id: String,
        auto_approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// Which of a server's tools are switched off changed. `disabled_tools` is
    /// the resulting set in full, by WIRE name, not a delta: the user picks a
    /// selection, and one event that states it is easier to read back than a
    /// pair of added/removed lists.
    ///
    /// Its own variant rather than a field on `McpServerUpdated`, which is an
    /// auto-approve change. A disabled tool leaves every request, so this moves
    /// the agent's tool surface the way `McpServerRemoved` does for a whole
    /// server.
    McpServerDisabledToolsChanged {
        server_id: String,
        disabled_tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An MCP server was unregistered and its tools left the agent's surface.
    McpServerRemoved {
        server_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// One of the three permission allowlists was rewritten wholesale, through
    /// its Settings editor.
    ///
    /// `patterns` is the resulting set in full, never a delta, on the model of
    /// `McpServerDisabledToolsChanged`. The point of the event is auditing a
    /// WIDENED grant after the fact, so the row has to say what the permission
    /// became. "It changed" answers nothing a month later.
    ///
    /// One variant with a typed `grant_file`, not three near-identical ones.
    /// `GrantFile` is already this codebase's single identity for the three
    /// lanes. Three variants would be one event written three times, drifting
    /// apart the way the three handlers just did. A subscriber that wants one
    /// lane scopes it with a `condition` on `grant_file`.
    ///
    /// Comments and blank lines are dropped: this records the grants, not the
    /// file's formatting.
    PermissionGrantsChanged {
        grant_file: crate::core::GrantFile,
        patterns: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An OAuth account was connected (or re-authorized with new scopes). The
    /// counterpart of `OAuthAccountDeleted`: without it the Accounts list only
    /// refreshed live on a disconnect, and connecting from one device left
    /// every other one showing a stale list until a reload.
    OAuthAccountConnected {
        account_id: String,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
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
    /// Compose state on a thread changed (text, images, or mode). Broadcast
    /// to SSE for cross-device sync; intentionally NOT persisted to the events
    /// table — the `thread_summaries` row holds the current state, and
    /// keystroke history isn't audit-worthy. Receivers reconcile via the
    /// `origin_device_id` echo check + "don't clobber my focused textarea"
    /// guard described in `docs/plans/2026-05-03-threads-as-drafts-design.md`.
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
        /// Per-draft dropdown selections (target/scope, coding agent, Lucidos
        /// model + reasoning, coding-agent model + reasoning) as a partial
        /// `ComposeSelectionOverride`-shaped object. `None` = the PUT didn't
        /// touch the selection (COALESCE-preserve); receivers hydrate their
        /// per-draft store from it under the same origin/focus guards as text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<serde_json::Value>,
        /// The thread's *compose epoch* after this change (`docs/glossary.md`):
        /// how many times a submission has consumed the thread's compose slot.
        /// Carried on every broadcast so a device always holds a value its next
        /// compose PUT can be fenced against, including the device whose own
        /// write is in flight. `#[serde(default)]` reads a pre-epoch payload as
        /// 0, which is also what a never-submitted thread stores.
        #[serde(default)]
        compose_epoch: i64,
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
    /// A background spawn entered the *Thread Queue* (admission control).
    /// Emitted for EVERY background spawn — entries that fit capacity are
    /// admitted immediately (a `ThreadQueueAdmitted` follows in the same
    /// breath), entries over capacity wait in the queue. `requeued: true`
    /// marks the boot sweep re-queuing an `admitted` entry whose work died
    /// with the previous engine process. Projection: `thread_queue` row
    /// upserted with status `'queued'`.
    ThreadQueued {
        entry_id: Uuid,
        /// Capacity bucket: `event-trigger` | `cron` | `sub-thread` | `coding-agent`.
        kind: crate::engine::thread_queue::ThreadQueueKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        /// Bound thread for sub-thread / coding-agent spawns (pre-allocated id).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        /// Human preview for the Thread Queue panel.
        summary: String,
        /// Full re-executable spawn request (`ThreadQueueRequest` JSON).
        request: serde_json::Value,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        requeued: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A Thread Queue entry was admitted — capacity allowed it (or the user
    /// clicked Run now; that path carries the `actor`). The entry's work is
    /// now executing; the `thread_queue` row flips to `'admitted'` and is the
    /// persisted active-session record until `ThreadQueueCompleted`.
    ThreadQueueAdmitted {
        entry_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A Thread Queue entry was dropped without running — user clicked Drop,
    /// the per-trigger queue cap overflowed (capacity policy `drop-oldest`),
    /// or the entry's trigger no longer exists. Projection: row deleted.
    ThreadQueueDropped {
        entry_id: Uuid,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// An admitted Thread Queue entry's work finished (any outcome — the
    /// thread's own terminal events carry success/failure). Frees the
    /// capacity slot; projection deletes the row.
    ThreadQueueCompleted {
        entry_id: Uuid,
    },
    /// Transient panel-refresh signal — broadcast, never persisted, no
    /// projection. Emitted when only the in-memory user-initiated occupants of
    /// the shared pool change (a user response admitted / queued / released);
    /// those are not persisted as `thread_queue` rows, so the panel refetches
    /// and merges them on this. Background changes already fire the persisted
    /// `ThreadQueue*` events.
    ThreadQueueChanged {},
    /// The *capacity policy* governing the Thread Queue changed. Persisted —
    /// the latest event IS the stored policy (reconstructed at boot); no
    /// separate config table.
    CapacityPolicyChanged {
        policy: crate::engine::thread_queue::CapacityPolicy,
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

    /// Wire-format `type` names the engine writes an `events` row for. One
    /// list, two entry points: [`Self::is_persisted`] answers for a constructed
    /// event, [`Self::is_persisted_type_name`] for a bare name.
    ///
    /// A persisted event is a durable fact, so it is also what an event wait
    /// and a trigger may subscribe to (ADR 0113). That is why the name-keyed
    /// form exists: validation holds a name, not an event.
    ///
    /// `DomainEvent` is deliberately absent. It is a transport variant, and its
    /// row is stored under the inner event type, never that literal name.
    pub const PERSISTED_TYPE_NAMES: &'static [&'static str] = &[
        "NotificationCreated",
        "PreferencesChanged",
        "ReleaseNoticeResolved",
        "ArtifactImported",
        "ArtifactCreated",
        "ArtifactUpdated",
        "ArtifactDeleted",
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
        "TriggerCompleted",
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
        "McpServerRegistered",
        "McpServerUpdated",
        "McpServerDisabledToolsChanged",
        "McpServerRemoved",
        "PermissionGrantsChanged",
        "OAuthAccountConnected",
        "OAuthAccountDeleted",
        "DataFileWritten",
        "DataFileDeleted",
        "DataFileEdited",
        "ApplyAllBatchStarted",
        "ApplyAllBatchCompleted",
        "EngineSupervisorRespawned",
        "EmailSent",
        "ProxyModulesReloaded",
        "ThreadQueued",
        "ThreadQueueAdmitted",
        "ThreadQueueDropped",
        "ThreadQueueCompleted",
        "CapacityPolicyChanged",
        "BackupCompleted",
        "BackupFailed",
    ];

    /// Whether this event writes a row to the `events` table.
    ///
    /// `DomainEvent` is the one variant whose answer depends on a field. Every
    /// other one is decided by its name, so both forms read one list.
    pub fn is_persisted(&self) -> bool {
        match self {
            Self::DomainEvent { transient, .. } => !transient,
            _ => Self::is_persisted_type_name(self.event_type()),
        }
    }

    /// [`Self::is_persisted`] for a bare wire name, which is all a validator or
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
            Self::ChangesUpdated { .. } => "ChangesUpdated",
            Self::BackupProgress { .. } => "BackupProgress",
            Self::BackupCompleted { .. } => "BackupCompleted",
            Self::BackupFailed { .. } => "BackupFailed",
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
            Self::FrontendUpdateDeferred { .. } => "FrontendUpdateDeferred",
            Self::FrontendUpdateStranded { .. } => "FrontendUpdateStranded",
            Self::ServedFrontendAdvanced { .. } => "ServedFrontendAdvanced",
            Self::FrontendPreviewStarted { .. } => "FrontendPreviewStarted",
            Self::FrontendPreviewStopped { .. } => "FrontendPreviewStopped",
            Self::EngineBuildStateChanged { .. } => "EngineBuildStateChanged",
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
            Self::NativePushRequested { .. } => "NativePushRequested",
            Self::NativePushDismissRequested { .. } => "NativePushDismissRequested",
            Self::PluginInstalled { .. } => "PluginInstalled",
            Self::PluginLocalChangesMerged { .. } => "PluginLocalChangesMerged",
            Self::PluginUninstalled { .. } => "PluginUninstalled",
            Self::PluginInstallCanceled { .. } => "PluginInstallCanceled",
            Self::PluginUninstallCanceled { .. } => "PluginUninstallCanceled",
            Self::PluginMarketplaceRegistered { .. } => "PluginMarketplaceRegistered",
            Self::PluginMarketplaceRemoved { .. } => "PluginMarketplaceRemoved",
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
            Self::McpServerRegistered { .. } => "McpServerRegistered",
            Self::McpServerUpdated { .. } => "McpServerUpdated",
            Self::McpServerDisabledToolsChanged { .. } => "McpServerDisabledToolsChanged",
            Self::McpServerRemoved { .. } => "McpServerRemoved",
            Self::PermissionGrantsChanged { .. } => "PermissionGrantsChanged",
            Self::OAuthAccountConnected { .. } => "OAuthAccountConnected",
            Self::OAuthAccountDeleted { .. } => "OAuthAccountDeleted",
            Self::DataFileWritten { .. } => "DataFileWritten",
            Self::DataFileDeleted { .. } => "DataFileDeleted",
            Self::DataFileEdited { .. } => "DataFileEdited",
            Self::ApplyAllBatchStarted { .. } => "ApplyAllBatchStarted",
            Self::ApplyAllBatchCompleted { .. } => "ApplyAllBatchCompleted",
            Self::EngineSupervisorRespawned { .. } => "EngineSupervisorRespawned",
            Self::EmailSent { .. } => "EmailSent",
            Self::ProxyModulesReloaded { .. } => "ProxyModulesReloaded",
            Self::ThreadQueued { .. } => "ThreadQueued",
            Self::ThreadQueueAdmitted { .. } => "ThreadQueueAdmitted",
            Self::ThreadQueueDropped { .. } => "ThreadQueueDropped",
            Self::ThreadQueueCompleted { .. } => "ThreadQueueCompleted",
            Self::ThreadQueueChanged {} => "ThreadQueueChanged",
            Self::CapacityPolicyChanged { .. } => "CapacityPolicyChanged",
        }
    }

    /// The name this event is filed under: its `events.event_type` column, and
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
        "ReleaseNoticeResolved",
        "MemoryRebuildProgress",
        "EmbeddingModelStatusChanged",
        "ChangesUpdated",
        "BackupProgress",
        "BackupCompleted",
        "BackupFailed",
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
        "FrontendUpdateDeferred",
        "FrontendUpdateStranded",
        "ServedFrontendAdvanced",
        "FrontendPreviewStarted",
        "FrontendPreviewStopped",
        "EngineBuildStateChanged",
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
        "NativePushRequested",
        "NativePushDismissRequested",
        "PluginInstalled",
        "PluginLocalChangesMerged",
        "PluginUninstalled",
        "PluginInstallCanceled",
        "PluginUninstallCanceled",
        "PluginMarketplaceRegistered",
        "PluginMarketplaceRemoved",
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
        "McpServerRegistered",
        "McpServerUpdated",
        "McpServerDisabledToolsChanged",
        "McpServerRemoved",
        "PermissionGrantsChanged",
        "OAuthAccountConnected",
        "OAuthAccountDeleted",
        "DataFileWritten",
        "DataFileDeleted",
        "DataFileEdited",
        "ApplyAllBatchStarted",
        "ApplyAllBatchCompleted",
        "EngineSupervisorRespawned",
        "EmailSent",
        "ProxyModulesReloaded",
        "ThreadQueued",
        "ThreadQueueAdmitted",
        "ThreadQueueDropped",
        "ThreadQueueCompleted",
        "ThreadQueueChanged",
        "CapacityPolicyChanged",
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
            | Self::NotificationToastRequested { .. }
            | Self::NativePushRequested { .. }
            | Self::NativePushDismissRequested { .. } => "notification",
            Self::PreferencesChanged { .. }
            | Self::LanguageSet { .. }
            | Self::TimezoneSet { .. } => "preference",
            Self::ReleaseNoticeResolved { .. } => "release_notice",
            Self::ChangesUpdated { .. } | Self::ChangeDiscarded { .. } => "change",
            Self::MemoryRebuildProgress { .. }
            | Self::EmbeddingModelStatusChanged { .. }
            | Self::BackupProgress { .. }
            | Self::BackupCompleted { .. }
            | Self::BackupFailed { .. }
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
            | Self::PluginLocalChangesMerged { .. }
            | Self::PluginUninstalled { .. }
            | Self::PluginInstallCanceled { .. }
            | Self::PluginUninstallCanceled { .. } => "plugin",
            // Its own aggregate, not "plugin": a marketplace is the source a
            // plugin can be installed FROM, and registering one installs
            // nothing.
            Self::PluginMarketplaceRegistered { .. } | Self::PluginMarketplaceRemoved { .. } => {
                "plugin_marketplace"
            }
            Self::ThreadComposeChanged { .. } => "thread",
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
            | Self::CredentialRevealed { .. } => "credential",
            Self::EnvironmentVariableSet { .. } | Self::EnvironmentVariableDeleted { .. } => {
                "environment_variable"
            }
            Self::ModelCreated { .. } | Self::ModelUpdated { .. } | Self::ModelDeleted { .. } => {
                "model"
            }
            Self::WebhookCreated { .. }
            | Self::WebhookUpdated { .. }
            | Self::WebhookDeleted { .. } => "webhook",
            Self::McpServerRegistered { .. }
            | Self::McpServerUpdated { .. }
            | Self::McpServerDisabledToolsChanged { .. }
            | Self::McpServerRemoved { .. } => "mcp_server",
            Self::PermissionGrantsChanged { .. } => "permission_grant",
            Self::OAuthAccountConnected { .. } | Self::OAuthAccountDeleted { .. } => {
                "oauth_account"
            }
            Self::DataFileWritten { .. }
            | Self::DataFileDeleted { .. }
            | Self::DataFileEdited { .. } => "data_file",
            Self::ApplyAllBatchStarted { .. } | Self::ApplyAllBatchCompleted { .. } => {
                "apply_all_batch"
            }
            Self::EngineSupervisorRespawned { .. }
            | Self::FrontendUpdateDeferred { .. }
            | Self::FrontendUpdateStranded { .. }
            | Self::ServedFrontendAdvanced { .. }
            | Self::FrontendPreviewStarted { .. }
            | Self::FrontendPreviewStopped { .. }
            | Self::EngineBuildStateChanged { .. } => "engine",
            Self::EmailSent { .. } => "email",
            Self::ProxyModulesReloaded { .. } => "proxy_modules",
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
            }
            | Self::NativePushRequested {
                notification_id, ..
            } => notification_id.to_string(),
            // `None` = dismiss-all; there is no single notification to key on.
            Self::NativePushDismissRequested {
                notification_id, ..
            } => notification_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "all".to_string()),
            // Raw manifest is nested one layer in — see `InstalledRecord` for the path.
            Self::PluginInstalled { manifest, .. } => manifest
                .get("manifest")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
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
                // Composite id: a single app can be pinned independently on
                // many devices, so the (app_id, device_id) pair is what
                // identifies the row, not just app_id.
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
            | Self::CredentialRevealed { service_name, .. } => service_name.clone(),
            Self::EnvironmentVariableSet { name, .. }
            | Self::EnvironmentVariableDeleted { name, .. } => name.clone(),
            Self::ReleaseNoticeResolved { notice_id, .. } => notice_id.clone(),
            Self::ModelCreated { id, .. }
            | Self::ModelUpdated { id, .. }
            | Self::ModelDeleted { id, .. } => id.clone(),
            Self::WebhookCreated { webhook_id, .. }
            | Self::WebhookUpdated { webhook_id, .. }
            | Self::WebhookDeleted { webhook_id, .. } => webhook_id.clone(),
            Self::McpServerRegistered { server_id, .. }
            | Self::McpServerUpdated { server_id, .. }
            | Self::McpServerDisabledToolsChanged { server_id, .. }
            | Self::McpServerRemoved { server_id, .. } => server_id.clone(),
            Self::PermissionGrantsChanged { grant_file, .. } => grant_file.file_name().to_string(),
            Self::OAuthAccountConnected { account_id, .. }
            | Self::OAuthAccountDeleted { account_id, .. } => account_id.clone(),
            Self::DataFileWritten { path, .. }
            | Self::DataFileDeleted { path, .. }
            | Self::DataFileEdited { path, .. } => path.clone(),
            Self::ApplyAllBatchStarted { batch_id, .. }
            | Self::ApplyAllBatchCompleted { batch_id, .. } => batch_id.to_string(),
            Self::EngineSupervisorRespawned { supervisor_pid, .. } => supervisor_pid.to_string(),
            // The preview is engine-level, but WHICH thread's worktree it shows
            // is its identity: one preview per thread, one slot at a time.
            Self::FrontendPreviewStarted { thread_id, .. }
            | Self::FrontendPreviewStopped { thread_id, .. } => thread_id.to_string(),
            Self::EmailSent { account, .. } => account.clone(),
            Self::ThreadQueued { entry_id, .. }
            | Self::ThreadQueueAdmitted { entry_id, .. }
            | Self::ThreadQueueDropped { entry_id, .. }
            | Self::ThreadQueueCompleted { entry_id } => entry_id.to_string(),
            _ => "global".into(),
        }
    }

    pub fn to_payload(&self) -> serde_json::Value {
        match self {
            // TriggerExecuted is engine-driven and carries no actor field, so
            // its raw payload is the wire shape. The CRUD variants below DO
            // carry an `actor` (stamped by the HTTP handlers via
            // `emit_user_system`); they must merge it in so the persisted row
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
