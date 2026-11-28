use std::collections::BTreeSet;

use uuid::Uuid;

use crate::core::AppKind;
use crate::engine::build_deferral::BuildRequest;
use crate::engine::change_ops::AppliedMerge;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::git_ops::{consume_harden_marker, consume_plan_marker, git_cmd};
use crate::engine::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::LucidosEngine;

pub(crate) const APPLIED_IN_BROADCAST: i64 = 15;

/// INVARIANT: The one window every `ChangesUpdated` frame answers `restart_required`
/// over: has any restart-requiring change been applied at all.
///
/// Don't reach for `DateTime::MIN_UTC`. Its year (-262143) sits outside the
/// Postgres timestamptz range, so binding it errors with "timestamp out of
/// range". The helper then falls back to false and hides a real
/// restart-required change.
const RESTART_REQUIRED_WINDOW: chrono::DateTime<chrono::Utc> =
    chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;

/// INVARIANT: Build the `ChangesUpdated` payload every emitter sends, or `None` when a
/// read failed and the caller should skip the emit.
///
/// One builder, because two of them drifted. The HTTP handler enriched
/// `thread_unsettled` and the engine one did not. So any unrelated thread
/// finishing replaced the panel's list with rows whose Apply button was live
/// again, and clicking it 409'd against `guard_change_action`.
///
/// A read failure skips the whole frame rather than sending a partial one. A
/// client would render emptied lists and an "all clear" badge, which is worse
/// than one missed refresh. The next event-driven emit retries.
async fn changes_updated_payload(
    pool: &sqlx::PgPool,
    proj: &crate::core::changes_projection::ChangesProjection,
    apply_estimates: crate::engine::apply_estimate::ApplyEstimates,
    landing_lane: crate::engine::landing_lane::LaneSnapshot,
    main_check: crate::engine::MainCheckSnapshot,
) -> Option<SystemEvent> {
    let (pending_r, set_aside_r, applied_r, restart_r) = tokio::join!(
        crate::core::changes::list_pending_for_readers(
            pool,
            proj,
            crate::core::changes::PendingScope::All,
        ),
        crate::core::changes::list_set_aside_for_readers(
            pool,
            proj,
            crate::core::changes::PendingScope::All,
        ),
        proj.list_recently_applied(APPLIED_IN_BROADCAST, None),
        proj.requires_restart_since(RESTART_REQUIRED_WINDOW),
    );
    let (pending, set_aside, mut applied, restart_required) =
        match (pending_r, set_aside_r, applied_r, restart_r) {
            (Ok(p), Ok(s), Ok(a), Ok(r)) => (p, s, a, r),
            (perr, serr, aerr, rerr) => {
                if let Err(e) = perr {
                    log_legacy!(
                        "[Changes] changes_updated_payload: list_pending_for_readers: {}",
                        e
                    );
                }
                if let Err(e) = serr {
                    log_legacy!(
                        "[Changes] changes_updated_payload: list_set_aside_for_readers: {}",
                        e
                    );
                }
                if let Err(e) = aerr {
                    log_legacy!(
                        "[Changes] changes_updated_payload: list_recently_applied: {}",
                        e
                    );
                }
                if let Err(e) = rerr {
                    log_legacy!(
                        "[Changes] changes_updated_payload: requires_restart_since: {}",
                        e
                    );
                }
                log_legacy!("[Changes] changes_updated_payload: skipping ChangesUpdated emit");
                return None;
            }
        };
    if let Err(e) = crate::core::changes::enrich_thread_titles(pool, &mut applied).await {
        log_legacy!("[Changes] enrich applied titles: {}", e);
    }
    Some(SystemEvent::ChangesUpdated {
        total_pending: pending.len(),
        pending,
        set_aside,
        applied,
        restart_required,
        apply_estimates,
        landing_lane,
        main_check,
    })
}

impl LucidosEngine {
    pub async fn broadcast_changes_updated(&self) {
        let estimates = match self.apply_estimates.current(self.pool()).await {
            Ok(estimates) => estimates,
            Err(e) => {
                log_legacy!(
                    "[Changes] apply estimates: {}; skipping ChangesUpdated emit",
                    e
                );
                return;
            }
        };
        let Some(event) = changes_updated_payload(
            self.pool(),
            self.changes(),
            estimates,
            self.landing_lane_snapshot(),
            self.main_check_snapshot(),
        )
        .await
        else {
            return;
        };
        self.event_bus
            .emit_or_log(BusEvent::System(event), "[Changes] ChangesUpdated")
            .await;
    }

    pub async fn emit_merge_resolution_cleared(
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
        self.broadcast_changes_updated().await;
    }

    pub async fn emit_apply_failed_unhardened(
        &self,
        thread_id: Uuid,
        change_id: &str,
        actor: Option<MessageOrigin>,
        log_tag: &str,
    ) {
        let error_msg =
            "Hardening did not complete (no marker recorded). Click Apply again to retry.";
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        pre_merge_sha: None,
                        post_merge_sha: None,
                        landing_merge_sha: None,
                        change_id: change_id.to_string(),
                        error: error_msg.to_string(),
                        actor,
                    },
                    meta: EventMeta::NONE,
                },
                log_tag,
            )
            .await;
        // INVARIANT: Feed the Apply-All driver — without this an Apply All batch with
        // an unhardened member would stall when hardening fails.
        if let Ok(id) = Uuid::parse_str(change_id) {
            self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
                id,
                error_msg.to_string(),
            ));
        }
    }

    pub async fn emit_change_hardened(&self, thread_id: Uuid, change_id: Uuid, log_tag: &str) {
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

    #[allow(clippy::too_many_arguments)]
    pub async fn emit_change_applied(
        &self,
        claim: &crate::engine::change_ops::apply_claim::ApplyClaim,
        thread_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        commits: Vec<String>,
        thread_title: Option<String>,
        actor: Option<MessageOrigin>,
        merge: Option<AppliedMerge>,
    ) -> Result<BuildRequest, String> {
        let change_id = claim.change_id();
        let merge = match merge {
            Some(merge) => Some(merge),
            None => match self.failed_landing_merge(change_id).await {
                Ok(merge) => merge,
                Err(error) => {
                    let error = format!("The earlier landing could not be checked: {error}. Retry Apply to finish recording it.");
                    self.emit_apply_failed_with_merge(
                        thread_id,
                        change_id,
                        &error,
                        actor.clone(),
                        None,
                    )
                    .await;
                    return Err(error);
                }
            },
        };
        let (pre_merge_sha, post_merge_sha, landing_merge_sha) = match merge {
            Some(m) => (Some(m.pre), Some(m.post), m.landing),
            None => (None, None, None),
        };
        // INVARIANT: Use the landed trees, not the proposal snapshot or approval marker.
        // A no-op apply has no landed files, so it cannot claim a plan landing.
        let content = match (&pre_merge_sha, &post_merge_sha) {
            (Some(pre), Some(post)) => {
                let change = match self.changes().get_by_id(change_id).await {
                    Ok(Some(change)) => change,
                    result => {
                        log_legacy!(
                            "[Changes] Cannot classify applied change {}: {:?}",
                            change_id,
                            result
                        );
                        let error = "The change merged to main, but its landing could not be recorded. Its pending change could not be read. Retry Apply to finish recording it.".to_string();
                        self.emit_apply_failed_with_merge(
                            thread_id,
                            change_id,
                            &error,
                            actor.clone(),
                            Some(AppliedMerge {
                                pre: pre.clone(),
                                post: post.clone(),
                                landing: landing_merge_sha.clone(),
                            }),
                        )
                        .await;
                        return Err(error);
                    }
                };
                let range = format!("{pre}..{post}");
                match crate::engine::git_ops::changed_files_in_range(
                    std::path::Path::new(&change.repo_root),
                    &range,
                )
                .await
                {
                    Ok(files) => crate::engine::thread_events::ChangeContent::from_files(&files),
                    Err(error) => {
                        log_legacy!(
                            "[Changes] Cannot classify applied change {}: {}",
                            change_id,
                            error
                        );
                        let error = format!("The change merged to main, but its landing could not be recorded. Git could not read the landed files: {error}. Retry Apply to finish recording it.");
                        self.emit_apply_failed_with_merge(
                            thread_id,
                            change_id,
                            &error,
                            actor.clone(),
                            Some(AppliedMerge {
                                pre: pre.clone(),
                                post: post.clone(),
                                landing: landing_merge_sha.clone(),
                            }),
                        )
                        .await;
                        return Err(error);
                    }
                }
            }
            _ => crate::engine::thread_events::ChangeContent::Code,
        };
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
        let accepted = match self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeApplied {
                    content,
                    change_id: change_id.to_string(),
                    requires_restart,
                    client_update,
                    commits,
                    thread_title,
                    actor: actor.clone(),
                    pre_merge_sha: pre_merge_sha.clone(),
                    post_merge_sha: post_merge_sha.clone(),
                    landing_merge_sha: landing_merge_sha.clone(),
                    path: String::new(),
                },
                meta: EventMeta::NONE,
            })
            .await
        {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Err("the event was refused".to_string()),
            Err(e) if e.to_string().contains(CHANGE_APPLIED_ONCE_INDEX) => {
                log_legacy!(
                    "[Changes] ChangeApplied for {} refused: another engine on this database already recorded it",
                    change_id
                );
                Ok(false)
            }
            Err(e) => {
                log_legacy!("[EventBus] [Changes] ChangeApplied emit failed: {}", e);
                Err(e.to_string())
            }
        };
        let accepted = match accepted {
            Ok(accepted) => accepted,
            Err(cause) => {
                let error = format!("The landing could not be recorded: {cause}. Retry Apply to finish recording it.");
                let merge = pre_merge_sha
                    .zip(post_merge_sha)
                    .map(|(pre, post)| AppliedMerge {
                        pre,
                        post,
                        landing: landing_merge_sha,
                    });
                self.emit_apply_failed_with_merge(thread_id, change_id, &error, actor, merge)
                    .await;
                return Err(error);
            }
        };
        // INVARIANT: Feed the Apply-All driver. Non-member ids are no-ops; batch
        // members advance the registry and (if more remain) trigger the
        // next apply. Non-blocking — the channel send hands off to the
        // driver task, never holds a lock.
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Applied(
            change_id,
        ));
        if accepted {
            self.clear_gate_markers_of_applied_change(change_id).await;
            return Ok(self
                .post_apply_dev_refresh(thread_id, requires_restart, client_update)
                .await);
        }
        Ok(BuildRequest::None)
    }

    async fn clear_gate_markers_of_applied_change(&self, change_id: Uuid) {
        let change = match self.changes().get_by_id(change_id).await {
            Ok(Some(change)) => change,
            Ok(None) => return,
            Err(e) => {
                log_legacy!(
                    "[Changes] marker clear for applied change {} skipped: {}",
                    change_id,
                    e
                );
                return;
            }
        };
        let repo_root = std::path::Path::new(&change.repo_root);
        consume_harden_marker(self.pool(), repo_root, &change.branch_name).await;
        consume_plan_marker(self.pool(), repo_root, &change.branch_name).await;
    }

    async fn post_apply_dev_refresh(
        &self,
        thread_id: Uuid,
        requires_restart: bool,
        client_update: bool,
    ) -> BuildRequest {
        let Some(engine) = self.try_clone_arc() else {
            // INVARIANT: `main.rs` installs the self-reference immediately after `Arc::new`,
            // long before the HTTP server binds — so in a real engine this is
            // unreachable and only unit tests take it. Log it anyway: a silently
            // skipped rebuild is precisely the failure this funnel exists to
            // stop, and it must not be invisible if the wiring ever moves.
            log_legacy!(
                "[Changes] post-apply refresh skipped for change on thread {} — engine self-reference not installed",
                thread_id
            );
            return BuildRequest::None;
        };
        let is_lucidos_source =
            crate::engine::change_ops::load_apply_kind_context(self.pool(), Some(thread_id))
                .await
                .is_lucidos_source();
        let decision = post_apply_refresh(requires_restart, client_update, is_lucidos_source);
        log_legacy!(
            "[Changes] post-apply refresh: {:?} (requires_restart={}, client_update={})",
            decision,
            requires_restart,
            client_update
        );
        match decision {
            PostApplyRefresh::RebuildEngine => {
                return engine.automatic_background_rebuild().await.into()
            }
            PostApplyRefresh::ReSnapshotFrontend => engine.refresh_served_frontend_after_rebuild(),
            PostApplyRefresh::Nothing => {}
        }
        BuildRequest::None
    }
    /// INVARIANT: Scan files touched by an applied change and emit per-entity events
    /// (`AppCreated`/`AppUpdated`/`AppDeleted`,
    /// `ArtifactCreated`/`ArtifactUpdated`/`ArtifactDeleted`) so cached
    /// frontend lists refresh.
    ///
    /// `ChangeApplied` alone is insufficient: the frontend's
    /// `processSSEForReferences` subscribes to entity events by name
    /// (`AppCreated` → `loadApps()`, etc.) and never inspects change file
    /// paths. Without this emission, an app created by a coding-agent thread
    /// via raw file writes (`data/apps/<id>/manifest.json` + siblings) shows
    /// up on disk after Apply but never refreshes `appsList`, so search finds
    /// it (search reads disk) but `openAppById` doesn't (it reads cache) —
    /// the "no app with id" toast.
    ///
    /// Detection per entity uses (post-merge disk existence, pre-merge git
    /// existence):
    ///   `(true, false)` → Created
    ///   `(true, true)`  → Updated
    ///   `(false, true)` → Deleted
    ///   `(false, false)` → skipped (transient touch, not user-visible)
    ///
    /// Triggers are intentionally skipped: their state lives in the
    /// event-sourced `TriggerCreated`/`Updated`/`Deleted` projection
    /// (`scheduler::list_trigger_configs`), not on disk — a CC change that
    /// writes `data/triggers/<id>/...` files cannot make a working trigger
    /// without also emitting the right projection events. That's a deeper
    /// architectural gap to address separately.
    pub async fn emit_entity_events_for_change_apply(
        &self,
        files: &[String],
        pre_merge_sha: Option<&str>,
        post_merge_sha: Option<&str>,
        actor: Option<MessageOrigin>,
        writer_thread_id: Option<Uuid>,
    ) {
        let origin = match writer_thread_id {
            Some(id) => crate::core::artifact_writer::ArtifactWriterOrigin::Thread(id),
            None => crate::core::artifact_writer::ArtifactWriterOrigin::Known(
                crate::engine::event_bus::ArtifactWriter::Engine,
            ),
        };
        let writes_artifact = files.iter().any(|file| {
            file.starts_with("data/artifacts/") && self.workspace_path().join(file).exists()
        });
        let writer = if !writes_artifact {
            None
        } else {
            match origin.resolve(self.event_bus.pool()).await {
                Ok(writer) => Some(writer),
                Err(e) => {
                    crate::log_legacy!(
                        "[Changes] Failed to resolve artifact writer for apply: {e}"
                    );
                    let message = format!(
                    "The change applied, but artifact writes could not be recorded because their writer could not be resolved: {e}"
                );
                    if let Err(error) = self
                        .create_notification(
                            "Artifact history could not be recorded",
                            &message,
                            None,
                            writer_thread_id,
                            None,
                            crate::scheduler::notifications::Tap::Modal,
                            actor.clone(),
                        )
                        .await
                    {
                        crate::log_legacy!(
                            "[Changes] Could not announce missing artifact history: {error}"
                        );
                    }
                    None
                }
            }
        };
        let events = compute_entity_events_for_change_apply(
            self.workspace_path(),
            pre_merge_sha,
            post_merge_sha,
            files,
            actor,
            writer_thread_id,
            writer,
        )
        .await;
        for ev in events {
            let label = match &ev {
                SystemEvent::AppCreated { .. }
                | SystemEvent::AppUpdated { .. }
                | SystemEvent::AppDeleted { .. } => "[Changes] App entity event from apply",
                SystemEvent::ArtifactCreated { .. }
                | SystemEvent::ArtifactUpdated { .. }
                | SystemEvent::ArtifactDeleted { .. } => {
                    "[Changes] Artifact entity event from apply"
                }
                _ => "[Changes] Entity event from apply",
            };
            self.event_bus
                .emit_or_log(BusEvent::System(ev), label)
                .await;
        }
    }

    pub async fn emit_apply_failed(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        error: &str,
        actor: Option<MessageOrigin>,
    ) {
        self.record_apply_failure(thread_id, change_id, error, actor, None)
            .await;
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
            change_id,
            error.to_string(),
        ));
    }

    async fn failed_landing_merge(
        &self,
        change_id: Uuid,
    ) -> Result<Option<AppliedMerge>, sqlx::Error> {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT payload->>'pre_merge_sha', payload->>'post_merge_sha', payload->>'landing_merge_sha' \
             FROM events WHERE event_type = 'ChangeApplyFailed' AND payload->>'change_id' = $1 \
             AND payload->>'pre_merge_sha' IS NOT NULL AND payload->>'post_merge_sha' IS NOT NULL \
             ORDER BY sequence DESC LIMIT 1",
        ).bind(change_id.to_string()).fetch_optional(self.pool()).await?;
        Ok(row.map(|(pre, post, landing)| AppliedMerge { pre, post, landing }))
    }

    async fn record_apply_failure(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        error: &str,
        actor: Option<MessageOrigin>,
        merge: Option<AppliedMerge>,
    ) {
        let (pre_merge_sha, post_merge_sha, landing_merge_sha) = match merge {
            Some(merge) => (Some(merge.pre), Some(merge.post), merge.landing),
            None => (None, None, None),
        };
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        pre_merge_sha,
                        post_merge_sha,
                        landing_merge_sha,
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

    async fn emit_apply_failed_with_merge(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        error: &str,
        actor: Option<MessageOrigin>,
        merge: Option<AppliedMerge>,
    ) {
        self.record_apply_failure(thread_id, change_id, error, actor, merge)
            .await;
        match crate::engine::landing_lane::lane_held_changes(self.pool(), &[change_id]).await {
            Ok(held) if held.contains(&change_id) => {
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::ChangeUnqueued {
                                change_id: change_id.to_string(),
                                reason: crate::engine::thread_events::UnqueueReason::LandingFailed,
                            },
                            meta: EventMeta::NONE,
                        },
                        "[Changes] ChangeUnqueued after failed landing",
                    )
                    .await;
            }
            Err(cause) => log_legacy!(
                "[Changes] Could not check failed change's lane membership: {}",
                cause
            ),
            _ => {}
        }
        // INVARIANT: Feed the Apply-All driver. Failed members do NOT halt the batch
        // — the driver records the failure and keeps applying the rest.
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
            change_id,
            error.to_string(),
        ));
    }

    pub(crate) async fn emit_missing_hardening_detected(&self, thread_id: Uuid) {
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::MissingHardeningDetected {
                        origin: Some(MessageOrigin::engine(EngineReason::MissingHardening)),
                    },
                    meta: EventMeta {
                        channel: Some(EventChannel::ClaudeCode),
                        ..EventMeta::NONE
                    },
                },
                "[Changes] MissingHardeningDetected",
            )
            .await;
    }

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
                        origin: Some(MessageOrigin::engine(EngineReason::MergeConflict)),
                    },
                    meta: EventMeta {
                        channel: Some(EventChannel::ClaudeCode),
                        ..EventMeta::NONE
                    },
                },
                "[Changes] MergeConflictDetected",
            )
            .await;
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Parked(
            change_id,
        ));
        // INVARIANT: The event opens the pairing that `resolving_conflict` reads, so the
        // panel repaints the row as an apply in flight.
        self.broadcast_changes_updated().await;
    }

    /// INVARIANT: Emit `MergeConflictDetected` AND build the merge prompt to send to CC.
    ///
    /// Bundle the emit with the prompt construction so no caller can send a
    /// merge prompt without recording the boundary event first. The three call
    /// sites (live `apply_now::merge_via_cc_session`, Tier-3
    /// `spawn_merge_session`, Tier-2 `run_merge_session_tier2`) MUST go through
    /// this — otherwise the chat-exchange panel has nothing to render and the
    /// `MergeConflictDetected` SSE consumers (toast, `applyingChangeIds`)
    /// never fire.
    ///
    /// Returns the prompt body the caller then ships to CC by whatever
    /// per-tier mechanism applies (live `msg_tx.send`, fresh
    /// `run_direct_agent`, resumed `run_direct_agent`).
    pub async fn start_merge_and_get_prompt(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        files: Vec<String>,
        target_branch: &str,
        body_intro: Option<&str>,
        description: Option<&str>,
    ) -> String {
        self.emit_merge_conflict_detected(thread_id, change_id, files)
            .await;
        let is_app =
            crate::engine::change_ops::load_apply_kind_context(&self.pool, Some(thread_id))
                .await
                .is_app();
        crate::engine::merge_prompts::build_merge_prompt(
            target_branch,
            body_intro,
            description,
            is_app,
        )
    }
}

pub(crate) async fn compute_entity_events_for_change_apply(
    workspace_path: &std::path::Path,
    pre_merge_sha: Option<&str>,
    post_merge_sha: Option<&str>,
    files: &[String],
    actor: Option<MessageOrigin>,
    writer_thread_id: Option<Uuid>,
    writer: Option<crate::engine::event_bus::ArtifactWriter>,
) -> Vec<SystemEvent> {
    let mut app_ids: BTreeSet<String> = BTreeSet::new();
    let mut artifact_paths: BTreeSet<String> = BTreeSet::new();
    for file in files {
        if let Some(rest) = file.strip_prefix("data/apps/") {
            if let Some(slash) = rest.find('/') {
                let app_id = &rest[..slash];
                if !app_id.is_empty() {
                    app_ids.insert(app_id.to_string());
                }
            }
        } else if let Some(rest) = file.strip_prefix("data/artifacts/") {
            if !rest.is_empty() {
                artifact_paths.insert(rest.to_string());
            }
        }
    }

    let mut events = Vec::new();

    for app_id in app_ids {
        let manifest_relpath = format!("data/apps/{}/manifest.json", app_id);
        let exists_post = workspace_path.join(&manifest_relpath).exists();
        let exists_pre = match pre_merge_sha {
            Some(sha) => git_path_exists_at(workspace_path, sha, &manifest_relpath).await,
            None => false,
        };
        let event = match (exists_post, exists_pre) {
            (true, false) => Some(SystemEvent::AppCreated {
                app_id: app_id.clone(),
                name: read_app_name(workspace_path, &app_id),
                actor: actor.clone(),
            }),
            (true, true) => Some(SystemEvent::AppUpdated {
                app_id: app_id.clone(),
                name: read_app_name(workspace_path, &app_id),
                actor: actor.clone(),
            }),
            (false, true) => Some(SystemEvent::AppDeleted {
                kind: match pre_merge_sha {
                    Some(sha) => app_kind_at(workspace_path, sha, &app_id).await,
                    None => None,
                },
                app_id: app_id.clone(),
                actor: actor.clone(),
            }),
            (false, false) => None,
        };
        if let Some(ev) = event {
            events.push(ev);
        }
    }

    let commit = post_merge_sha.unwrap_or_default().to_string();
    for path in artifact_paths {
        let full = format!("data/artifacts/{}", path);
        let exists_post = workspace_path.join(&full).exists();
        let exists_pre = match pre_merge_sha {
            Some(sha) => git_path_exists_at(workspace_path, sha, &full).await,
            None => false,
        };
        // INVARIANT: A failed provenance lookup must not invent a writer or suppress
        // app and deletion events, which need no artifact writer.
        if exists_post && writer.is_none() {
            continue;
        }
        let event = match (exists_post, exists_pre) {
            (true, false) => Some(SystemEvent::ArtifactCreated {
                artifact_path: path.clone(),
                commit: commit.clone(),
                source: Some("change_apply".to_string()),
                writer_thread_id,
                writer: writer.clone(),
            }),
            (true, true) => Some(SystemEvent::ArtifactUpdated {
                artifact_path: path.clone(),
                commit: commit.clone(),
                source: Some("change_apply".to_string()),
                writer_thread_id,
                writer: writer.clone(),
            }),
            (false, true) => Some(SystemEvent::ArtifactDeleted {
                artifact_path: path.clone(),
                commit: commit.clone(),
            }),
            (false, false) => None,
        };
        if let Some(ev) = event {
            events.push(ev);
        }
    }

    events
}

/// INVARIANT: Return true iff `path` exists in the tree at `sha`. Uses `git cat-file -e`
/// (exit 0 = present, exit 1 = missing). Any other failure (timeout, repo
/// error, broken sha) is treated as "missing" so the caller doesn't spuriously
/// emit `*Deleted` for an Updated path. That miss has a cost in the other
/// direction (an Updated becomes a Created — frontend reloads list, no harm;
/// a Deleted is suppressed — list stays stale until next event), so log the
/// unexpected exit code path so flake is greppable in `[Changes] git cat-file`.
async fn git_path_exists_at(repo_root: &std::path::Path, sha: &str, path: &str) -> bool {
    let spec = format!("{}:{}", sha, path);
    match git_cmd(&["cat-file", "-e", &spec], repo_root).await {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            if o.status.code() != Some(1) {
                log_legacy!(
                    "[Changes] git cat-file -e {} exited {:?} stderr={}",
                    spec,
                    o.status.code(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            false
        }
        Err(e) => {
            log_legacy!("[Changes] git cat-file -e {} failed: {}", spec, e);
            false
        }
    }
}

async fn app_kind_at(repo_root: &std::path::Path, sha: &str, app_id: &str) -> Option<AppKind> {
    let spec = format!("{}:data/apps/{}/manifest.json", sha, app_id);
    let out = match git_cmd(&["show", &spec], repo_root).await {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            log_legacy!(
                "[Changes] git show {} exited {:?}: {}",
                spec,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return None;
        }
        Err(e) => {
            log_legacy!("[Changes] git show {} failed: {}", spec, e);
            return None;
        }
    };
    let manifest: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(AppKind::from_manifest(
        app_id,
        manifest.get("kind").and_then(|k| k.as_str()),
    ))
}

fn read_app_name(workspace_path: &std::path::Path, app_id: &str) -> Option<String> {
    let manifest = workspace_path
        .join("data/apps")
        .join(app_id)
        .join("manifest.json");
    let raw = std::fs::read_to_string(&manifest).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostApplyRefresh {
    RebuildEngine,
    ReSnapshotFrontend,
    Nothing,
}

/// INVARIANT: Decide the post-apply refresh from the merged change's shape — the pure
/// core of [`LucidosEngine::post_apply_dev_refresh`].
///
/// `is_lucidos_source` gates BOTH arms: only Lucidos's own source can rebuild
/// Lucidos's engine binary or its served `dist/`. That gate is load-bearing for
/// the RESTART arm, not just cosmetic — an *external-repo coding-agent thread*
/// computes `requires_restart` with the same `files_require_restart` file
/// heuristic against SOMEONE ELSE'S tree, so a change touching a `.rs` file in
/// a user-registered repo arrives here flagged restart-requiring. App changes
/// are already forced to `requires_restart == false` upstream; external-repo
/// ones are not, and rebuilding Lucidos because an unrelated repo has Rust in
/// it would be plainly wrong.
fn post_apply_refresh(
    requires_restart: bool,
    client_update: bool,
    is_lucidos_source: bool,
) -> PostApplyRefresh {
    if !is_lucidos_source {
        PostApplyRefresh::Nothing
    } else if requires_restart {
        PostApplyRefresh::RebuildEngine
    } else if client_update {
        PostApplyRefresh::ReSnapshotFrontend
    } else {
        PostApplyRefresh::Nothing
    }
}

pub(crate) const CHANGE_APPLIED_ONCE_INDEX: &str = "events_change_applied_once";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn classification_failure_reports_an_unrecorded_landing_without_claiming_success() {
        crate::test_support::run_on_engine_runtime(
            "engine::change_ops::emitters::tests::classification_failure_reports_an_unrecorded_landing_without_claiming_success",
            classification_failure_case,
        );
    }

    async fn classification_failure_case() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
        let thread = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
        for proposed in [false, true] {
            let change = Uuid::new_v4();
            if proposed {
                let event: ThreadEvent = serde_json::from_value(serde_json::json!({
                    "type": "ChangeProposed", "change_id": change.to_string(),
                    "files": ["docs/plans/example.md"], "branch_name": "work",
                    "repo_root": workspace.path().join("missing-repository").to_string_lossy(),
                }))
                .unwrap();
                engine
                    .event_bus
                    .emit(BusEvent::Thread {
                        thread_id: thread,
                        event,
                        meta: EventMeta::NONE,
                    })
                    .await
                    .unwrap();
            }
            let claim = engine.apply_claims.try_claim(change).unwrap();
            let error = engine
                .emit_change_applied(
                    &claim,
                    thread,
                    false,
                    false,
                    Vec::new(),
                    None,
                    None,
                    Some(AppliedMerge::new("before", "after")),
                )
                .await
                .unwrap_err();
            assert!(error.contains("merged to main"));
            assert!(error.contains("Retry Apply"));
            let events: Vec<String> = sqlx::query_scalar(
                "SELECT event_type FROM events WHERE thread_id = $1 AND payload->>'change_id' = $2 AND event_type IN ('ChangeApplied', 'ChangeApplyFailed') ORDER BY sequence",
            ).bind(thread).bind(change.to_string()).fetch_all(&pool).await.unwrap();
            assert_eq!(events, ["ChangeApplyFailed"]);
            if proposed {
                assert!(engine
                    .changes()
                    .get_by_id(change)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_pending());
            }
            assert!(engine.apply_claims.is_held(change));
            drop(claim);
            assert!(!engine.apply_claims.is_held(change));
        }
        let staging = workspace.path().join("staging-repository");
        let restored = workspace.path().join("restored-repository");
        tokio::fs::create_dir_all(&staging).await.unwrap();
        async fn git(repo: &std::path::Path, args: &[&str]) -> String {
            let out = git_cmd(args, repo).await.unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        }
        git(&staging, &["init", "-b", "main"]).await;
        git(&staging, &["config", "user.name", "Test User"]).await;
        git(&staging, &["config", "user.email", "test@example.com"]).await;
        tokio::fs::write(staging.join("README.md"), "Example repository")
            .await
            .unwrap();
        git(&staging, &["add", "."]).await;
        git(&staging, &["commit", "-m", "docs: initialize repository"]).await;
        let pre = git(&staging, &["rev-parse", "HEAD"]).await;
        git(&staging, &["checkout", "-b", "plan"]).await;
        tokio::fs::create_dir_all(staging.join("docs/plans"))
            .await
            .unwrap();
        tokio::fs::write(staging.join("docs/plans/example.md"), "The plan")
            .await
            .unwrap();
        git(&staging, &["add", "."]).await;
        git(&staging, &["commit", "-m", "docs: add plan"]).await;
        git(&staging, &["checkout", "main"]).await;
        git(&staging, &["merge", "--no-ff", "--no-edit", "plan"]).await;
        let post = git(&staging, &["rev-parse", "HEAD"]).await;
        let change = Uuid::new_v4();
        let event: ThreadEvent = serde_json::from_value(serde_json::json!({
            "type": "ChangeProposed", "change_id": change.to_string(),
            "files": ["docs/plans/example.md"], "branch_name": "plan",
            "repo_root": restored.to_string_lossy(),
        }))
        .unwrap();
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event: ThreadEvent::ChangeQueued {
                    change_id: change.to_string(),
                    snapshot_sha: post.clone(),
                },
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        engine
            .emit_apply_failed(thread, change, "Repository has uncommitted changes", None)
            .await;
        assert!(!crate::engine::landing_lane::queued_changes(&pool, thread)
            .await
            .unwrap()
            .is_empty());
        let claim = engine.apply_claims.try_claim(change).unwrap();
        assert!(engine
            .emit_change_applied(
                &claim,
                thread,
                false,
                false,
                Vec::new(),
                None,
                None,
                Some(AppliedMerge::landing(&pre, &post)),
            )
            .await
            .is_err());
        drop(claim);
        let failure: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = 'ChangeApplyFailed' AND payload->>'change_id' = $1 ORDER BY sequence DESC LIMIT 1",
        ).bind(change.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(failure["pre_merge_sha"], pre);
        assert_eq!(failure["post_merge_sha"], post);
        assert_eq!(failure["landing_merge_sha"], post);
        assert!(crate::engine::landing_lane::queued_changes(&pool, thread)
            .await
            .unwrap()
            .is_empty());
        let claim = engine.apply_claims.try_claim(change).unwrap();
        assert!(engine
            .emit_change_applied(&claim, thread, false, false, Vec::new(), None, None, None,)
            .await
            .is_err());
        tokio::fs::rename(&staging, &restored).await.unwrap();
        engine
            .emit_change_applied(&claim, thread, false, false, Vec::new(), None, None, None)
            .await
            .unwrap();
        let applied: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = 'ChangeApplied' AND payload->>'change_id' = $1",
        ).bind(change.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(applied["content"], "plan-only");
        assert_eq!(applied["pre_merge_sha"], pre);
        assert_eq!(applied["post_merge_sha"], post);
        assert_eq!(applied["landing_merge_sha"], post);
        drop(claim);
        let refused = Uuid::new_v4();
        let event: ThreadEvent = serde_json::from_value(serde_json::json!({
            "type": "ChangeProposed", "change_id": refused.to_string(),
            "files": ["docs/plans/example.md"], "branch_name": "plan",
            "repo_root": restored.to_string_lossy(),
        }))
        .unwrap();
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: thread,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        crate::test_support::refuse_event_type(&pool, "ChangeApplied").await;
        let claim = engine.apply_claims.try_claim(refused).unwrap();
        let error = engine
            .emit_change_applied(
                &claim,
                thread,
                false,
                false,
                Vec::new(),
                None,
                None,
                Some(AppliedMerge::landing(&pre, &post)),
            )
            .await
            .unwrap_err();
        assert!(error.contains("landing could not be recorded"));
        assert!(engine
            .changes()
            .get_by_id(refused)
            .await
            .unwrap()
            .unwrap()
            .is_pending());
        let failure: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = 'ChangeApplyFailed' AND payload->>'change_id' = $1 ORDER BY sequence DESC LIMIT 1",
        ).bind(refused.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(failure["pre_merge_sha"], pre);
        assert_eq!(failure["post_merge_sha"], post);
        drop(claim);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_changes_updated_frame_carries_the_thread_unsettled_gate() {
        use crate::core::changes_projection::ChangesProjection;
        use crate::engine::event_bus::EventBus;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();

        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: format!("cc-{thread}"),
                branch: String::new(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("start a coding-agent thread");

        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::ChangeProposed {
                content: Default::default(),
                change_id: change.to_string(),
                description: Some("work".to_string()),
                files: vec!["a.rs".to_string()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: "claude-code/work".to_string(),
                repo_root: "/repo".to_string(),
                hardened: true,
                incomplete: false,
                set_aside: false,
                path: String::new(),
                diff: String::new(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("propose the change");

        sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
            .bind(thread)
            .execute(&pool)
            .await
            .expect("put the thread mid-turn");

        let proj = ChangesProjection::new(pool.clone());
        let event = changes_updated_payload(
            &pool,
            &proj,
            Default::default(),
            Default::default(),
            Default::default(),
        )
        .await
        .expect("every read answered");
        match event {
            SystemEvent::ChangesUpdated { pending, .. } => {
                assert_eq!(pending.len(), 1, "one pending change, got {:?}", pending);
                assert!(
                    pending[0].thread_state().unwrap().unsettled(),
                    "the frame must report the thread as unsettled"
                );
            }
            other => panic!("expected ChangesUpdated, got {:?}", other),
        }

        teardown_test_db(&db).await;
    }

    #[test]
    fn both_halves_of_the_merge_resolution_pairing_broadcast_changes_updated() {
        const SRC: &str = include_str!("change_ops_emitters.rs");
        const BROADCAST: &str = "self.broadcast_changes_updated().await;";
        for emitter in [
            "async fn emit_merge_conflict_detected(",
            "async fn emit_merge_resolution_cleared(",
        ] {
            let start = SRC
                .find(emitter)
                .unwrap_or_else(|| panic!("{emitter} must still exist"));
            let rest = &SRC[start + emitter.len()..];
            let end = rest.find("\n    }").expect("the emitter must end");
            assert!(
                rest[..end].contains(BROADCAST),
                "{emitter} must broadcast ChangesUpdated, or clients keep a stale Apply button"
            );
        }
    }

    #[test]
    fn restart_requiring_change_rebuilds_the_engine() {
        for client_update in [false, true] {
            assert_eq!(
                post_apply_refresh(true, client_update, true),
                PostApplyRefresh::RebuildEngine,
                "requires_restart wins outright (client_update={client_update})"
            );
        }
    }

    #[test]
    fn frontend_only_lucidos_source_change_resnapshots_the_served_frontend() {
        assert_eq!(
            post_apply_refresh(false, true, true),
            PostApplyRefresh::ReSnapshotFrontend
        );
    }

    #[test]
    fn non_lucidos_source_change_advances_nothing() {
        assert_eq!(
            post_apply_refresh(false, true, false),
            PostApplyRefresh::Nothing
        );
    }

    #[test]
    fn external_repo_change_never_rebuilds_the_lucidos_engine() {
        assert_eq!(
            post_apply_refresh(true, false, false),
            PostApplyRefresh::Nothing
        );
        assert_eq!(
            post_apply_refresh(true, true, false),
            PostApplyRefresh::Nothing
        );
    }

    #[test]
    fn change_touching_neither_surface_advances_nothing() {
        assert_eq!(
            post_apply_refresh(false, false, true),
            PostApplyRefresh::Nothing
        );
    }

    async fn init_repo(path: &std::path::Path) -> String {
        std::fs::create_dir_all(path.join("data")).unwrap();
        std::fs::write(path.join("data/.keep"), "").unwrap();
        git_cmd(&["init", "-q", "-b", "main"], path).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], path)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], path)
            .await
            .unwrap();
        git_cmd(&["add", "-A"], path).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "init"], path)
            .await
            .unwrap();
        let out = git_cmd(&["rev-parse", "HEAD"], path).await.unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    async fn commit_all(path: &std::path::Path, msg: &str) -> String {
        git_cmd(&["add", "-A"], path).await.unwrap();
        git_cmd(&["commit", "-q", "-m", msg], path).await.unwrap();
        let out = git_cmd(&["rev-parse", "HEAD"], path).await.unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write_app_manifest(workspace: &std::path::Path, app_id: &str, name: &str) -> PathBuf {
        let dir = workspace.join("data/apps").join(app_id);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest_path = dir.join("manifest.json");
        std::fs::write(
            &manifest_path,
            format!(r#"{{"name":"{}","description":"","knowhow":[]}}"#, name),
        )
        .unwrap();
        std::fs::write(dir.join("index.html"), "<html/>").unwrap();
        manifest_path
    }

    #[tokio::test]
    async fn missing_writer_preserves_apps_and_deletions_without_inventing_artifact_provenance() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        init_repo(ws).await;
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/deleted.md"), "old").unwrap();
        std::fs::write(ws.join("data/artifacts/updated.md"), "old").unwrap();
        let pre_sha = commit_all(ws, "Existing artifacts").await;
        std::fs::remove_file(ws.join("data/artifacts/deleted.md")).unwrap();
        std::fs::write(ws.join("data/artifacts/created.md"), "new").unwrap();
        std::fs::write(ws.join("data/artifacts/updated.md"), "new").unwrap();
        write_app_manifest(ws, "new-app", "New App");
        let post_sha = commit_all(ws, "Apply mixed change").await;
        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &[
                "data/apps/new-app/manifest.json".into(),
                "data/artifacts/deleted.md".into(),
                "data/artifacts/created.md".into(),
                "data/artifacts/updated.md".into(),
            ],
            None,
            Some(Uuid::new_v4()),
            None,
        )
        .await;
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(events.iter().any(|event| matches!(event,
            SystemEvent::AppCreated { app_id, .. } if app_id == "new-app"
        )));
        assert!(events.iter().any(|event| matches!(event,
            SystemEvent::ArtifactDeleted { artifact_path, .. } if artifact_path == "deleted.md"
        )));
    }

    #[tokio::test]
    async fn cc_apply_creating_a_new_app_emits_app_created() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let pre_sha = init_repo(ws).await;

        write_app_manifest(ws, "habit-tracker", "Habit Tracker");
        let post_sha = commit_all(ws, "Create app habit-tracker").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &[
                "data/apps/habit-tracker/manifest.json".to_string(),
                "data/apps/habit-tracker/index.html".to_string(),
            ],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert_eq!(
            events.len(),
            1,
            "expected exactly one app event, got {:?}",
            events
        );
        match &events[0] {
            SystemEvent::AppCreated { app_id, name, .. } => {
                assert_eq!(app_id, "habit-tracker");
                assert_eq!(name.as_deref(), Some("Habit Tracker"));
            }
            other => panic!("expected AppCreated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn cc_apply_modifying_existing_app_emits_app_updated() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_app_manifest(ws, "habit-tracker", "Habit Tracker");
        git_cmd(&["init", "-q", "-b", "main"], ws).await.unwrap();
        git_cmd(&["config", "user.email", "t@e.com"], ws)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "T"], ws).await.unwrap();
        git_cmd(&["add", "-A"], ws).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "seed"], ws).await.unwrap();
        let pre_sha = String::from_utf8(git_cmd(&["rev-parse", "HEAD"], ws).await.unwrap().stdout)
            .unwrap()
            .trim()
            .to_string();

        std::fs::write(
            ws.join("data/apps/habit-tracker/index.html"),
            "<html>v2</html>",
        )
        .unwrap();
        let post_sha = commit_all(ws, "Edit habit-tracker").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &["data/apps/habit-tracker/index.html".to_string()],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert_eq!(events.len(), 1, "got {:?}", events);
        assert!(
            matches!(&events[0], SystemEvent::AppUpdated { app_id, .. } if app_id == "habit-tracker"),
            "expected AppUpdated for habit-tracker, got {:?}",
            events[0]
        );
    }

    #[tokio::test]
    async fn cc_apply_removing_app_emits_app_deleted() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_app_manifest(ws, "going-away", "Going Away");
        git_cmd(&["init", "-q", "-b", "main"], ws).await.unwrap();
        git_cmd(&["config", "user.email", "t@e.com"], ws)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "T"], ws).await.unwrap();
        git_cmd(&["add", "-A"], ws).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "seed"], ws).await.unwrap();
        let pre_sha = String::from_utf8(git_cmd(&["rev-parse", "HEAD"], ws).await.unwrap().stdout)
            .unwrap()
            .trim()
            .to_string();

        std::fs::remove_dir_all(ws.join("data/apps/going-away")).unwrap();
        let post_sha = commit_all(ws, "Delete going-away").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &[
                "data/apps/going-away/manifest.json".to_string(),
                "data/apps/going-away/index.html".to_string(),
            ],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert_eq!(events.len(), 1, "got {:?}", events);
        assert!(
            matches!(
                &events[0],
                SystemEvent::AppDeleted { app_id, kind: Some(AppKind::App), .. }
                    if app_id == "going-away"
            ),
            "got {:?}",
            events[0]
        );
    }

    #[tokio::test]
    async fn cc_apply_removing_a_widget_records_its_kind() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let pre_sha = init_repo(ws).await;
        let dir = ws.join("data/apps/fare-grid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name": "Fare grid", "kind": "widget"}"#,
        )
        .unwrap();
        let with_widget = commit_all(ws, "Add fare-grid").await;
        std::fs::remove_dir_all(&dir).unwrap();
        let post_sha = commit_all(ws, "Delete fare-grid").await;
        assert_ne!(pre_sha, with_widget);

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&with_widget),
            Some(&post_sha),
            &["data/apps/fare-grid/manifest.json".to_string()],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert!(
            matches!(
                &events[..],
                [SystemEvent::AppDeleted {
                    kind: Some(AppKind::Widget),
                    ..
                }]
            ),
            "got {:?}",
            events
        );
    }

    #[tokio::test]
    async fn cc_apply_emits_artifact_events_too() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let pre_sha = init_repo(ws).await;

        let artifacts = ws.join("data/artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("notes.md"), "hello").unwrap();
        let post_sha = commit_all(ws, "Add artifact").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &["data/artifacts/notes.md".to_string()],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert_eq!(events.len(), 1, "got {:?}", events);
        match &events[0] {
            SystemEvent::ArtifactCreated {
                artifact_path,
                commit,
                source,
                writer,
                ..
            } => {
                assert_eq!(
                    writer.as_ref(),
                    Some(&crate::engine::event_bus::ArtifactWriter::Thread)
                );
                assert_eq!(artifact_path, "notes.md");
                assert_eq!(commit, &post_sha);
                assert_eq!(source.as_deref(), Some("change_apply"));
            }
            other => panic!("expected ArtifactCreated, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn unrelated_paths_are_ignored() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let pre_sha = init_repo(ws).await;

        std::fs::write(ws.join("README.md"), "hi").unwrap();
        std::fs::create_dir_all(ws.join("data/triggers/some-trigger")).unwrap();
        std::fs::write(ws.join("data/triggers/some-trigger/run.md"), "run").unwrap();
        let post_sha = commit_all(ws, "non-entity changes").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &[
                "README.md".to_string(),
                "data/triggers/some-trigger/run.md".to_string(),
            ],
            None,
            None,
            Some(crate::engine::event_bus::ArtifactWriter::Thread),
        )
        .await;

        assert!(events.is_empty(), "expected no events, got {:?}", events);
    }
}
