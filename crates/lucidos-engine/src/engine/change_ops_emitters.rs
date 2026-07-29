//! `LucidosEngine` event emit helpers for the changes lifecycle. Each
//! method wraps `event_bus.emit_or_log` for one ChangeProposed-related
//! `ThreadEvent` (or the `ChangesUpdated` system event), so the call sites
//! in `change_ops.rs` stay free of the boilerplate.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_events::{EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// Broadcast the current changes state (pending/applied/restart) to all SSE clients.
    /// On any DB read failure, skip the broadcast — the next event-driven emit
    /// will retry, and an SSE payload with stale/wrong counts is worse than
    /// none at all (clients would render emptied lists and "all clear" badges).
    pub(crate) async fn broadcast_changes_updated(&self) {
        let proj = self.changes();
        // UNIX_EPOCH ≈ "since forever" for restart-required tracking. Don't use
        // `DateTime::MIN_UTC` — its year (-262143) is outside Postgres
        // timestamptz range (4713 BC … 294276 AD), so binding it errors with
        // "timestamp out of range" and the helper falls back to false, hiding
        // any actual restart-required change.
        let (pending_r, applied_r, restart_r) = tokio::join!(
            proj.list_pending(),
            proj.list_recently_applied(15, None),
            proj.requires_restart_since(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH),
        );
        let (mut pending, mut applied, restart) = match (pending_r, applied_r, restart_r) {
            (Ok(p), Ok(a), Ok(r)) => (p, a, r),
            (perr, aerr, rerr) => {
                if let Err(e) = perr { log!("[Changes] broadcast_changes_updated: list_pending: {}", e); }
                if let Err(e) = aerr { log!("[Changes] broadcast_changes_updated: list_recently_applied: {}", e); }
                if let Err(e) = rerr { log!("[Changes] broadcast_changes_updated: requires_restart_since: {}", e); }
                log!("[Changes] broadcast_changes_updated: skipping ChangesUpdated emit");
                return;
            }
        };
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
        let error_msg = "Hardening did not complete (no marker recorded). Click Apply again to retry.";
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        change_id: change_id.to_string(),
                        error: error_msg.to_string(),
                        actor,
                    },
                    meta: EventMeta::NONE,
                },
                log_tag,
            )
            .await;
        // Feed the Apply-All driver — without this an Apply All batch with
        // an unhardened member would stall when hardening fails.
        if let Ok(id) = Uuid::parse_str(change_id) {
            self.notify_apply_all(
                crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
                    id,
                    error_msg.to_string(),
                ),
            );
        }
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
        // `emit` rather than `emit_or_log` because the post-apply refresh below
        // MUST see whether this emit was accepted. The bus runs a single-fire
        // guard for `ChangeApplied` (`FOR UPDATE` on the change row) and returns
        // `Ok(None)` for a recognized duplicate — the double-fire race, an
        // Apply-All retry, the conflict-recovery cleanup, a post-restart re-emit.
        // Firing the refresh on the DUPLICATE would coalesce (abort) the rebuild
        // the ACCEPTED emit just started, and the replacement build can then read
        // `SkippedLocked` and fall back to `Idle` — no build running at all,
        // which is the exact failure this funnel exists to prevent.
        let accepted = match self
            .event_bus
            .emit(BusEvent::Thread {
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
            })
            .await
        {
            Ok(res) => res.is_some(),
            Err(e) => {
                // Same message shape `emit_or_log` produced, so the existing
                // `[EventBus] [Changes] ChangeApplied emit failed: …` stays greppable.
                log!("[EventBus] [Changes] ChangeApplied emit failed: {}", e);
                false
            }
        };
        // Feed the Apply-All driver. Non-member ids are no-ops; batch
        // members advance the registry and (if more remain) trigger the
        // next apply. Non-blocking — the channel send hands off to the
        // driver task, never holds a lock.
        //
        // Deliberately NOT gated on `accepted`, unlike the refresh below: batch
        // member status is first-write-wins, so a duplicate `Applied` is already
        // idempotent here — while withholding it on a suppressed duplicate could
        // strand a batch whose only signal was that duplicate.
        self.notify_apply_all(
            crate::engine::apply_all_driver::ApplyAllDriveMsg::Applied(change_id),
        );
        if accepted {
            self.post_apply_dev_refresh(thread_id, requires_restart, client_update)
                .await;
        }
    }

    /// Advance what dev serves after a change LANDED on main — the single
    /// funnel for both halves of the *Switch to new version* flow.
    ///
    /// Lives here, next to the `ChangeApplied` emit, because that emit is the
    /// one thing every merge path performs exactly once: the `apply_change`
    /// tiers, the already-merged fast path, the live in-place merge
    /// (`apply_now_success`), the async in-place conflict recovery, and the
    /// Tier-2/3 conflict-resolution cleanup in `run_session/completion.rs`.
    /// It used to hang off the `apply_change` wrapper alone, so the three
    /// paths that bypass that wrapper merged an engine-affecting change
    /// without ever starting a rebuild — no build, no spinner, and a "Switch
    /// to new version" pointing at a binary that predates the merge.
    ///
    /// No-op when the engine self-reference isn't installed (unit tests) and,
    /// inside each helper, when packaged (packaged never rebuilds from source).
    async fn post_apply_dev_refresh(
        &self,
        thread_id: Uuid,
        requires_restart: bool,
        client_update: bool,
    ) {
        let Some(engine) = self.try_clone_arc() else {
            // `main.rs` installs the self-reference immediately after `Arc::new`,
            // long before the HTTP server binds — so in a real engine this is
            // unreachable and only unit tests take it. Log it anyway: a silently
            // skipped rebuild is precisely the failure this funnel exists to
            // stop, and it must not be invisible if the wiring ever moves.
            log!(
                "[Changes] post-apply refresh skipped for change on thread {} — engine self-reference not installed",
                thread_id
            );
            return;
        };
        // One indexed `thread_summaries` lookup, on a user-initiated action —
        // and it gates BOTH arms, so it can't be skipped for the restart case.
        let is_lucidos_source =
            crate::engine::change_ops::load_apply_kind_context(self.pool(), Some(thread_id))
                .await
                .is_lucidos_source();
        // Log the DECISION, not just the action. Diagnosing the bug this funnel
        // fixes meant comparing binary mtimes against merge times, because the
        // log recorded the merge succeeding and then said nothing about whether
        // a rebuild had been asked for.
        let decision = post_apply_refresh(requires_restart, client_update, is_lucidos_source);
        log!(
            "[Changes] post-apply refresh: {:?} (requires_restart={}, client_update={})",
            decision,
            requires_restart,
            client_update
        );
        match decision {
            PostApplyRefresh::RebuildEngine => engine.trigger_background_rebuild(),
            PostApplyRefresh::ReSnapshotFrontend => {
                engine.refresh_served_frontend_after_rebuild()
            }
            PostApplyRefresh::Nothing => {}
        }
    }
    /// Scan files touched by an applied change and emit per-entity events
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
    pub(crate) async fn emit_entity_events_for_change_apply(
        &self,
        files: &[String],
        pre_merge_sha: Option<&str>,
        post_merge_sha: Option<&str>,
        actor: Option<MessageOrigin>,
    ) {
        let events = compute_entity_events_for_change_apply(
            self.workspace_path(),
            pre_merge_sha,
            post_merge_sha,
            files,
            actor,
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
        // Feed the Apply-All driver. Failed members do NOT halt the batch
        // — the driver records the failure and keeps applying the rest.
        self.notify_apply_all(crate::engine::apply_all_driver::ApplyAllDriveMsg::Failed(
            change_id,
            error.to_string(),
        ));
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
                        channel: Some(EventChannel::ClaudeCode),
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
                        channel: Some(EventChannel::ClaudeCode),
                        ..EventMeta::NONE
                    },
                },
                "[Changes] MergeConflictDetected",
            )
            .await;
    }

    /// Emit `MergeConflictDetected` AND build the merge prompt to send to CC.
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
    pub(crate) async fn start_merge_and_get_prompt(
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
        // App threads get an app-appropriate merge prompt (no `/harden`, no
        // Lucidos-source test suites). Resolved here from the thread so every
        // merge call site (in-session apply, Tier-2/Tier-3 spawn) inherits it.
        let is_app = crate::engine::change_ops::load_apply_kind_context(&self.pool, Some(thread_id))
            .await
            .is_app();
        crate::engine::agent_session::build_merge_prompt(
            target_branch,
            body_intro,
            description,
            is_app,
        )
    }
}

/// Engine-free core of `emit_entity_events_for_change_apply`. Scans the file
/// list, checks pre/post existence, and returns the set of `SystemEvent`s to
/// emit. Pure modulo two side effects: reading the on-disk manifest for app
/// names, and shelling out to `git cat-file -e` against `pre_merge_sha`.
/// Extracted so tests can drive the logic with a temp git workspace and no
/// `LucidosEngine`.
pub(crate) async fn compute_entity_events_for_change_apply(
    workspace_path: &std::path::Path,
    pre_merge_sha: Option<&str>,
    post_merge_sha: Option<&str>,
    files: &[String],
    actor: Option<MessageOrigin>,
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
        let event = match (exists_post, exists_pre) {
            (true, false) => Some(SystemEvent::ArtifactCreated {
                artifact_path: path.clone(),
                commit: commit.clone(),
                source: Some("change_apply".to_string()),
            }),
            (true, true) => Some(SystemEvent::ArtifactUpdated {
                artifact_path: path.clone(),
                commit: commit.clone(),
                source: Some("change_apply".to_string()),
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

/// Return true iff `path` exists in the tree at `sha`. Uses `git cat-file -e`
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
            // exit 1 = "missing" (expected); anything else (timeout from the
            // 30s wrapper, repo broken, sha unreadable) is suspicious.
            if o.status.code() != Some(1) {
                log!(
                    "[Changes] git cat-file -e {} exited {:?} stderr={}",
                    spec,
                    o.status.code(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            false
        }
        Err(e) => {
            log!("[Changes] git cat-file -e {} failed: {}", spec, e);
            false
        }
    }
}

/// Read the app's display name from `data/apps/<id>/manifest.json` for the
/// `name` field on `AppCreated`/`AppUpdated`. Returns `None` if the manifest
/// can't be read or doesn't have a string `name` — the event payload's `name`
/// is already `Option<String>`, so consumers handle the missing case.
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

/// What dev has to do to start serving a change that just merged into main.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostApplyRefresh {
    /// Engine-affecting change: kick the background `cargo build` so the
    /// on-disk binary advances and "New version available → Switch to new
    /// version" surfaces. Apply itself never restarts the engine.
    RebuildEngine,
    /// Frontend-only Lucidos-source change (engine binary unchanged): the
    /// switch flow won't run, so re-snapshot the rebuilt `dist/` in-process.
    ReSnapshotFrontend,
    /// Nothing to advance — an app / external-repo change, or a diff that
    /// touches neither the engine nor the served client.
    Nothing,
}

/// Decide the post-apply refresh from the merged change's shape — the pure
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// An engine-affecting change ALWAYS rebuilds, whatever else it touched.
    /// This is the arm that regressed: a mixed Rust+TS Apply from a live
    /// coding-agent session merged and then sat there with no build running,
    /// no spinner, and a Switch offer pointing at the pre-merge binary.
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

    /// Frontend-only Lucidos-source change: no new binary to switch onto, so
    /// the served `dist/` snapshot has to advance in-process instead.
    #[test]
    fn frontend_only_lucidos_source_change_resnapshots_the_served_frontend() {
        assert_eq!(
            post_apply_refresh(false, true, true),
            PostApplyRefresh::ReSnapshotFrontend
        );
    }

    /// App / external-repo changes never advance the host's engine binary or
    /// its served client — their `.ts`/`.css` files live in the data tree.
    #[test]
    fn non_lucidos_source_change_advances_nothing() {
        assert_eq!(
            post_apply_refresh(false, true, false),
            PostApplyRefresh::Nothing
        );
    }

    /// The kind gate binds the RESTART arm too. An *external-repo coding-agent
    /// thread* derives `requires_restart` from `files_require_restart` run over
    /// someone else's tree, so a user-registered repo that merely contains Rust
    /// arrives here flagged restart-requiring — rebuilding Lucidos's own engine
    /// for it would be plainly wrong. (App changes are forced to `false`
    /// upstream; external-repo ones are not, which is why this can't lean on
    /// the caller's kind adjustment.)
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

    /// A docs-only / data-only Lucidos-source change touches neither surface.
    #[test]
    fn change_touching_neither_surface_advances_nothing() {
        assert_eq!(
            post_apply_refresh(false, false, true),
            PostApplyRefresh::Nothing
        );
    }

    /// Initialise a fresh git repo at `path` and commit an empty `data/.keep`
    /// so the tree has a root and we can take its SHA. Returns the initial
    /// commit's SHA.
    async fn init_repo(path: &std::path::Path) -> String {
        std::fs::create_dir_all(path.join("data")).unwrap();
        std::fs::write(path.join("data/.keep"), "").unwrap();
        git_cmd(&["init", "-q", "-b", "main"], path).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], path)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], path).await.unwrap();
        git_cmd(&["add", "-A"], path).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "init"], path).await.unwrap();
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
    async fn cc_apply_creating_a_new_app_emits_app_created() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let pre_sha = init_repo(ws).await;

        // Simulate CC's effect: write the app's files into main post-merge.
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
        )
        .await;

        assert_eq!(events.len(), 1, "expected exactly one app event, got {:?}", events);
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
        // Pre-state already has the app.
        write_app_manifest(ws, "habit-tracker", "Habit Tracker");
        git_cmd(&["init", "-q", "-b", "main"], ws).await.unwrap();
        git_cmd(&["config", "user.email", "t@e.com"], ws).await.unwrap();
        git_cmd(&["config", "user.name", "T"], ws).await.unwrap();
        git_cmd(&["add", "-A"], ws).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "seed"], ws).await.unwrap();
        let pre_sha = String::from_utf8(
            git_cmd(&["rev-parse", "HEAD"], ws).await.unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // CC rewrote index.html — manifest still present, name unchanged.
        std::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<html>v2</html>").unwrap();
        let post_sha = commit_all(ws, "Edit habit-tracker").await;

        let events = compute_entity_events_for_change_apply(
            ws,
            Some(&pre_sha),
            Some(&post_sha),
            &["data/apps/habit-tracker/index.html".to_string()],
            None,
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
        git_cmd(&["config", "user.email", "t@e.com"], ws).await.unwrap();
        git_cmd(&["config", "user.name", "T"], ws).await.unwrap();
        git_cmd(&["add", "-A"], ws).await.unwrap();
        git_cmd(&["commit", "-q", "-m", "seed"], ws).await.unwrap();
        let pre_sha = String::from_utf8(
            git_cmd(&["rev-parse", "HEAD"], ws).await.unwrap().stdout,
        )
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
        )
        .await;

        // One app touched → one event (not two: dedup on app_id).
        assert_eq!(events.len(), 1, "got {:?}", events);
        assert!(
            matches!(&events[0], SystemEvent::AppDeleted { app_id, .. } if app_id == "going-away"),
            "got {:?}",
            events[0]
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
        )
        .await;

        assert_eq!(events.len(), 1, "got {:?}", events);
        match &events[0] {
            SystemEvent::ArtifactCreated { artifact_path, commit, source } => {
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

        // Files outside data/apps/ and data/artifacts/ should not produce events.
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
        )
        .await;

        assert!(events.is_empty(), "expected no events, got {:?}", events);
    }
}
