//! Execution side of the Thread Queue: turns an admitted
//! [`ThreadQueueRequest`] back into the spawn it describes.
//!
//! The [`ThreadQueueExecutor`] trait is the manager's seam — production
//! installs [`EngineThreadQueueExecutor`] (via `LucidosEngine::set_self_arc`),
//! tests install mocks so admission mechanics are testable without an LLM.

use std::sync::Weak;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::ThreadQueueRequest;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, TriggerInvocation};
use crate::engine::LucidosEngine;
use crate::scheduler::user_tasks;

/// An admitted entry handed to the executor. The manager completes the
/// capacity slot when [`ThreadQueueExecutor::execute`] resolves (or panics).
pub struct ExecutableEntry {
    pub id: Uuid,
    pub request: ThreadQueueRequest,
    /// Cooperative cancel from the submitter (cron task loop); `None` for
    /// every other kind and for entries re-queued across a restart.
    pub cancel: Option<CancellationToken>,
}

#[async_trait]
pub trait ThreadQueueExecutor: Send + Sync {
    /// Admission-time hook, awaited inline before `ThreadQueueAdmitted` is
    /// emitted and before [`Self::execute`] is spawned. Used by the
    /// sub-thread kind to emit its eager `MessageReceived` so the parent's
    /// `active_children_count` increments before the spawning tool call
    /// returns — same ordering the pre-queue `run_thread` path guaranteed.
    async fn prepare(&self, _request: &mut ThreadQueueRequest) {}

    /// Run the entry's work to completion (any outcome). Runs inside a
    /// spawned task; resolving releases the entry's capacity slot.
    async fn execute(&self, entry: ExecutableEntry);
}

/// Production executor — dispatches to the same engine paths the spawn
/// sites used before admission control existed.
pub(crate) struct EngineThreadQueueExecutor {
    engine: Weak<LucidosEngine>,
}

impl EngineThreadQueueExecutor {
    pub(crate) fn new(engine: Weak<LucidosEngine>) -> Self {
        Self { engine }
    }
}

/// The eager `MessageReceived` the Thread Queue emits at admission for a
/// sub-thread spawn. `None` for every other request kind.
///
/// Split out of [`ThreadQueueExecutor::prepare`] so the linkage-versus-origin
/// split is testable without an engine: a `relation: "top"` request must
/// produce an event with `parent_thread_id: None` (no callback, no count bump)
/// AND a `ThreadLink` origin naming the spawning thread.
pub(crate) fn eager_sub_thread_message(
    workspace: &std::path::Path,
    request: &ThreadQueueRequest,
) -> Option<crate::engine::thread_events::ThreadEvent> {
    let ThreadQueueRequest::SubThread {
        prompt,
        parent_thread_id,
        spawning_event_id,
        model,
        reasoning_effort,
        origin,
        ..
    } = request
    else {
        return None;
    };
    Some(crate::engine::chat::make_message_received(
        workspace,
        prompt,
        None,
        None,
        None,
        *parent_thread_id,
        *spawning_event_id,
        ActorMode::Agent,
        model.as_deref(),
        reasoning_effort.as_deref(),
        origin.clone(),
        None,
    ))
}

/// Map an admitted `CodingAgent` entry onto its pre-allocated thread id plus its
/// spawn params. `None` for every other request kind. Split out for the same
/// reason as [`eager_sub_thread_message`]: it is where a top spawn's attribution
/// would be silently dropped, and a test can check it without booting a session.
///
/// `resolve_images` turns the persisted content-addressed hashes back into
/// inline images (the engine reads them from the blob store); a test passes a
/// closure returning `None`.
pub(crate) fn coding_agent_spawn_params(
    request: ThreadQueueRequest,
    resolve_images: impl FnOnce(&[String]) -> Option<Vec<crate::api::ChatImage>>,
) -> Option<(Uuid, crate::engine::claude_code::SpawnAgentThreadParams)> {
    let ThreadQueueRequest::CodingAgent {
        // The queue scopes the execution at this depth; the spawn params
        // carry none.
        depth: _,
        prompt,
        cc_thread_id,
        image_hashes,
        device_id,
        parent_thread_id,
        spawning_event_id,
        repo_id,
        title,
        app_id,
        coding_agent,
        model,
        reasoning_effort,
        origin,
    } = request
    else {
        return None;
    };
    Some((
        cc_thread_id,
        crate::engine::claude_code::SpawnAgentThreadParams {
            prompt,
            user_images: resolve_images(&image_hashes),
            device_id,
            parent_thread_id,
            spawning_event_id,
            repo_id,
            caller_title: title,
            app_id,
            coding_agent,
            model,
            reasoning_effort,
            origin,
        },
    ))
}

#[async_trait]
impl ThreadQueueExecutor for EngineThreadQueueExecutor {
    async fn prepare(&self, request: &mut ThreadQueueRequest) {
        // Read what the decision needs through a shared borrow, so the event
        // can be built from `request` before the mutable write-back below.
        let ThreadQueueRequest::SubThread {
            child_thread_id,
            pre_emitted_origin,
            ..
        } = &*request
        else {
            return;
        };
        if pre_emitted_origin.is_some() {
            return; // already emitted (re-admission after a failed emit retry)
        }
        let child_thread_id = *child_thread_id;
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        let Some(event) = eager_sub_thread_message(engine.workspace_path(), request) else {
            return;
        };
        // Eager MessageReceived — a Child-relation child's
        // active_children_count must increment before the parent can finish
        // its turn, otherwise ResponseGenerated wins the race and the parent
        // flips to "review" before the child is on the projection. On emit
        // failure the spawn path emits its own MessageReceived instead
        // (pre_emitted_origin stays None).
        //
        // `None` is the child's own provenance, not the submitter's. This runs
        // inline on whichever task submitted. A trigger fire spawning a
        // sub-thread would otherwise stamp its own marker here, and never wake
        // on the child it asked for. See `EventBus::emit_as_trigger`.
        let result = engine
            .event_bus
            .emit_as_trigger(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id: child_thread_id,
                    event,
                    meta: EventMeta {
                        channel: Some(EventChannel::Chat),
                        ..EventMeta::NONE
                    },
                },
                None,
            )
            .await;
        let ThreadQueueRequest::SubThread {
            pre_emitted_origin, ..
        } = request
        else {
            return; // unreachable: the same variant matched above
        };
        match result {
            Ok(Some(emit)) => *pre_emitted_origin = Some(emit.event_id),
            Ok(None) => log!("[ThreadQueue] sub-thread MessageReceived emit returned no result"),
            Err(e) => log!(
                "[ThreadQueue] sub-thread MessageReceived emit failed: {}",
                e
            ),
        }
    }

    async fn execute(&self, entry: ExecutableEntry) {
        let Some(engine) = self.engine.upgrade() else {
            log!(
                "[ThreadQueue] Engine gone — entry {} not executed",
                entry.id
            );
            return;
        };
        engine.execute_thread_queue_entry(entry).await;
    }
}

impl LucidosEngine {
    /// Look up a live trigger config; `None` (logged) when the trigger was
    /// deleted while the entry waited.
    ///
    /// An unreadable registry is logged as its own cause. Both skip the fire,
    /// but they send an operator to different places, and `.ok()` reported
    /// either one as a deletion.
    fn queued_trigger_config(&self, trigger_id: &str) -> Option<crate::triggers::TriggerConfig> {
        match self.trigger_configs.read() {
            Ok(configs) => {
                let config = configs.get(trigger_id).cloned();
                if config.is_none() {
                    log!(
                        "[ThreadQueue] Trigger {} no longer exists, skipping its queued fire",
                        trigger_id
                    );
                }
                config
            }
            Err(e) => {
                log!(
                    "[ThreadQueue] Trigger registry unreadable ({}), skipping {}'s queued fire",
                    e,
                    trigger_id
                );
                None
            }
        }
    }

    /// Run one admitted Thread Queue entry. Mirrors the pre-queue spawn
    /// sites exactly — this is dispatch, not new behavior.
    pub(crate) async fn execute_thread_queue_entry(
        self: std::sync::Arc<Self>,
        entry: ExecutableEntry,
    ) {
        use std::sync::atomic::Ordering;

        match entry.request {
            ThreadQueueRequest::EventTrigger {
                trigger_id,
                event_type,
                event_payload,
                depth,
                origin_thread_id,
                source_event_id,
            } => {
                let Some(config) = self.queued_trigger_config(&trigger_id) else {
                    return;
                };
                log!(
                    "[Scheduler] Firing event trigger '{}' for event '{}'",
                    config.name,
                    event_type
                );
                let invocation = TriggerInvocation::Event {
                    event_type,
                    event_id: source_event_id,
                    thread_id: origin_thread_id,
                };
                crate::scheduler::ACTIVE_TASK_COUNT.fetch_add(1, Ordering::Relaxed);
                debug_assert_eq!(
                    user_tasks::current_event_trigger_depth(),
                    depth,
                    "the queue scopes the whole execution at the request's depth"
                );
                // The run AND the `TriggerExecuted` that closes it are both
                // links in this fire's chain, so both run inside the scope
                // the queue established at the request's depth. Recording
                // outside it stamped depth 0, and a trigger subscribed to
                // `TriggerExecuted` re-fired unbounded.
                //
                // `ACTIVE_TRIGGER_ID` wraps the same pair, for the same reason:
                // every event either half emits is this trigger's own, so the
                // matcher must be able to see whose fire it was. It stays
                // here rather than moving to the queue beside the depth. It
                // must NOT reach the work a fire hands off
                // (`docs/adr/0137-a-trigger-never-wakes-itself.md`), and the
                // depth must.
                let inner = user_tasks::ACTIVE_TRIGGER_ID.scope(config.id.clone(), async {
                    let result = user_tasks::execute_user_task(
                        self.clone(),
                        self.pool(),
                        &config,
                        invocation,
                        Some(&event_payload),
                        entry.cancel,
                        entry.id,
                    )
                    .await;
                    self.record_trigger_executed(
                        &config.id,
                        crate::triggers::TriggerRunStatus::from_success(result.is_ok()),
                    )
                    .await;
                    result
                });
                let result = match origin_thread_id {
                    Some(tid) => user_tasks::ORIGIN_THREAD_ID.scope(tid, inner).await,
                    None => inner.await,
                };
                crate::scheduler::ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::Relaxed);
                if let Err(e) = result {
                    log!("[Scheduler] Event trigger '{}' failed: {}", config.name, e);
                }
            }
            ThreadQueueRequest::Cron { trigger_id } => {
                let Some(config) = self.queued_trigger_config(&trigger_id) else {
                    return;
                };
                if config.paused {
                    // Paused while queued (or between drain and execute) —
                    // the fire waits for nobody; the next schedule occurrence
                    // happens after resume.
                    log!(
                        "[ThreadQueue] Trigger '{}' paused — skipping queued scheduled fire",
                        config.name
                    );
                    return;
                }
                let active =
                    crate::scheduler::ACTIVE_TASK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if active > 1 {
                    log!(
                        "[Scheduler] Concurrent execution: {} tasks now active (starting '{}')",
                        active,
                        config.name
                    );
                }
                // A cron fire has no chain depth to carry, but it owns every
                // event it emits just as an event fire does. The scope covers
                // the run AND the `TriggerExecuted` that closes it.
                let result = user_tasks::ACTIVE_TRIGGER_ID
                    .scope(config.id.clone(), async {
                        let result = user_tasks::execute_user_task(
                            self.clone(),
                            self.pool(),
                            &config,
                            TriggerInvocation::Schedule,
                            None,
                            entry.cancel,
                            entry.id,
                        )
                        .await;
                        crate::scheduler::ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::Relaxed);
                        // Record after execution so crash mid-task → catch-up re-executes.
                        self.record_trigger_executed(
                            &config.id,
                            crate::triggers::TriggerRunStatus::from_success(result.is_ok()),
                        )
                        .await;
                        result
                    })
                    .await;
                if let Err(e) = result {
                    log!("[Scheduler] Task '{}' execution failed: {}", config.name, e);
                }
            }
            ThreadQueueRequest::SubThread {
                depth: _, // the queue already scoped this task at it
                prompt,
                child_thread_id,
                parent_thread_id,
                spawning_event_id,
                title,
                model,
                reasoning_effort,
                pre_emitted_origin,
                origin,
            } => {
                match self.spawn_thread(
                    &prompt,
                    parent_thread_id,
                    spawning_event_id,
                    child_thread_id,
                    pre_emitted_origin,
                    title.as_deref(),
                    model,
                    reasoning_effort,
                    origin,
                ) {
                    Ok((_, handle)) => {
                        // The processing task is the work — awaiting it is what
                        // holds this entry's capacity slot until the sub-thread
                        // finishes its turn.
                        if let Err(e) = handle.await {
                            log!(
                                "[ThreadQueue] Sub-thread {} task join failed: {:?}",
                                child_thread_id,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        log!(
                            "[ThreadQueue] Sub-thread {} spawn failed: {}",
                            child_thread_id,
                            e
                        );
                    }
                }
            }
            request @ ThreadQueueRequest::CodingAgent { .. } => {
                let Some((cc_thread_id, params)) = coding_agent_spawn_params(request, |hashes| {
                    self.resolve_queued_image_hashes(hashes)
                }) else {
                    return; // unreachable: the same variant matched above
                };
                // Inner spawn + watcher: monitor_cc_task owns the panic
                // cleanup (ResponseFailed + SessionEnded + session removal);
                // awaiting the watcher holds the capacity slot until both the
                // session AND any panic cleanup finish.
                let inner = tokio::spawn(self.clone().run_agent_thread_spawn(params, cc_thread_id));
                let watcher = Self::monitor_cc_task(self.clone(), cc_thread_id, inner);
                if let Err(e) = watcher.await {
                    log!(
                        "[ThreadQueue] Coding-agent {} watcher join failed: {:?}",
                        cc_thread_id,
                        e
                    );
                }
            }
            ThreadQueueRequest::AgentChat {
                depth: _, // the queue already scoped this task at it
                message,
                thread_id,
                event_id,
                image_hashes,
                device_id,
                model,
                reasoning_effort,
                use_coding_agent,
                repo_id,
                cc_model,
                coding_agent,
                title,
                mode,
                origin,
                parent_thread_id,
                spawning_event_id,
                app_id,
            } => {
                if let Some(app) = app_id {
                    match self.pending_app_spawn.lock() {
                        Ok(mut guard) => {
                            guard.insert(thread_id, app);
                        }
                        Err(e) => {
                            log!("[ThreadQueue] pending_app_spawn poisoned: {}", e);
                        }
                    }
                }
                let images = self.resolve_queued_image_hashes(&image_hashes);
                let result = self
                    .process_message_with_steps(
                        &message,
                        model.as_deref(),
                        None,
                        None,
                        reasoning_effort.as_deref(),
                        images.as_deref(),
                        device_id.as_deref(),
                        use_coding_agent,
                        event_id.as_deref(),
                        Some(thread_id),
                        None,
                        repo_id.as_deref(),
                        None,
                        parent_thread_id,
                        spawning_event_id,
                        mode,
                        cc_model.as_deref(),
                        coding_agent,
                        None,
                        title.as_deref(),
                        origin,
                        None,
                        crate::engine::FollowUpUrgency::Normal,
                    )
                    .await;
                match result {
                    Ok(res) => {
                        if res.proposed_change {
                            if res.auto_apply {
                                self.auto_apply_proposed_change(res.request_id, thread_id, None)
                                    .await;
                            }
                            self.broadcast_changes_updated().await;
                        }
                        if !res.orphaned_injections.is_empty() {
                            crate::api::chat::process_orphan_chain(
                                self.clone(),
                                res.thread_id,
                                res.orphaned_injections,
                            )
                            .await;
                        }
                    }
                    // No terminator here: the turn settles its own exchange,
                    // anchored. An unanchored copy is what the idempotency
                    // gate cannot match, so it double-fired.
                    Err(e) => log!("[ThreadQueue] Agent chat spawn failed: {}", e),
                }
            }
        }
    }

    /// Convert in-memory `ChatImage`s into content-addressed blob hashes so
    /// a queue request stays persistable (no base64 in event payloads —
    /// same rule as `MessageReceived.user_image_hashes`). Content-addressed
    /// writes are idempotent, so images that already live in the blob store
    /// are not duplicated. Unwritable images are dropped with a log line.
    pub(crate) fn queued_image_hashes(
        &self,
        images: Option<&[crate::api::ChatImage]>,
    ) -> Vec<String> {
        let Some(images) = images else {
            return Vec::new();
        };
        images
            .iter()
            .filter_map(|img| {
                match crate::core::blobs::write_blob_from_base64(self.workspace_path(), &img.base64)
                {
                    Ok(blob) => Some(blob.hash),
                    Err(e) => {
                        log!(
                            "[ThreadQueue] failed to persist image blob for queued spawn: {}",
                            e
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// Resolve content-addressed image hashes back to inline `ChatImage`s.
    /// Missing blobs are dropped with a log line (same policy as
    /// `chat_submit`'s `image_hashes` resolution).
    fn resolve_queued_image_hashes(&self, hashes: &[String]) -> Option<Vec<crate::api::ChatImage>> {
        if hashes.is_empty() {
            return None;
        }
        let mut resolved = Vec::with_capacity(hashes.len());
        for hash in hashes {
            match crate::core::blobs::read_blob_as_base64(self.workspace_path(), hash) {
                Some((base64, mime_type)) => {
                    resolved.push(crate::api::ChatImage { base64, mime_type })
                }
                None => log!(
                    "[ThreadQueue] image blob {} missing on disk, dropping from queued spawn",
                    hash
                ),
            }
        }
        Some(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::{MessageOrigin, ThreadDirection, ThreadEvent};

    /// The attribution a spawn site stamps. These are plumbing tests, so the
    /// origin is opaque payload that has to arrive intact; that it matches what
    /// `spawn_origin` produces is pinned where that function lives.
    fn spawning_thread_link(spawning_thread: Uuid) -> Option<MessageOrigin> {
        Some(MessageOrigin::ThreadLink {
            thread_id: spawning_thread,
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        })
    }

    /// `run_thread` with `relation: "top"`: the admission-time `MessageReceived`
    /// must name the launching thread (so the route popover links back) while
    /// carrying no callback linkage (so nothing counts it as a child).
    #[test]
    fn eager_sub_thread_message_keeps_attribution_without_linkage() {
        let spawning_thread = Uuid::new_v4();
        let request = ThreadQueueRequest::SubThread {
            depth: 0,
            prompt: "independent work".into(),
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: None,
            spawning_event_id: None,
            title: None,
            model: None,
            reasoning_effort: None,
            pre_emitted_origin: None,
            origin: spawning_thread_link(spawning_thread),
        };
        let event =
            eager_sub_thread_message(std::path::Path::new("/tmp/lucidos-test-ws"), &request)
                .expect("SubThread produces an event");
        let ThreadEvent::MessageReceived {
            origin,
            parent_thread_id,
            spawning_event_id,
            ..
        } = event
        else {
            panic!("expected MessageReceived");
        };
        assert_eq!(origin, spawning_thread_link(spawning_thread));
        assert_eq!(parent_thread_id, None);
        assert_eq!(spawning_event_id, None);
    }

    #[test]
    fn eager_sub_thread_message_is_only_for_sub_threads() {
        let request = ThreadQueueRequest::Cron {
            trigger_id: "t".into(),
        };
        assert!(
            eager_sub_thread_message(std::path::Path::new("/tmp/lucidos-test-ws"), &request)
                .is_none()
        );
    }

    /// The `run_coding_agent` half of the same invariant, at the hop where a
    /// top spawn's attribution would otherwise be dropped on the floor.
    #[test]
    fn coding_agent_spawn_params_keep_attribution_without_linkage() {
        let spawning_thread = Uuid::new_v4();
        let cc_thread_id = Uuid::new_v4();
        let request = ThreadQueueRequest::CodingAgent {
            depth: 0,
            prompt: "independent work".into(),
            cc_thread_id,
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            model: None,
            reasoning_effort: None,
            origin: spawning_thread_link(spawning_thread),
        };
        let (id, params) =
            coding_agent_spawn_params(request, |_| None).expect("CodingAgent produces params");
        assert_eq!(id, cc_thread_id);
        assert_eq!(params.origin, spawning_thread_link(spawning_thread));
        assert_eq!(params.parent_thread_id, None);
        assert_eq!(params.spawning_event_id, None);
    }

    /// A child spawn keeps both halves: the same origin AND the linkage that
    /// drives the callback and the parent's child count.
    #[test]
    fn coding_agent_spawn_params_keep_a_child_spawn_intact() {
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        let request = ThreadQueueRequest::CodingAgent {
            depth: 0,
            prompt: "delegated work".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: Some(spawning_thread),
            spawning_event_id: Some(tool_call),
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            model: None,
            reasoning_effort: None,
            origin: spawning_thread_link(spawning_thread),
        };
        let (_, params) =
            coding_agent_spawn_params(request, |_| None).expect("CodingAgent produces params");
        assert_eq!(params.parent_thread_id, Some(spawning_thread));
        assert_eq!(params.spawning_event_id, Some(tool_call));
        assert_eq!(params.origin, spawning_thread_link(spawning_thread));
    }

    /// THE hop that dropped the model. `ThreadQueueRequest::CodingAgent` had no
    /// field for it, so a caller's `model: "claude-sonnet-5"` died at the tool
    /// boundary and the session took the `cc-settings.json` default. Nothing
    /// failed: the spawn returned success and the executor card showed Opus
    /// under a spawn call that said Sonnet.
    ///
    /// This is a pure function over the request, so the carry can be pinned
    /// without booting a session. If it ever regresses, it regresses here.
    #[test]
    fn coding_agent_spawn_params_carry_the_model_and_effort_pins() {
        let request = ThreadQueueRequest::CodingAgent {
            depth: 0,
            prompt: "a single-file shell edit".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            model: Some("claude-sonnet-5".into()),
            reasoning_effort: Some("low".into()),
            origin: None,
        };
        let (_, params) =
            coding_agent_spawn_params(request, |_| None).expect("CodingAgent produces params");
        assert_eq!(params.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(params.reasoning_effort.as_deref(), Some("low"));
    }

    /// An unpinned spawn must arrive unpinned, not defaulted here. The backend
    /// owns its own default, and materialising one at this hop would hide a
    /// later break in exactly the way the original bug hid.
    #[test]
    fn an_unpinned_spawn_reaches_the_backend_with_no_model() {
        let request = ThreadQueueRequest::CodingAgent {
            depth: 0,
            prompt: "unpinned".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            model: None,
            reasoning_effort: None,
            origin: None,
        };
        let (_, params) =
            coding_agent_spawn_params(request, |_| None).expect("CodingAgent produces params");
        assert_eq!(params.model, None);
        assert_eq!(params.reasoning_effort, None);
    }

    /// Both trigger arms hand `execute_user_task` the id of the entry being
    /// executed, so the fire can bind its thread to the row that admitted it.
    ///
    /// Pass any OTHER uuid and the fire updates a row it does not own. Its real
    /// entry then stays unbound and re-fires after a restart, which is the bug
    /// ADR 0133 fixed. The type system already forces an argument, and only the
    /// call site can say it is the right one. A behavior test would need a whole
    /// engine, so the two sites are pinned here, like the depth scope below.
    #[test]
    fn both_trigger_arms_pass_their_own_entry_id_to_the_fire() {
        const SRC: &str = include_str!("executor.rs");
        // Split so this line is not itself a third match.
        let needle = concat!("user_tasks::", "execute_user_task(");
        let calls: Vec<&str> = SRC
            .match_indices(needle)
            .map(|(i, _)| {
                let after = &SRC[i..];
                &after[..after.find(")\n").expect("the call closes")]
            })
            .collect();
        assert_eq!(calls.len(), 2, "one call per trigger kind");
        for call in calls {
            assert!(
                call.contains("entry.id"),
                "a trigger fire must carry its own entry id: {call}"
            );
        }
    }

    /// The frame a fire's completion writes is a link in that fire's chain.
    ///
    /// `TriggerExecuted` is persisted, so a trigger may subscribe to it.
    /// Recording it outside the fire's depth scope stamped 0, and the cap never
    /// engaged for such a trigger. The queue now owns that scope, around the
    /// whole execution. What this pins is that the completion is recorded
    /// inside the arm rather than after it. A behavior test would need a whole
    /// engine.
    #[test]
    fn the_event_arm_records_its_completion_inside_the_fires_own_future() {
        const SRC: &str = include_str!("executor.rs");
        // Split so this line is not itself the match.
        let anchor = concat!("let inner = user_tasks::", "ACTIVE_TRIGGER_ID.scope(");
        let arm = SRC
            .split_once(anchor)
            .expect("the event arm builds the fire's future")
            .1;
        let scoped = arm
            .split_once("let result = match origin_thread_id")
            .expect("the future ends where the origin-thread wrapper begins")
            .0;
        assert!(
            scoped.contains("record_trigger_executed"),
            "the completion must be recorded inside the fire's own future"
        );
    }

    /// The queue owns the depth scope, so the executor must not re-establish
    /// one. Two scopes for one value is how they drift.
    #[test]
    fn the_executor_does_not_scope_the_depth_itself() {
        const SRC: &str = include_str!("executor.rs");
        let needle = concat!("EVENT_TRIGGER_DEPTH", ".scope(");
        assert!(
            !SRC.contains(needle),
            "the Thread Queue scopes execution; see ThreadQueue::spawn_execution"
        );
    }

    /// The body of every `ACTIVE_TRIGGER_ID.scope(...)` in this file, balanced
    /// from its opening paren. String literals are skipped, so a `log!` holding
    /// a stray paren cannot throw the count off. Every index lands on an ASCII
    /// delimiter, which is always a char boundary.
    fn trigger_id_scope_bodies(src: &str) -> Vec<&str> {
        // Split so this line is not itself a match.
        let needle = concat!("user_tasks::", "ACTIVE_TRIGGER_ID");
        src.match_indices(needle)
            .map(|(i, _)| {
                let after = &src[i..];
                let open =
                    after.find(".scope(").expect("the marker opens a scope") + ".scope(".len();
                let bytes = after.as_bytes();
                let (mut depth, mut in_str, mut escaped) = (1usize, false, false);
                let mut end = None;
                for (j, &c) in bytes.iter().enumerate().skip(open) {
                    match (in_str, escaped, c) {
                        (true, true, _) => escaped = false,
                        (true, false, b'\\') => escaped = true,
                        (true, false, b'"') => in_str = false,
                        (false, _, b'"') => in_str = true,
                        (false, _, b'(') => depth += 1,
                        (false, _, b')') => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                &after[open..end.expect("the scope closes")]
            })
            .collect()
    }

    /// A trigger is never woken by an event its own fire emitted, and the
    /// matcher decides that from the marker this scope sets. Miss either arm
    /// and that trigger's fires stay self-waking, silently.
    ///
    /// The `TriggerExecuted` that closes a fire must be inside the scope too.
    /// Leave it outside and the event most likely to match a subscription goes
    /// unstamped. That is the trap the depth scope above already fell into.
    #[test]
    fn both_trigger_arms_run_the_whole_fire_under_the_trigger_id_scope() {
        let scopes = trigger_id_scope_bodies(include_str!("executor.rs"));
        assert_eq!(scopes.len(), 2, "one scope per trigger kind");
        for scope in scopes {
            assert!(
                scope.contains("execute_user_task"),
                "the fire itself must run inside the scope: {scope}"
            );
            assert!(
                scope.contains("record_trigger_executed"),
                "the closing completion must be inside the scope too: {scope}"
            );
        }
    }

    /// Every emit in this file states whose fire it belongs to. The queue
    /// executor runs `prepare` inline on whichever task submitted. A bare
    /// `emit` here would stamp the submitter's trigger onto a sub-thread's
    /// first event, costing that trigger the wake it asked for.
    #[test]
    fn the_executor_never_emits_on_the_ambient_marker() {
        // Split so the needle is not itself a match.
        let needle = concat!(".emit", "(");
        assert!(
            !include_str!("executor.rs").contains(needle),
            "state the provenance: call EventBus::emit_as_trigger instead"
        );
    }

    /// The balancer must find the real closing paren, not the first one. A
    /// scope whose body ends early would pass the test above while leaving half
    /// the fire outside the marker.
    #[test]
    fn the_scope_balancer_stops_at_the_matching_paren() {
        // Built from a split literal, so this fixture is not a third match in
        // the whole-file scan above.
        let src = format!(
            "{}.scope(id, f(x, \") not this one\"))\nafter",
            concat!("user_tasks::", "ACTIVE_TRIGGER_ID")
        );
        assert_eq!(
            trigger_id_scope_bodies(&src),
            vec!["id, f(x, \") not this one\")"]
        );
    }
}
