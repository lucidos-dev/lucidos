//! `SystemEvent` — the typed enum of every non-thread-scoped event that flows
//! through `EventBus`, plus its variant-discriminator match-tables
//! (`event_type`, `aggregate`, `aggregate_id`, `is_persisted`, `to_payload`).

use serde::Serialize;
use uuid::Uuid;

use crate::engine::thread_events::MessageOrigin;

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
    /// A device started focusing on a thread. Transient — projection lives in
    /// the `thread_presence` table. Used by notification suppression so a
    /// thread-scoped notification doesn't buzz the device already viewing it.
    ThreadFocused {
        thread_id: Uuid,
        device_id: String,
    },
    /// A device stopped focusing on a thread (visibility hidden, blur, switch,
    /// or unload). Transient — removes the projection row.
    ThreadUnfocused {
        thread_id: Uuid,
        device_id: String,
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
            Self::RecoveryProgress { .. } => "RecoveryProgress",
            Self::Toast { .. } => "Toast",
            Self::ArtifactImported { .. } => "ArtifactImported",
            Self::TriggerCreated { .. } => "TriggerCreated",
            Self::TriggerUpdated { .. } => "TriggerUpdated",
            Self::TriggerDeleted { .. } => "TriggerDeleted",
            Self::TriggerEnabled { .. } => "TriggerEnabled",
            Self::TriggerDisabled { .. } => "TriggerDisabled",
            Self::TriggerExecuted { .. } => "TriggerExecuted",
            Self::AppCreated { .. } => "AppCreated",
            Self::AppUpdated { .. } => "AppUpdated",
            Self::AppDeleted { .. } => "AppDeleted",
            Self::DomainEvent { .. } => "DomainEvent",
            Self::ArtifactCreated { .. } => "ArtifactCreated",
            Self::ArtifactUpdated { .. } => "ArtifactUpdated",
            Self::ArtifactDeleted { .. } => "ArtifactDeleted",
            Self::LanguageSet { .. } => "LanguageSet",
            Self::TimezoneSet { .. } => "TimezoneSet",
            Self::RepositoryImported { .. } => "RepositoryImported",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::ThreadFocused { .. } => "ThreadFocused",
            Self::ThreadUnfocused { .. } => "ThreadUnfocused",
            Self::PluginInstalled { .. } => "PluginInstalled",
            Self::PluginUninstalled { .. } => "PluginUninstalled",
            Self::PluginInstallCanceled { .. } => "PluginInstallCanceled",
            Self::PluginUninstallCanceled { .. } => "PluginUninstallCanceled",
            Self::ThreadComposeChanged { .. } => "ThreadComposeChanged",
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
        "RecoveryProgress",
        "Toast",
        "ArtifactImported",
        "TriggerCreated",
        "TriggerUpdated",
        "TriggerDeleted",
        "TriggerEnabled",
        "TriggerDisabled",
        "TriggerExecuted",
        "AppCreated",
        "AppUpdated",
        "AppDeleted",
        "DomainEvent",
        "ArtifactCreated",
        "ArtifactUpdated",
        "ArtifactDeleted",
        "LanguageSet",
        "TimezoneSet",
        "RepositoryImported",
        "TriggerCompleted",
        "ChangeDiscarded",
        "ThreadFocused",
        "ThreadUnfocused",
        "PluginInstalled",
        "PluginUninstalled",
        "PluginInstallCanceled",
        "PluginUninstallCanceled",
        "ThreadComposeChanged",
        "ThreadEvent",
    ];

    pub fn is_reserved_type_name(name: &str) -> bool {
        Self::RESERVED_TYPE_NAMES.contains(&name)
    }

    pub fn aggregate(&self) -> &str {
        match self {
            Self::NotificationCreated { .. }
            | Self::NotificationRead { .. }
            | Self::NotificationsAllRead { .. } => "notification",
            Self::PreferencesChanged { .. }
            | Self::LanguageSet { .. }
            | Self::TimezoneSet { .. } => "preference",
            Self::ChangesUpdated { .. } | Self::ChangeDiscarded { .. } => "change",
            Self::MemoryRebuildProgress { .. }
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
            Self::AppCreated { .. } | Self::AppUpdated { .. } | Self::AppDeleted { .. } => "app",
            Self::DomainEvent { .. } => "domain",
            Self::ThreadFocused { .. } | Self::ThreadUnfocused { .. } => "presence",
            Self::PluginInstalled { .. }
            | Self::PluginUninstalled { .. }
            | Self::PluginInstallCanceled { .. }
            | Self::PluginUninstallCanceled { .. } => "plugin",
            Self::ThreadComposeChanged { .. } => "thread",
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
            Self::AppCreated { app_id, .. }
            | Self::AppUpdated { app_id, .. }
            | Self::AppDeleted { app_id, .. } => app_id.clone(),
            Self::DomainEvent { event_type, .. } => event_type.clone(),
            Self::ChangeDiscarded { change_id } => change_id.clone(),
            Self::ThreadFocused { thread_id, .. } | Self::ThreadUnfocused { thread_id, .. } => {
                thread_id.to_string()
            }
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
            | Self::TriggerExecuted { payload, .. }
            | Self::DomainEvent { payload, .. } => payload.clone(),
            _ => serde_json::to_value(self).unwrap_or_default(),
        }
    }
}
