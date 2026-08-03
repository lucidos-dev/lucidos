//! The write chokepoint for trigger lifecycle events: emit, then leave the
//! trigger registry consistent before returning.
//!
//! Sibling of [`crate::engine::trigger_group_writes`], which does the same for
//! trigger *groups*, and the reason this module exists is that triggers did
//! not. `trigger_configs` was materialized only by the scheduler's EventBus
//! subscriber, which runs asynchronously: `EventBus::emit` committed the event,
//! broadcast it, and returned, and the HTTP handler answered 200 while the
//! registry might still hold pre-write state. Under a loaded e2e suite that
//! window is wide enough that a `PUT {"paused": true}` answering
//! `success: true` was followed by a run request that fired the trigger it had
//! just paused. The same window is reachable from the LLM tool surface
//! (`pause_trigger` then `run_trigger` in one turn) with no HTTP involved.
//!
//! So every trigger write goes through [`TriggerRegistryWriter`], which gives
//! the whole surface read-your-writes: create, update, delete, pause and resume
//! alike, from HTTP, the LLM tools, plugin resync and the thread-queue overflow
//! guard.
//!
//! **Emit first, then apply, never the reverse.** A failed emit must leave the
//! registry untouched: a resume that is live in memory but absent from the
//! event log would come back paused after a restart, silently. Emit-then-apply
//! can only lose the *fast* apply (the subscriber still gets the broadcast), it
//! can never invent state the log does not carry.
//!
//! **And the pair is serialized**, under `LucidosEngine::trigger_write_lock`.
//! `emit` broadcasts before it returns, so two writers racing on one trigger
//! can interleave as A-emit, B-emit, B-apply, A-apply and leave the registry
//! holding A's older payload for good. The scheduler subscriber replays both in
//! log order but is free to have done so before A's late apply, so it is not
//! the backstop it looks like: the lock is.
//!
//! Callers name a [`TriggerWrite`] rather than building the `SystemEvent`
//! themselves. That is what makes the guarantee mechanical instead of
//! customary: the `SystemEvent::Trigger*` constructors exist only in this file,
//! so there is no way to spell a trigger write that skips the registry. A
//! source-scan test holds that line.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use serde_json::Value;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;
use crate::triggers::registry::materialize_trigger_event;
use crate::triggers::TriggerConfig;

/// Which lifecycle event a trigger write records, and the only way to reach
/// the matching `SystemEvent::Trigger*` constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerWrite {
    Created,
    Updated,
    Deleted,
    /// Pause. Distinct from an `Updated` carrying `{"paused": true}` only in
    /// provenance; both land on the same registry field. The engine emits this
    /// one from the thread-queue overflow guard and the legacy migration; the
    /// user-facing pause and resume both go through `Updated`.
    ///
    /// There is deliberately no `Enabled` counterpart: nothing emits
    /// `TriggerEnabled` today. The registry still materializes it, because
    /// workspaces created before the split have the event in their log and
    /// replay walks it.
    Disabled,
}

impl TriggerWrite {
    /// The event name, for logs and for the registry's own dispatch.
    pub(crate) fn event_type(self) -> &'static str {
        match self {
            Self::Created => "TriggerCreated",
            Self::Updated => "TriggerUpdated",
            Self::Deleted => "TriggerDeleted",
            Self::Disabled => "TriggerDisabled",
        }
    }

    fn into_event(
        self,
        trigger_id: String,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> SystemEvent {
        match self {
            Self::Created => SystemEvent::TriggerCreated {
                trigger_id,
                payload,
                actor,
            },
            Self::Updated => SystemEvent::TriggerUpdated {
                trigger_id,
                payload,
                actor,
            },
            Self::Deleted => SystemEvent::TriggerDeleted {
                trigger_id,
                payload,
                actor,
            },
            Self::Disabled => SystemEvent::TriggerDisabled {
                trigger_id,
                payload,
                actor,
            },
        }
    }
}

/// Everything a trigger write touches: where the event goes, and the two
/// halves of the *trigger registry* it must leave consistent. Grouped because
/// they always travel together, and because the Thread Queue holds exactly this
/// trio without holding a strong engine handle.
pub(crate) struct TriggerRegistryWriter<'a> {
    pub event_bus: &'a EventBus,
    pub trigger_configs: &'a RwLock<HashMap<String, TriggerConfig>>,
    pub workspace_path: &'a Path,
    /// Serializes emit + apply so the registry is written in the order the log
    /// records. See `LucidosEngine::trigger_write_lock` for what goes wrong
    /// without it.
    pub write_lock: &'a tokio::sync::Mutex<()>,
}

impl TriggerRegistryWriter<'_> {
    /// Emit a trigger lifecycle event and materialize it into the registry
    /// before returning, so the caller's next read observes its own write.
    ///
    /// A failed emit propagates and applies nothing. Arms no cron job: that
    /// stays with the scheduler's subscriber, which owns `tracked_tasks` and
    /// reacts to the same broadcast.
    pub(crate) async fn write(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Held across both steps: `emit` broadcasts before it returns, so an
        // unguarded writer can be preempted between emitting and applying,
        // and then land its older payload on top of a newer writer's.
        let _guard = self.write_lock.lock().await;
        let event = write.into_event(trigger_id.to_string(), payload.clone(), actor);
        self.event_bus.emit(BusEvent::System(event)).await?;
        materialize_trigger_event(
            self.trigger_configs,
            self.workspace_path,
            write.event_type(),
            trigger_id,
            &payload,
        );
        Ok(())
    }

    /// Fire-and-forget [`write`](Self::write). `module` is the per-call-site log
    /// tag, e.g. `"[Triggers]"`; the event name is appended, so failures land as
    /// `[EventBus] [Triggers] TriggerUpdated emit failed: …` and stay greppable
    /// by module.
    pub(crate) async fn write_or_log(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
        module: &str,
    ) {
        if let Err(e) = self.write(write, trigger_id, payload, actor).await {
            crate::log!(
                "[EventBus] {} {} emit failed: {}",
                module,
                write.event_type(),
                e
            );
        }
    }
}

impl LucidosEngine {
    /// The engine's own bus, registry and workspace as a [`TriggerRegistryWriter`].
    pub(crate) fn trigger_registry_writer(&self) -> TriggerRegistryWriter<'_> {
        TriggerRegistryWriter {
            event_bus: &self.event_bus,
            trigger_configs: &self.trigger_configs,
            workspace_path: &self.workspace_path,
            write_lock: &self.trigger_write_lock,
        }
    }

    /// Sugar over [`TriggerRegistryWriter::write`].
    ///
    /// Pairs with [`emit_trigger_write_or_log`](Self::emit_trigger_write_or_log)
    /// exactly as `EventBus::emit` pairs with `emit_or_log`: take the `Result`
    /// when the caller can report the failure, take the logging variant when it
    /// cannot.
    pub(crate) async fn emit_trigger_write(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.trigger_registry_writer()
            .write(write, trigger_id, payload, actor)
            .await
    }

    /// Sugar over [`TriggerRegistryWriter::write_or_log`].
    pub(crate) async fn emit_trigger_write_or_log(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
        module: &str,
    ) {
        self.trigger_registry_writer()
            .write_or_log(write, trigger_id, payload, actor, module)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::registry::TRIGGER_LIFECYCLE_EVENTS;

    const ALL_WRITES: [TriggerWrite; 4] = [
        TriggerWrite::Created,
        TriggerWrite::Updated,
        TriggerWrite::Deleted,
        TriggerWrite::Disabled,
    ];

    /// Every `TriggerWrite` must name an event the registry actually
    /// materializes, or a write would persist and change nothing in memory.
    /// The reverse does not hold: `TriggerEnabled` is materialized for replay
    /// of old logs but nothing writes it, which is why it has no variant.
    #[test]
    fn every_write_names_an_event_the_registry_applies() {
        let named: Vec<&str> = ALL_WRITES.iter().map(|k| k.event_type()).collect();
        for kind in ALL_WRITES {
            assert!(
                TRIGGER_LIFECYCLE_EVENTS.contains(&kind.event_type()),
                "{:?} names {}, which the registry ignores",
                kind,
                kind.event_type()
            );
        }
        let unwritten: Vec<&str> = TRIGGER_LIFECYCLE_EVENTS
            .into_iter()
            .filter(|e| !named.contains(e))
            .collect();
        assert_eq!(
            unwritten,
            vec!["TriggerEnabled"],
            "a lifecycle event with no TriggerWrite can only be reached by replay; \
             add a variant if something now writes it"
        );
    }

    /// The event a write builds must carry the id and payload it was given, and
    /// must report the same `event_type` the registry will dispatch on.
    #[test]
    fn a_write_builds_the_event_it_names() {
        for kind in ALL_WRITES {
            let event = kind.into_event(
                "t-1".to_string(),
                serde_json::json!({ "trigger_id": "t-1" }),
                None,
            );
            assert_eq!(event.event_type(), kind.event_type());
            assert_eq!(event.aggregate_id(), "t-1");
        }
    }

    /// Two writers race on one trigger. Every write must land, and the registry
    /// must end where the *event log* ends, not wherever the last writer to be
    /// scheduled happened to leave it.
    ///
    /// **What this does and does not prove.** It asserts against the
    /// highest-sequence event in the log rather than an expected value, so it
    /// catches any coarse reordering (an apply moved off the write path, a
    /// dropped write, a torn merge). It does NOT reliably reproduce the
    /// hazard the write lock exists for: that window is the few instructions
    /// between `emit` broadcasting and the caller applying, with no await in
    /// between, so hitting it needs the writer's thread to be descheduled
    /// there while the other writer completes three DB round-trips. Multi-
    /// threaded so the window is at least open; the guarantee itself comes
    /// from the lock holding the pair, not from this test going green.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_leave_the_registry_where_the_log_ends() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        use crate::triggers::TriggerConfig;
        use std::collections::HashMap;
        use std::sync::{Arc, RwLock};

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = Arc::new(tempfile::tempdir().unwrap());
        let configs: Arc<RwLock<HashMap<String, TriggerConfig>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let write_lock = Arc::new(tokio::sync::Mutex::new(()));
        let bus = Arc::new(bus);

        TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        }
        .write(
            TriggerWrite::Created,
            "t-1",
            serde_json::json!({
                "trigger_id": "t-1",
                "name": "Start",
                "slug": "racy",
                "schedule": ["0 0 2 * * *"],
                "timezone": "UTC",
                "run": { "type": "intent", "intent": "x" },
            }),
            None,
        )
        .await
        .expect("create");

        // Two writers, interleaved renames. Each await point is a chance for
        // the other to slip between an emit and its apply.
        let mut handles = Vec::new();
        for writer_id in 0..2 {
            let (bus, configs, ws, write_lock) =
                (bus.clone(), configs.clone(), ws.clone(), write_lock.clone());
            handles.push(tokio::spawn(async move {
                for round in 0..8 {
                    TriggerRegistryWriter {
                        event_bus: &bus,
                        trigger_configs: &configs,
                        workspace_path: ws.path(),
                        write_lock: &write_lock,
                    }
                    .write(
                        TriggerWrite::Updated,
                        "t-1",
                        serde_json::json!({
                            "trigger_id": "t-1",
                            "name": format!("w{writer_id}-r{round}"),
                        }),
                        None,
                    )
                    .await
                    .expect("update");
                }
            }));
        }
        for h in handles {
            h.await.expect("writer task panicked");
        }

        let last_logged: String = sqlx::query_scalar(
            "SELECT payload->>'name' FROM events \
             WHERE event_type = 'TriggerUpdated' AND aggregate_id = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind("t-1")
        .fetch_one(&pool)
        .await
        .expect("last logged name");

        assert_eq!(
            configs.read().unwrap()["t-1"].name,
            last_logged,
            "the registry must agree with the newest event in the log, not with \
             whichever writer applied last"
        );
        teardown_test_db(&db).await;
    }

    /// Source-scan tripwire, in the repo's existing idiom (see
    /// `core::announced_surfaces_tests`). A new emitter that builds a trigger
    /// lifecycle event and hands it straight to the bus compiles cleanly,
    /// passes review, and reintroduces the stale-read race, because nothing
    /// about the call site looks wrong. The only reliable guard is mechanical:
    /// the constructors live here, and nowhere else.
    #[test]
    fn only_the_chokepoint_constructs_a_trigger_lifecycle_event() {
        use crate::test_support::source_scan::production_sources;

        // The subscriber has to name the variants to *destructure* them off the
        // broadcast; it never builds one.
        const ALLOWED: [&str; 2] = ["engine/trigger_writes.rs", "scheduler/mod.rs"];

        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in production_sources() {
            if ALLOWED.contains(&rel.as_str()) {
                continue;
            }
            for event in TRIGGER_LIFECYCLE_EVENTS {
                // `SystemEvent::TriggerX {` is construction; a match arm spells
                // `SystemEvent::TriggerX { .. }` and carries no field.
                let ctor = format!("SystemEvent::{} {{", event);
                if text.contains(&ctor) && !text.contains(&format!("{} .. }}", ctor)) {
                    offenders.push(format!("{rel}: {event}"));
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "a trigger lifecycle event must be written through \
             LucidosEngine::emit_trigger_write (or the free emit_trigger_write), \
             so the registry is consistent before the caller's next read. \
             Constructed outside the chokepoint in:\n  {}",
            offenders.join("\n  ")
        );
    }
}
