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
        let result = engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: child_thread_id,
                event,
                meta: EventMeta {
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
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
    fn queued_trigger_config(&self, trigger_id: &str) -> Option<crate::triggers::TriggerConfig> {
        let config = self
            .trigger_configs
            .read()
            .ok()
            .and_then(|c| c.get(trigger_id).cloned());
        if config.is_none() {
            log!(
                "[ThreadQueue] Trigger {} no longer exists — skipping queued fire",
                trigger_id
            );
        }
        config
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
                let inner = user_tasks::EVENT_TRIGGER_DEPTH.scope(
                    depth,
                    user_tasks::execute_user_task(
                        self.clone(),
                        self.pool(),
                        &config,
                        invocation,
                        Some(&event_payload),
                        entry.cancel,
                    ),
                );
                let result = match origin_thread_id {
                    Some(tid) => user_tasks::ORIGIN_THREAD_ID.scope(tid, inner).await,
                    None => inner.await,
                };
                self.record_trigger_executed(
                    &config.id,
                    crate::triggers::TriggerRunStatus::from_success(result.is_ok()),
                )
                .await;
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
                let result = user_tasks::execute_user_task(
                    self.clone(),
                    self.pool(),
                    &config,
                    TriggerInvocation::Schedule,
                    None,
                    entry.cancel,
                )
                .await;
                crate::scheduler::ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::Relaxed);
                // Record after execution so crash mid-task → catch-up re-executes.
                self.record_trigger_executed(
                    &config.id,
                    crate::triggers::TriggerRunStatus::from_success(result.is_ok()),
                )
                .await;
                if let Err(e) = result {
                    log!("[Scheduler] Task '{}' execution failed: {}", config.name, e);
                }
            }
            ThreadQueueRequest::SubThread {
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
                    Err(e) => {
                        log!("[ThreadQueue] Agent chat spawn failed: {}", e);
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                            error: e.to_string(),
                                        },
                                    meta: EventMeta::NONE,
                                },
                                "[ThreadQueue] ResponseFailed (agent chat)",
                            )
                            .await;
                    }
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
            origin: spawning_thread_link(spawning_thread),
        };
        let (_, params) =
            coding_agent_spawn_params(request, |_| None).expect("CodingAgent produces params");
        assert_eq!(params.parent_thread_id, Some(spawning_thread));
        assert_eq!(params.spawning_event_id, Some(tool_call));
        assert_eq!(params.origin, spawning_thread_link(spawning_thread));
    }
}
