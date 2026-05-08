//! EventBus — single emission point for all domain events.
//!
//! Producers call typed methods (emit_thread, emit_notification, etc.).
//! The bus persists the event, updates projections, and broadcasts to consumers.
//! Consumers (SSE, memory indexer, etc.) subscribe independently.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::core::store::LegacyInitiator;
use crate::engine::thread_events::{ActorMode, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::{self, resolve_transition, ArchiveState, ThreadType};

/// DB row from thread_summaries for child-to-parent fan-out:
/// (parent_thread_id, is_cc, title, first_message, parent_callback_sent).
type ChildSummaryRow = (Option<Uuid>, bool, Option<String>, Option<String>, bool);

/// Status expression used by every "response/session done" projection: the
/// thread goes 'waiting' iff the CC session left pending changes to review,
/// otherwise 'idle'. CodingAgentIdled binds the value as $2 (it's also being
/// written in the same query); the rest read the stored cc_has_changes.
const STATUS_FROM_CC_HAS_CHANGES: &str = "CASE WHEN cc_has_changes THEN 'waiting' ELSE 'idle' END";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What flows through the broadcast channel. Consumers match on `typed`.
#[derive(Clone, Debug)]
pub struct EmittedEvent {
    /// Event UUID — always present. For persisted events this is the DB primary key;
    /// for transient events a fresh UUID is generated for SSE correlation.
    pub event_id: Uuid,
    /// DB sequence number (None for transient events).
    pub seq: Option<i64>,
    /// When the event was created.
    pub created: DateTime<Utc>,
    /// Typed event — consumers match on the variant.
    pub typed: BusEvent,
    /// Post-event projection snapshot. Set for persisted Thread events
    /// (fetched in-tx after the projection update). `None` for transient
    /// Thread events, System events, and child-count broadcasts — those
    /// don't represent a state delta the frontend needs to apply.
    pub aggregate: Option<crate::core::store::ThreadAggregate>,
}

/// Typed union of all aggregate events.
#[derive(Clone, Debug)]
pub enum BusEvent {
    /// Thread-scoped event (persisted or transient, determined by event.is_persisted()).
    Thread {
        thread_id: Uuid,
        event: ThreadEvent,
        meta: EventMeta,
    },
    /// System/global event (aggregate identity on the event itself).
    System(SystemEvent),
}

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
    /// A plugin was marked uninstalled. Guide-only in v1 — files stay until the
    /// LLM (or user) deletes them. Listed `files` is what was installed; some
    /// may have been edited or shared with another plugin.
    PluginUninstalled {
        id: String,
        version: String,
        files: Vec<String>,
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
            Self::PluginInstalled { .. } | Self::PluginUninstalled { .. } => "plugin",
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

impl EmittedEvent {
    /// Convert to SSE-compatible JSON string.
    /// Thread events use `{ "type": "ThreadEvent", "data": { thread_id, seq?, event } }`.
    /// System events serialize directly via serde `#[serde(tag = "type", content = "data")]`.
    pub fn to_sse_json(&self) -> String {
        let json = match &self.typed {
            BusEvent::Thread {
                thread_id,
                event,
                meta,
            } => {
                let mut event_json = serde_json::to_value(event).unwrap_or_default();
                // Merge EventMeta fields (channel, request_event_id, etc.) into the
                // event JSON so SSE consumers see the same shape as DB-loaded events.
                if let Some(obj) = event_json.as_object_mut() {
                    meta.apply(obj);
                }
                let mut data = serde_json::json!({
                    "thread_id": thread_id.to_string(),
                    "event": event_json,
                    "created": self.created.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
                if let Some(seq) = self.seq {
                    data["seq"] = serde_json::json!(seq);
                }
                data["event_id"] = serde_json::json!(self.event_id.to_string());
                if let Some(agg) = &self.aggregate {
                    if let Ok(agg_json) = serde_json::to_value(agg) {
                        data["aggregate"] = agg_json;
                    }
                }
                serde_json::json!({ "type": "ThreadEvent", "data": data })
            }
            BusEvent::System(SystemEvent::DomainEvent {
                event_type,
                payload,
                ..
            }) => {
                // Domain events are user-defined at runtime, so they live inside a
                // wrapper variant in Rust. On the wire we unwrap to the inner type so
                // the frontend can dispatch by the actual event name (e.g. the SDK's
                // `lucidos.sse.on('SlidePresenterState', ...)` matches the producer's
                // `emit_event('SlidePresenterState', payload)`).
                serde_json::json!({ "type": event_type, "data": payload })
            }
            BusEvent::System(event) => serde_json::to_value(event).unwrap_or_default(),
        };
        json.to_string()
    }
}

// ---------------------------------------------------------------------------
// EventBus
// ---------------------------------------------------------------------------

/// Result of emitting a persisted event.
pub struct EmitResult {
    /// UUID of the persisted event row.
    pub event_id: Uuid,
    /// Auto-assigned DB sequence number.
    pub seq: i64,
}

/// Sent when a child thread completes and needs to notify its parent.
#[derive(Debug)]
pub struct ParentCallback {
    pub parent_thread_id: Uuid,
    pub child_thread_id: Uuid,
    pub callback_text: String,
}

/// Trait for emitting domain events. Extracted from `EventBus` to allow
/// mock implementations in tests.
#[async_trait]
pub trait EventBusEmitter: Send + Sync {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Channel capacity for the event broadcast.
const BUS_CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct EventBus {
    pool: PgPool,
    event_tx: broadcast::Sender<EmittedEvent>,
    parent_callback_tx: mpsc::UnboundedSender<ParentCallback>,
    changes_projection: crate::core::changes_projection::ChangesProjection,
}

impl EventBus {
    pub fn new(pool: PgPool) -> (Self, mpsc::UnboundedReceiver<ParentCallback>) {
        let (event_tx, _) = broadcast::channel(BUS_CAPACITY);
        let (parent_callback_tx, parent_callback_rx) = mpsc::unbounded_channel();
        let changes_projection =
            crate::core::changes_projection::ChangesProjection::new(pool.clone());
        (
            Self {
                pool,
                event_tx,
                parent_callback_tx,
                changes_projection,
            },
            parent_callback_rx,
        )
    }

    pub fn changes_projection(&self) -> &crate::core::changes_projection::ChangesProjection {
        &self.changes_projection
    }

    /// Subscribe to all events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<EmittedEvent> {
        self.event_tx.subscribe()
    }

    /// Get a clone of the sender (for passing to consumers that need to check capacity, etc.)
    pub fn sender(&self) -> broadcast::Sender<EmittedEvent> {
        self.event_tx.clone()
    }

    // ---- Shared persistence ----

    /// Persist an event to the events table. Returns (event_id, sequence).
    async fn persist(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event_id: Uuid,
        aggregate: &str,
        aggregate_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<i64, sqlx::Error> {
        let seq: i64 = sqlx::query_scalar(
            r#"INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id)
               VALUES ($1, $2, $3, $4, $5, NOW(),
                       CASE WHEN $2 = 'thread' THEN $3::uuid ELSE NULL END)
               RETURNING sequence"#,
        )
        .bind(event_id)
        .bind(aggregate)
        .bind(aggregate_id)
        .bind(event_type)
        .bind(payload)
        .fetch_one(&mut **tx)
        .await?;

        Ok(seq)
    }

    // ---- Unified emit ----

    /// Emit an event and return only the persisted event_id, swallowing emit
    /// errors. Use when callers want the id (e.g. to record which `ToolCalled`
    /// triggered a spawn) but don't want to short-circuit on a bus failure.
    pub async fn emit_for_id(&self, event: BusEvent) -> Option<Uuid> {
        self.emit(event).await.ok().flatten().map(|r| r.event_id)
    }

    /// Emit and log on failure. Use when the caller wants observability for
    /// emit failures but cannot meaningfully recover (e.g. background
    /// broadcasts, projection updates after the primary work has succeeded).
    /// `ctx` should identify the call site, e.g. `"[ChangeOps] ChangeApplied"`.
    pub async fn emit_or_log(&self, event: BusEvent, ctx: &str) {
        if let Err(e) = self.emit(event).await {
            log!("[EventBus] {} emit failed: {}", ctx, e);
        }
    }

    /// Single entry point for all events.
    /// Persistence is determined by the event's `is_persisted()` method.
    pub async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        match &event {
            BusEvent::Thread {
                thread_id,
                event: te,
                meta,
            } => {
                if te.is_persisted() {
                    let event_id = meta.event_id.unwrap_or_else(Uuid::new_v4);
                    let mut tx = self.pool.begin().await?;

                    // Validate request_event_id exists in the DB. Orphaned references
                    // cause stuck threads when the frontend can't group events into
                    // exchanges. Log loudly so callers fix their origin_id handling.
                    if let Some(ref req_id) = meta.request_event_id {
                        let exists: bool =
                            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
                                .bind(req_id)
                                .fetch_one(&mut *tx)
                                .await
                                .unwrap_or(false);
                        if !exists {
                            crate::log!(
                                "[EventBus] WARNING: request_event_id {} does not exist in events table \
                                 (event_type={}, thread_id={}). This causes orphaned event references.",
                                req_id, te.event_type(), thread_id
                            );
                        }
                    }

                    let seq = self
                        .persist(
                            &mut tx,
                            event_id,
                            "thread",
                            &thread_id.to_string(),
                            te.event_type(),
                            &te.to_payload(meta),
                        )
                        .await?;
                    let side_effects = self
                        .update_thread_projection(&mut tx, *thread_id, te, meta)
                        .await?;
                    // Read-your-write within the same tx so the snapshot reflects
                    // exactly this event's post-state — no race with a concurrent
                    // emit for the same thread committing between the projection
                    // update and a post-commit fetch. A failed fetch logs and
                    // broadcasts without aggregate (frontend tolerates absence
                    // with a warning, but it indicates a backend bug).
                    let aggregate =
                        match crate::core::store::fetch_thread_aggregate(&mut *tx, *thread_id)
                            .await
                        {
                            Ok(agg) => agg,
                            Err(e) => {
                                crate::log!(
                                    "[EventBus] Failed to fetch ThreadAggregate for {}: {}",
                                    thread_id,
                                    e
                                );
                                None
                            }
                        };
                    tx.commit().await?;
                    let broadcast_created = Utc::now();

                    // Capture what notify_parent_if_child needs before event is moved
                    let notify_thread_id = *thread_id;
                    let notify_event = te.clone();

                    let _ = self.event_tx.send(EmittedEvent {
                        event_id,
                        seq: Some(seq),
                        created: broadcast_created,
                        typed: event,
                        aggregate,
                    });
                    // Run after broadcast so a panic here can't skip SSE delivery
                    self.notify_parent_if_child(notify_thread_id, &notify_event)
                        .await;
                    // If a child was just created, notify the parent with updated counts
                    if let ThreadEvent::MessageReceived {
                        parent_thread_id: Some(pid),
                        ..
                    } = &notify_event
                    {
                        self.broadcast_children_count(*pid).await;
                    }
                    // Side-effect events run in their own transactions, after the
                    // main commit. Section changes are NOT among them — the
                    // per-event aggregate already carries the post-projection
                    // section to subscribers, no follow-up broadcast required.
                    for effect in side_effects {
                        if let Err(e) = Box::pin(self.emit(effect)).await {
                            crate::log!("[EventBus] Side-effect emit failed: {}", e);
                        }
                    }
                    Ok(Some(EmitResult { event_id, seq }))
                } else {
                    let _ = self.event_tx.send(EmittedEvent {
                        event_id: Uuid::new_v4(),
                        seq: None,
                        created: Utc::now(),
                        typed: event,
                        aggregate: None,
                    });
                    Ok(None)
                }
            }
            BusEvent::System(se) => {
                if se.is_persisted() {
                    let event_id = Uuid::new_v4();
                    let stored_event_type = match &se {
                        SystemEvent::DomainEvent { event_type, .. } => event_type.as_str(),
                        _ => se.event_type(),
                    };
                    let mut tx = self.pool.begin().await?;
                    let seq = self
                        .persist(
                            &mut tx,
                            event_id,
                            se.aggregate(),
                            &se.aggregate_id(),
                            stored_event_type,
                            &se.to_payload(),
                        )
                        .await?;
                    self.update_system_projection(&mut tx, event_id, se).await?;
                    tx.commit().await?;

                    let _ = self.event_tx.send(EmittedEvent {
                        event_id,
                        seq: Some(seq),
                        created: Utc::now(),
                        typed: event,
                        aggregate: None,
                    });
                    Ok(Some(EmitResult { event_id, seq }))
                } else {
                    // Transient system events still drive projections (e.g.
                    // thread_presence). The events table is intentionally
                    // skipped — these are high-churn and not interesting to
                    // replay. Skip the SSE broadcast when the projection
                    // reports no real change (e.g. ThreadFocused heartbeats
                    // every 30s from the frontend).
                    let should_broadcast = self.update_transient_system_projection(se).await?;
                    if should_broadcast {
                        let _ = self.event_tx.send(EmittedEvent {
                            event_id: Uuid::new_v4(),
                            seq: None,
                            created: Utc::now(),
                            typed: event,
                            aggregate: None,
                        });
                    }
                    Ok(None)
                }
            }
        }
    }

    /// Project transient system events that maintain external state (without
    /// being persisted to the events table). Returns `true` when callers
    /// should still broadcast the event to SSE consumers, `false` when the
    /// projection update was a no-op (heartbeat refresh, redundant unfocus).
    /// Variants without a projection always broadcast.
    async fn update_transient_system_projection(
        &self,
        event: &SystemEvent,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match event {
            SystemEvent::ThreadFocused {
                thread_id,
                device_id,
            } => {
                crate::core::ThreadPresenceStore::record_focused(&self.pool, device_id, *thread_id)
                    .await
            }
            SystemEvent::ThreadUnfocused {
                thread_id,
                device_id,
            } => {
                crate::core::ThreadPresenceStore::record_unfocused(
                    &self.pool, device_id, *thread_id,
                )
                .await
            }
            _ => Ok(true),
        }
    }

    // ---- Parent callback ----

    /// Send a ChildrenCountChanged transient event to the parent thread's SSE channel.
    /// `aggregate` carries any other projection changes (e.g. archive_state) the
    /// caller made before emitting — the frontend overlays it onto thread.meta.
    fn send_children_count_event(
        &self,
        parent_id: Uuid,
        active: i64,
        total: i64,
        aggregate: Option<crate::core::store::ThreadAggregate>,
    ) {
        let _ = self.event_tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: None,
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: parent_id,
                event: ThreadEvent::ChildrenCountChanged { active, total },
                meta: EventMeta::default(),
            },
            aggregate,
        });
    }

    /// Query children counts from DB and broadcast to the parent thread's SSE channel.
    async fn broadcast_children_count(&self, parent_id: Uuid) {
        let counts: Option<(i64, i64)> = match sqlx::query_as(
            "SELECT active_children_count::bigint, total_children_count::bigint FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(row) => row,
            Err(e) => {
                crate::log!("[EventBus] Failed to query children counts for {}: {}", parent_id, e);
                return;
            }
        };
        if let Some((active, total)) = counts {
            self.send_children_count_event(parent_id, active, total, None);
        }
    }

    /// Handle parent notification when a child thread emits a terminal event.
    /// Decrements the parent's `active_children_count` and, for completion events,
    /// sends a callback message with results.
    async fn notify_parent_if_child(&self, child_thread_id: Uuid, event: &ThreadEvent) {
        // Terminal events that mean a child is done (decrement counter).
        // Canceled/Aborted children didn't complete — no callback, but still done.
        // Transient SessionEnded reasons (StaleResume) are mid-retry: they must
        // not decrement the parent counter or fire the completion callback, or
        // the real CodingAgentIdled that lands seconds later would be orphaned.
        let is_terminal = match event {
            ThreadEvent::CodingAgentIdled { .. }
            | ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseFailed { .. }
            | ThreadEvent::ResponseCanceled { .. }
            | ThreadEvent::ResponseAborted { .. } => true,
            ThreadEvent::SessionEnded { reason } => !reason.is_transient(),
            _ => false,
        };
        if !is_terminal {
            return;
        }

        // Look up parent, child info, CC status, and whether callback was already sent
        let row: Option<ChildSummaryRow> = match sqlx::query_as::<_, ChildSummaryRow>(
            "SELECT parent_thread_id, is_cc, title, first_message, parent_callback_sent FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(child_thread_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(Some(row)) => Some(row),
            Ok(None) => return,
            Err(e) => {
                crate::log!("[FanOut] Failed to look up parent for child {}: {}", child_thread_id, e);
                return;
            }
        };

        let Some((Some(parent_id), is_cc, title, first_msg, callback_already_sent)) = row else {
            return;
        };

        // CC threads can emit CodingAgentIdled multiple times (initial work,
        // auto-harden, background agents). Only process the first one —
        // subsequent idles should not decrement the counter again or send
        // duplicate callbacks to the parent.
        if is_cc && callback_already_sent && matches!(event, ThreadEvent::CodingAgentIdled { .. }) {
            return;
        }

        // CC sessions can terminate without ever emitting CodingAgentIdled or
        // SessionEnded — e.g. the user cancels and the session sits archived,
        // leaving only ResponseCanceled/ResponseAborted as terminal signals. The
        // `!callback_already_sent` guard collapses multiple terminal events for
        // the same child to a single decrement.
        let should_decrement = if is_cc {
            matches!(event, ThreadEvent::CodingAgentIdled { .. })
                || (!callback_already_sent
                    && matches!(
                        event,
                        ThreadEvent::SessionEnded { .. }
                            | ThreadEvent::ResponseCanceled { .. }
                            | ThreadEvent::ResponseAborted { .. }
                    ))
        } else {
            matches!(
                event,
                ThreadEvent::ResponseGenerated { .. }
                    | ThreadEvent::ResponseFailed { .. }
                    | ThreadEvent::ResponseCanceled { .. }
                    | ThreadEvent::ResponseAborted { .. }
                    | ThreadEvent::SessionEnded { .. }
            )
        };
        // Completion events (not cancel/abort) trigger a callback to the parent
        // and surface the parent to inbox. For CC children, SessionEnded also
        // counts when no prior callback was sent — handles CC sessions that end
        // without ever idling (crash, shutdown, user-ended).
        let should_callback = matches!(
            (is_cc, event),
            (true, ThreadEvent::CodingAgentIdled { .. })
                | (false, ThreadEvent::ResponseGenerated { .. })
                | (_, ThreadEvent::ResponseFailed { .. })
        ) || (is_cc
            && !callback_already_sent
            && matches!(event, ThreadEvent::SessionEnded { .. }));

        if should_decrement || should_callback {
            self.update_parent_after_child_terminal(
                parent_id,
                should_decrement,
                should_callback,
            )
            .await;
        }

        // For CC cancel/abort the should_callback path doesn't fire (we don't
        // want to tell the parent LLM "your child was canceled" — that's UI
        // noise the user already sees). But we still need to mark the callback
        // as sent so a subsequent CodingAgentIdled or SessionEnded for the
        // same child doesn't decrement again.
        if should_decrement
            && is_cc
            && matches!(
                event,
                ThreadEvent::ResponseCanceled { .. } | ThreadEvent::ResponseAborted { .. }
            )
        {
            self.mark_parent_callback_sent(child_thread_id).await;
        }

        if !should_callback {
            return;
        }

        let label = title
            .or_else(|| first_msg.map(|m| m.chars().take(80).collect()))
            .unwrap_or_else(|| "unknown task".into());

        // Fetch the child thread's last response text to include in callback
        let last_response: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseGenerated' \
             AND payload->>'text' IS NOT NULL AND payload->>'text' != '' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(child_thread_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[FanOut] Failed to fetch child response for {}: {}",
                child_thread_id,
                e
            );
            None
        });

        let status = match event {
            ThreadEvent::CodingAgentIdled {
                has_changes: true, ..
            } => "completed with proposed changes",
            ThreadEvent::CodingAgentIdled {
                has_changes: false, ..
            } => "completed (no changes)",
            ThreadEvent::ResponseGenerated { .. } => "completed",
            ThreadEvent::ResponseFailed { error } => {
                let truncated = &error[..error.floor_char_boundary(200)];
                self.mark_parent_callback_sent(child_thread_id).await;
                return self.send_parent_callback(
                    parent_id,
                    child_thread_id,
                    &format!(
                        "[Child thread failed] Thread \"{}\" (id: {}) failed: {}",
                        label, child_thread_id, truncated
                    ),
                );
            }
            _ => "finished",
        };

        let result_section = if let Some(response) = last_response {
            let max_len = 2000;
            let truncated = &response[..response.floor_char_boundary(max_len)];
            let suffix = if response.len() > max_len {
                "… (truncated)"
            } else {
                ""
            };
            format!("\n\nResult:\n{}{}", truncated, suffix)
        } else {
            String::new()
        };

        let callback_text = format!(
            "[Child thread completed] Thread \"{}\" (id: {}) {}.\
             \nPhrases like \"session can finish\" or \"## Session Summary\" describe \
             the child subprocess only — if you were following a multi-step procedure, \
             continue with the next step. Otherwise use run_thread to refine.{}",
            label, child_thread_id, status, result_section
        );

        self.mark_parent_callback_sent(child_thread_id).await;
        self.send_parent_callback(parent_id, child_thread_id, &callback_text);
    }

    /// Combined into one round-trip + one broadcast so subscribers see the
    /// count change and the section change in the same envelope — replaces the
    /// old separate `ThreadMarkedUnread` side-effect that raced with the
    /// children-count broadcast.
    async fn update_parent_after_child_terminal(
        &self,
        parent_id: Uuid,
        decrement: bool,
        surface_to_inbox: bool,
    ) {
        let dec = if decrement { 1_i64 } else { 0 };
        let new_archive = if surface_to_inbox {
            Some(ArchiveState::Inbox.as_str())
        } else {
            None
        };
        let row: Option<(i64, i64)> = match sqlx::query_as(
            "UPDATE thread_summaries SET \
             active_children_count = GREATEST(0, active_children_count - $2), \
             archive_state = COALESCE($3, archive_state) \
             WHERE thread_id = $1 \
             RETURNING active_children_count::bigint, total_children_count::bigint",
        )
        .bind(parent_id)
        .bind(dec)
        .bind(new_archive)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to update parent {} after child terminal: {}",
                    parent_id,
                    e
                );
                return;
            }
        };
        let Some((active, total)) = row else { return };
        let aggregate = match crate::core::store::fetch_thread_aggregate(&self.pool, parent_id)
            .await
        {
            Ok(agg) => agg,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to fetch aggregate for parent {}: {}",
                    parent_id,
                    e
                );
                None
            }
        };
        self.send_children_count_event(parent_id, active, total, aggregate);
    }

    async fn mark_parent_callback_sent(&self, child_thread_id: Uuid) {
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET parent_callback_sent = TRUE WHERE thread_id = $1",
        )
        .bind(child_thread_id)
        .execute(&self.pool)
        .await
        {
            crate::log!(
                "[FanOut] Failed to mark callback sent for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    fn send_parent_callback(
        &self,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        callback_text: &str,
    ) {
        if let Err(e) = self.parent_callback_tx.send(ParentCallback {
            parent_thread_id,
            child_thread_id,
            callback_text: callback_text.to_string(),
        }) {
            crate::log!(
                "[FanOut] Failed to send parent callback for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    // ---- Contract helpers ----

    /// Get the thread type from thread_summaries.
    async fn get_thread_type(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> ThreadType {
        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await
                .unwrap_or(None);
        if source.as_deref() == Some("claude_code") {
            ThreadType::CodingAgent
        } else {
            ThreadType::Chat
        }
    }

    /// Get the current stored section from thread_summaries.
    async fn get_current_section(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> ArchiveState {
        let section: Option<String> =
            sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await
                .unwrap_or(None);
        section
            .map(|s| ArchiveState::parse(&s))
            .unwrap_or(ArchiveState::Archived)
    }

    /// Apply a contract transition result to the database. Only effect is the
    /// section update — the per-event aggregate snapshot then carries the new
    /// state to subscribers, so no follow-up section-changing event is emitted.
    async fn apply_transition(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        result: &thread_lifecycle::TransitionResult,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(new_section) = result.new_section {
            sqlx::query("UPDATE thread_summaries SET archive_state = $1 WHERE thread_id = $2")
                .bind(new_section.as_str())
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

    // ---- Thread projection ----

    /// Returns side-effect events to emit after the main transaction commits.
    ///
    /// Structure: Step 1 runs metadata updates (the match statement).
    /// Step 2 validates and applies section transitions via the lifecycle contract.
    /// This ensures upsert events create the row before the contract checks it.
    async fn update_thread_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<Vec<BusEvent>, Box<dyn std::error::Error + Send + Sync>> {
        // Step 1: Run metadata updates
        let match_side_effects: Vec<BusEvent> = match event {
            // Thread start events — upsert the summary row
            ThreadEvent::MessageReceived { text, parent_thread_id, spawning_event_id, mode, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("chat");
                // Map ActorMode to the legacy two-state `initiator` column:
                // Human → "user", Agent | Engine → "system". The column was
                // never tri-state; promoting it would require a migration and
                // a frontend type change. See `LegacyInitiator` in
                // core/store/threads.rs for the matching read path.
                let msg_initiator = match mode {
                    ActorMode::Human => LegacyInitiator::User.as_str(),
                    ActorMode::Agent | ActorMode::Engine => LegacyInitiator::System.as_str(),
                };
                // Compute child depth and inherit initiator from parent —
                // a non-Human parent forces "system" on its descendants.
                let (child_depth, initiator) = if let Some(pid) = parent_thread_id {
                    let parent_row: Option<(i32, String)> = sqlx::query_as(
                        "SELECT COALESCE(depth, 0), initiator FROM thread_summaries WHERE thread_id = $1"
                    )
                    .bind(pid)
                    .fetch_optional(&mut **tx)
                    .await?;
                    match parent_row {
                        Some((d, init)) if init == "system" => (d + 1, "system"),
                        Some((d, _)) => (d + 1, msg_initiator),
                        None => (1, msg_initiator),
                    }
                } else {
                    (0, msg_initiator)
                };
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, parent_thread_id, spawning_event_id, depth, status, last_revived_at, state)
                       VALUES ($1, $2, $3, $6, NOW(), NOW(), 1, $4, $7, $5, 'running', NOW(), 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           message_count = thread_summaries.message_count + 1,
                           status = 'running',
                           last_revived_at = NOW(),
                           state = 'active',
                           first_message = COALESCE(thread_summaries.first_message, EXCLUDED.first_message),
                           -- composing → active: the actual send's channel wins
                           -- (the lagged compose-mode source must not survive the
                           -- transition). Active follow-ups already passed the
                           -- continuity check, so 'chat' fall-through covers
                           -- legacy rows missing an explicit assertion.
                           source = CASE
                               WHEN thread_summaries.state = 'composing' THEN EXCLUDED.source
                               WHEN thread_summaries.source = 'chat' THEN EXCLUDED.source
                               ELSE thread_summaries.source
                           END,
                           compose_text = '',
                           compose_images = '[]'::jsonb,
                           compose_mode = NULL
                       -- Defense in depth: refuse to resurrect a discarded thread if a
                       -- stale MessageReceived slips past the API-layer guard.
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(text)
                .bind(source)
                .bind(parent_thread_id)
                .bind(child_depth)
                .bind(initiator)
                .bind(spawning_event_id)
                .execute(&mut **tx)
                .await?;

                // If this message has a parent, increment the parent's active_children_count.
                // Parents are always Chat threads — CC threads are always children.
                if let Some(pid) = parent_thread_id {
                    sqlx::query(
                        "UPDATE thread_summaries SET active_children_count = active_children_count + 1, \
                         total_children_count = total_children_count + 1 WHERE thread_id = $1"
                    )
                    .bind(pid)
                    .execute(&mut **tx)
                    .await?;
                }

                Vec::new()
            }
            // CC session lifecycle — session start/recovery don't update last_activity
            // (the first real activity event will set it).
            ThreadEvent::SessionStarted { .. } | ThreadEvent::SessionRecovered { .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("claude_code");
                // Extract repo_id from SessionStarted; SessionRecovered has no repo_id.
                let session_repo_id = match &event {
                    ThreadEvent::SessionStarted { repo_id, .. } => repo_id.as_deref(),
                    _ => None,
                };
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status, last_revived_at, cc_repo_id, state)
                       VALUES ($1, $2, TRUE, NOW(), NOW(), 0, 'running', NOW(), $3, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET is_cc = TRUE, source = $2,
                           initiator = COALESCE(thread_summaries.initiator, 'unknown'),
                           -- Existing value wins: a thread's repo is locked at first SessionStarted.
                           -- The chat handler enforces that follow-ups can't pick a different repo,
                           -- but defend the projection so any drift (legacy data, replay) doesn't
                           -- silently flip the thread to a different repo's skill set.
                           cc_repo_id = COALESCE(thread_summaries.cc_repo_id, $3),
                           state = 'active',
                           compose_text = '',
                           compose_images = '[]'::jsonb,
                           compose_mode = NULL
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(source)
                .bind(session_repo_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::TriggerStarted { trigger_id, trigger_name, go_to_review, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("trigger");
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, status, last_revived_at, trigger_id, trigger_name, trigger_go_to_review, state)
                       VALUES ($1, $2, $3, 'system', NOW(), NOW(), 1, 'running', NOW(), $4, $5, $6, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           message_count = thread_summaries.message_count + 1,
                           status = 'running',
                           last_revived_at = NOW(),
                           state = 'active',
                           trigger_id = COALESCE(thread_summaries.trigger_id, EXCLUDED.trigger_id),
                           trigger_name = COALESCE(thread_summaries.trigger_name, EXCLUDED.trigger_name)
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(trigger_name.as_deref())
                .bind(source)
                .bind(trigger_id.as_str())
                .bind(trigger_name.as_deref())
                .bind(*go_to_review)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Activity events — update last_activity + status
            ThreadEvent::ResponseGenerated { .. } => {
                // Normal completion — go idle (or waiting if CC has pending changes).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, \
                     status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ResponseAborted { .. } => {
                // System interruption — same red error indicator as ResponseFailed,
                // unless a CC session left pending changes (changes dot wins).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, \
                     status = CASE WHEN cc_has_changes THEN 'waiting' ELSE 'failed' END \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CodingAgentIdled { has_changes, requires_restart, is_external_repo, .. } => {
                // CC session idled — SET (not OR) cc_has_changes from payload.
                // CodingAgentIdled is the authoritative snapshot of the session's state.
                // After apply/discard, the session emits has_changes=false to clear the flag.
                // Set has_response = TRUE so the thread appears in get_recent_threads
                // (CC threads don't go through ResponseGenerated, so this is the CC equivalent).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     has_response = TRUE, \
                     cc_has_changes = $2, \
                     cc_requires_restart = $3, \
                     cc_is_external_repo = $4, \
                     status = CASE WHEN $2 THEN 'waiting' ELSE 'idle' END \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(has_changes)
                .bind(requires_restart)
                .bind(is_external_repo)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeApplied {
                change_id,
                requires_restart,
                commits,
                pre_merge_sha,
                post_merge_sha,
                ..
            } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     status = 'idle' \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_applied(
                    tx,
                    change_id,
                    *requires_restart,
                    commits,
                    pre_merge_sha.as_deref(),
                    post_merge_sha.as_deref(),
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeDiscarded { change_id, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     status = 'idle' \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, "discarded",
                )
                .await?;
                Vec::new()
            }

            // Message count increment + activity (CC user messages and mid-flight injections)
            ThreadEvent::CodingAgentUserMessageSent { .. }
            | ThreadEvent::UserPromptInjected { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), message_count = message_count + 1, status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Title events
            ThreadEvent::ThreadTitleGenerated { title } | ThreadEvent::ThreadTitleRenamed { title } => {
                sqlx::query(
                    "UPDATE thread_summaries SET title = $2 WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(title)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Save/unsave
            ThreadEvent::ThreadSaved => {
                sqlx::query(
                    "UPDATE thread_summaries SET is_saved = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadUnsaved => {
                sqlx::query(
                    "UPDATE thread_summaries SET is_saved = FALSE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::ThreadArchived => {
                // Clear is_saved so display priority doesn't keep the row in
                // Saved (is_saved=true wins over state='archived').
                sqlx::query(
                    "UPDATE thread_summaries SET status = 'idle', \
                     state = 'archived', \
                     is_saved = FALSE, \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     active_children_count = 0, total_children_count = 0 \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadStarted { mode, .. } => {
                // Compose-time thread creation — the row appears in
                // `thread_summaries` with `state='composing'` so the frontend
                // can render it as a draft via cross-device SSE. Default
                // initiator is `user` since only humans open compose. Source
                // mirrors the user's chosen mode so a draft that auto-archives
                // before being sent still surfaces with the correct channel
                // pill. Send events later re-assert source via the
                // `source = 'chat'`-keyed CASE in MessageReceived.
                let source = if mode == "claude_code" { "claude_code" } else { "chat" };
                sqlx::query(
                    r#"INSERT INTO thread_summaries
                        (thread_id, initiator, source, created_at, last_activity, message_count,
                         state, compose_mode, status)
                       VALUES ($1, 'user', $3, NOW(), NOW(), 0, 'composing', $2, 'idle')
                       ON CONFLICT (thread_id) DO NOTHING"#,
                )
                .bind(thread_id)
                .bind(mode)
                .bind(source)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadDiscarded { .. } => {
                // Terminal transition. Only valid from `composing` — the
                // state-machine guard at the API boundary already rejected
                // discard from active/archived, so this UPDATE is safe to run
                // without re-checking. Compose fields are wiped so a stale
                // SSE replay can't show ghost text.
                sqlx::query(
                    "UPDATE thread_summaries SET state = 'discarded', \
                     compose_text = '', compose_images = '[]'::jsonb, compose_mode = NULL \
                     WHERE thread_id = $1 AND state = 'composing'",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Events that update status but no other metadata.
            ThreadEvent::ResponseCanceled { .. } => {
                // User canceled — go idle (or waiting if pending changes).
                // Set has_response so the thread appears in history (a canceled
                // response is still a response — the user should see the thread).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET has_response = TRUE, \
                     status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ResponseFailed { .. } => {
                // Failed response — distinct from 'waiting' (which means CC has
                // changes to review) so the UI can render an error indicator.
                // Set has_response so the thread stays visible.
                sqlx::query(
                    "UPDATE thread_summaries SET has_response = TRUE, status = 'failed' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::SessionEnded { reason } => {
                // Transient reasons (StaleResume) are mid-retry — the chat
                // handler is about to spawn a fresh session within the same
                // request. Flipping to terminal here would render the exchange
                // as "Aborted" until the retry's SessionStarted lands.
                if !reason.is_transient() {
                    sqlx::query(&format!(
                        "UPDATE thread_summaries SET has_response = TRUE, \
                         status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                    ))
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                }
                Vec::new()
            }
            ThreadEvent::TriggerCompleted { .. } => {
                // Trigger run done — go idle. Set has_response so the thread
                // appears in get_recent_threads (which filters has_response=TRUE).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, status = 'idle' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeProposed {
                change_id,
                description,
                files,
                requires_restart,
                commit_sha,
                branch_name,
                repo_root,
                hardened,
                incomplete,
                ..
            } => {
                // CodingAgentIdled already set status='waiting' if the session idled;
                // mid-session commits keep status='running'. Only the flag changes.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_has_changes = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                use crate::core::changes_projection::ChangesProjection;
                if change_id.is_empty() && commit_sha.is_some() {
                    ChangesProjection::write_proposed_per_commit(
                        tx,
                        branch_name,
                        description.as_deref(),
                        *requires_restart,
                    )
                    .await?;
                } else if !change_id.is_empty() {
                    ChangesProjection::write_proposed_aggregate(
                        tx,
                        change_id,
                        thread_id,
                        branch_name,
                        repo_root,
                        description.as_deref(),
                        files,
                        *requires_restart,
                        *hardened,
                        *incomplete,
                    )
                    .await?;
                }
                Vec::new()
            }
            ThreadEvent::MergeConflictDetected { .. } => {
                // Merge conflict — mark as applying.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_applying = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeApplyFailed { .. } => {
                // Apply failed — clear applying flag, stay waiting.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_applying = FALSE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CodingAgentPromptSent { text, .. } => {
                // Empty prompt = no agent intent → no status change. Real prompts
                // always carry text (user follow-up audit trail, automated CC
                // sessions). The contract in `status_transitions()` reflects this
                // exception.
                if !text.is_empty() {
                    sqlx::query(
                        "UPDATE thread_summaries SET status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                }
                Vec::new()
            }
            ThreadEvent::ContinueSignal { .. } => {
                // Continuation start event — bump last_activity and flip status
                // back to running so the thread surfaces in the recents list as
                // soon as the dispatcher emits the spawn. The contract's
                // status_transitions table sets Running too; we emit the SQL
                // here so the timestamp moves alongside it.
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ToolCalled { .. }
            | ThreadEvent::ToolResult { .. }
            | ThreadEvent::TextStreamed { .. }
            | ThreadEvent::Thinking { .. }
            | ThreadEvent::CodingAgentTextStreamed { .. }
            | ThreadEvent::CodingAgentToolCalled { .. }
            | ThreadEvent::CodingAgentToolResult { .. } => {
                // Update last_activity so the thread list timestamp stays current during
                // long-running agentic responses. Without this, the timestamp only
                // advances on discrete lifecycle events, not during streaming.
                //
                // Also bump status back to 'running' if the projection drifted to a
                // non-running state (e.g. CC emitted a mid-session `Result` that the
                // engine treated as idle, then continued working). `last_revived_at`
                // is gated by CASE rather than set unconditionally — sibling UPDATEs
                // for one-shot transitions (`ContinueSignal`, etc.) refresh it every
                // time, but activity events fire many times per turn and would
                // constantly reshuffle IN PROGRESS sort order if treated the same
                // way. Mirrors `status_transitions()` for these event types — see
                // `thread_lifecycle.rs`.
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     last_revived_at = CASE WHEN status != 'running' THEN NOW() \
                                            ELSE last_revived_at END, \
                     status = 'running' \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            // Both CC AskUserQuestion and CC permission prompts pause the
            // exchange on user input — surface in REVIEW. AskUserQuestion kills
            // the CC subprocess; the permission prompt keeps it alive while
            // its MCP stdio server blocks on the engine's HTTP response. The
            // projection treats them identically: status flips to
            // 'waiting_for_user_answer' on the request and back to 'running'
            // on the resolution.
            ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = 'waiting_for_user_answer' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeReverted { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, "reverted",
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeHardened { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_hardened(tx, change_id)
                    .await?;
                Vec::new()
            }
            ThreadEvent::MergeResolutionStarted {
                change_id,
                worktree_path,
                temp_branch,
            } => {
                crate::core::changes_projection::ChangesProjection::write_merge_started(
                    tx,
                    change_id,
                    worktree_path,
                    temp_branch,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::MergeResolutionCleared { change_id } => {
                crate::core::changes_projection::ChangesProjection::write_merge_cleared(
                    tx, change_id,
                )
                .await?;
                Vec::new()
            }
            // Events that don't affect thread_summaries metadata or status.
            // Exhaustive match — adding a new ThreadEvent variant forces you to decide
            // whether it needs a projection update. Never use `_ =>` here.
            ThreadEvent::MemorySearched { .. }
            | ThreadEvent::CredentialRequested { .. }
            | ThreadEvent::McpConsentRequested { .. }
            // Transient events (never persisted, never reach this function)
            | ThreadEvent::TextStreaming { .. }
            | ThreadEvent::Retrying { .. }
            | ThreadEvent::PreambleCompleting
            | ThreadEvent::CredentialRequest { .. }
            | ThreadEvent::EmailConfirmRequest { .. }
            | ThreadEvent::PushNotificationRequest
            | ThreadEvent::McpConsentRequest { .. }
            | ThreadEvent::RefreshFile { .. }
            | ThreadEvent::RefreshAppUI { .. }
            | ThreadEvent::CaptureAppUI { .. }
            | ThreadEvent::NavigationRequested { .. }
            | ThreadEvent::CodingAgentThreadSpawned { .. }
            | ThreadEvent::ChildrenCountChanged { .. }
            | ThreadEvent::MissingHardeningDetected { .. }
            | ThreadEvent::CodingAgentSettingsChanged { .. }
            // Passive bookkeeping for the background cleanup worker
            // (Phase 10.2). Persisted to the events stream for audit /
            // debugging but produces no projection side effects.
            | ThreadEvent::WorktreeCleaned { .. }
            // ImageUploaded is a per-thread audit fact for content-addressed
            // blob uploads. Persisted for audit + cross-device prefetch hint
            // via SSE; no projection side effects (no status change, no
            // section transition, no last_activity bump).
            | ThreadEvent::ImageUploaded { .. }
            | ThreadEvent::ContextTokensMeasured { .. } => Vec::new(),
        };

        // Step 2: Validate and apply section transition via the lifecycle contract.
        // This runs after metadata updates so upsert events have created the row.
        let thread_type = Self::get_thread_type(tx, &thread_id).await;
        let current = Self::get_current_section(tx, &thread_id).await;
        let (depth, source, trigger_go_to_review): (i32, Option<String>, bool) = sqlx::query_as(
            "SELECT COALESCE(depth, 0), source, COALESCE(trigger_go_to_review, FALSE) \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await
        .unwrap_or(None)
        .unwrap_or((0, None, false));
        // Trigger executions run unattended — don't surface in REVIEW. But
        // user followups on trigger threads ARE attended (latest start =
        // MessageReceived), and triggers with `go_to_review=true` opt back in
        // for reports/alerts the user is meant to read.
        let is_top_level = if depth > 0 {
            false
        } else if source.as_deref() != Some("trigger") || trigger_go_to_review {
            true
        } else {
            let latest_start: Option<String> = sqlx::query_scalar(
                "SELECT event_type FROM events WHERE aggregate_id = $1::text \
                 AND event_type IN ('TriggerStarted', 'MessageReceived') \
                 ORDER BY sequence DESC LIMIT 1",
            )
            .bind(thread_id)
            .fetch_optional(&mut **tx)
            .await?;
            latest_start.as_deref() == Some("MessageReceived")
        };
        match resolve_transition(event.event_type(), thread_type, current, is_top_level) {
            Ok(mut transition) => {
                // CodingAgentIdled(has_changes=false) after apply/discard is a housekeeping
                // event — the section is already 'inbox' so setting it again is redundant.
                // When section is Default (first idle with no changes), let the transition
                // through so the thread surfaces in REVIEW — the user needs to know the
                // CC session completed.
                if matches!(
                    event,
                    ThreadEvent::CodingAgentIdled {
                        has_changes: false,
                        ..
                    }
                ) && current == ArchiveState::Inbox
                {
                    transition.new_section = None;
                }
                Self::apply_transition(tx, thread_id, &transition).await?;
            }
            Err(v) => {
                crate::log!("[EventBus] {}", v);
                return Err(Box::new(v));
            }
        }
        Ok(match_side_effects)
    }

    // ---- System projection ----

    async fn update_system_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event_id: Uuid,
        event: &SystemEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            SystemEvent::NotificationCreated {
                id,
                title,
                message,
                task_id,
                app_id,
            } => {
                let notification_id = Uuid::parse_str(id).unwrap_or(event_id);
                let task_uuid = task_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                sqlx::query(
                    "INSERT INTO notifications (id, task_id, app_id, title, message, read, created_at) \
                     VALUES ($1, $2, $3, $4, $5, false, NOW())"
                )
                .bind(notification_id)
                .bind(task_uuid)
                .bind(app_id.as_deref())
                .bind(title)
                .bind(message)
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl EventBusEmitter for EventBus {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        EventBus::emit(self, event).await
    }
}

/// Test-only mock that records emitted events without touching a database.
#[cfg(test)]
pub struct MockEventBus {
    emitted: std::sync::Mutex<Vec<BusEvent>>,
    /// When set, `emit` returns this error instead of `Ok(None)`.
    pub fail_with: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl Default for MockEventBus {
    fn default() -> Self {
        Self {
            emitted: std::sync::Mutex::new(Vec::new()),
            fail_with: std::sync::Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl MockEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emitted_events(&self) -> Vec<BusEvent> {
        self.emitted.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl EventBusEmitter for MockEventBus {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(msg) = self.fail_with.lock().unwrap().as_ref() {
            return Err(msg.clone().into());
        }
        self.emitted.lock().unwrap().push(event);
        Ok(None)
    }
}

#[cfg(test)]
#[path = "event_bus_tests.rs"]
mod tests;
