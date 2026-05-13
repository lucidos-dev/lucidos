//! `LucidosEngine` event emit helpers for the changes lifecycle. Each
//! method wraps `event_bus.emit_or_log` for one ChangeProposed-related
//! `ThreadEvent` (or the `ChangesUpdated` system event), so the call sites
//! in `change_ops.rs` stay free of the boilerplate.

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// Broadcast the current changes state (pending/applied/restart) to all SSE clients.
    pub(crate) async fn broadcast_changes_updated(&self) {
        let proj = self.changes();
        let mut pending = proj.list_pending().await;
        let mut applied = proj.list_recently_applied(15, None).await;
        // UNIX_EPOCH ≈ "since forever" for restart-required tracking. Don't use
        // `DateTime::MIN_UTC` — its year (-262143) is outside Postgres
        // timestamptz range (4713 BC … 294276 AD), so binding it errors with
        // "timestamp out of range" and the helper falls back to false, hiding
        // any actual restart-required change.
        let restart = proj
            .requires_restart_since(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
            .await;
        let (r1, r2) = tokio::join!(
            crate::core::changes::enrich_thread_titles(self.pool(), &mut pending),
            crate::core::changes::enrich_thread_titles(self.pool(), &mut applied),
        );
        if let Err(e) = r1 {
            log!("[Changes] enrich pending titles: {}", e);
        }
        if let Err(e) = r2 {
            log!("[Changes] enrich applied titles: {}", e);
        }
        self.event_bus
            .emit_or_log(
                BusEvent::System(
                    SystemEvent::ChangesUpdated {
                        total_pending: pending.len(),
                        pending,
                        applied,
                        restart_required: restart,
                    },
                ),
                "[Changes] ChangesUpdated",
            )
            .await;
    }

    /// Emit `MergeResolutionCleared` for a change whose merge worktree was
    /// just torn down. The projection updates from the event.
    /// `log_tag` is forwarded to `emit_or_log` for failure observability.
    pub(crate) async fn emit_merge_resolution_cleared(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        log_tag: &str,
    ) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::MergeResolutionCleared {
                        change_id: change_id.to_string(),
                    },
                    meta: EventMeta::NONE,
                },
                log_tag,
            )
            .await;
    }

    /// Emit `ChangeApplyFailed` with the standard "hardening did not
    /// complete" message — used by the two post-hardening gates
    /// (`apply_now_inner` after the in-session run, `spawn_hardening_session`
    /// after the spawned run) to bail out when the marker never landed.
    pub(crate) async fn emit_apply_failed_unhardened(
        &self,
        thread_id: Uuid,
        change_id: &str,
        actor: Option<MessageOrigin>,
        log_tag: &str,
    ) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        change_id: change_id.to_string(),
                        error: "Hardening did not complete (no marker recorded). Click Apply again to retry.".to_string(),
                        actor,
                    },
                    meta: EventMeta::NONE,
                },
                log_tag,
            )
            .await;
    }

    /// Emit `ChangeHardened` for a change whose `/harden` marker just
    /// landed on its branch. The projection updates from the event.
    pub(crate) async fn emit_change_hardened(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        log_tag: &str,
    ) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeHardened {
                        change_id: change_id.to_string(),
                        actor: None,
                    },
                    meta: EventMeta::NONE,
                },
                log_tag,
            )
            .await;
    }

    /// Emit a ChangeApplied event and mark the change as applied in the database.
    /// `commits` and `thread_title` are surfaced in the restart-required toast,
    /// grouped by thread. `actor` identifies who initiated the apply — HTTP
    /// callers should pass `Some` (built via `api::actor::build_message_origin`);
    /// engine-internal applies pass `None`. If `thread_title` is `None`,
    /// looks it up from `thread_summaries` so the persisted event payload
    /// always carries it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit_change_applied(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        commits: Vec<String>,
        thread_title: Option<String>,
        actor: Option<MessageOrigin>,
        pre_merge_sha: Option<String>,
        post_merge_sha: Option<String>,
    ) {
        let thread_title = match thread_title {
            Some(t) => Some(t),
            None => sqlx::query_scalar::<_, Option<String>>(
                "SELECT title FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(self.pool())
            .await
            .ok()
            .flatten()
            .flatten(),
        };
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplied {
                        change_id: change_id.to_string(),
                        requires_restart,
                        client_update,
                        commits,
                        thread_title,
                        actor,
                        pre_merge_sha,
                        post_merge_sha,
                        path: String::new(),
                    },
                    meta: EventMeta::NONE,
                },
                "[Changes] ChangeApplied",
            )
            .await;
    }

    /// Emit a ChangeApplyFailed event so the frontend knows the apply didn't succeed
    /// and can keep the thread in "waiting" state with the Apply/Discard panel visible.
    /// `actor` carries the same value passed to the originating apply call.
    pub(crate) async fn emit_apply_failed(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        error: &str,
        actor: Option<MessageOrigin>,
    ) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        change_id: change_id.to_string(),
                        error: error.to_string(),
                        actor,
                    },
                    meta: EventMeta::NONE,
                },
                "[Changes] ChangeApplyFailed",
            )
            .await;
    }

    /// Emit the boundary event that opens a fresh exchange panel for the
    /// hardening run (so its steps don't attach to the previous CC turn).
    ///
    /// The event is always stamped with `MessageOrigin::Engine` because the
    /// *engine* is what detected the missing harden marker — even when a user
    /// click was the proximate trigger. This drives the route popover's actor
    /// chip to "Lucidos Engine".
    pub(crate) async fn emit_missing_hardening_detected(&self, thread_id: Uuid) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::MissingHardeningDetected {
                        origin: Some(MessageOrigin::engine(
                            EngineReason::MissingHardening,
                        )),
                    },
                    meta: EventMeta {
                        channel: Some(EventChannel::CodingAgent),
                        ..EventMeta::NONE
                    },
                },
                "[Changes] MissingHardeningDetected",
            )
            .await;
    }

    /// Emit the boundary event that opens a fresh panel for a merge run.
    ///
    /// Always stamped with `MessageOrigin::Engine` — the engine is what
    /// detected the conflict, regardless of who triggered the apply.
    pub(crate) async fn emit_merge_conflict_detected(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        files: Vec<String>,
    ) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::MergeConflictDetected {
                        change_id: change_id.to_string(),
                        files,
                        origin: Some(MessageOrigin::engine(
                            EngineReason::MergeConflict,
                        )),
                    },
                    meta: EventMeta {
                        channel: Some(EventChannel::CodingAgent),
                        ..EventMeta::NONE
                    },
                },
                "[Changes] MergeConflictDetected",
            )
            .await;
    }
}
