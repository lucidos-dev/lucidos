//! Engine-version identity + "new version available" detection.
//!
//! The dev half of the unified *Switch to new version* flow (see
//! `docs/plans/2026-07-01-new-engine-version-switch-flow.md`): after an *Apply*
//! rebuilds the engine binary on disk in the background, a running engine detects
//! that its on-disk binary differs from the version it's running and surfaces the
//! switch. Mirrors the gateway self-reload (`GATEWAY_BUILD_ID` /
//! `gateway_update_available`).
//!
//! Packaged builds never rebuild from source — `update_available` is always false
//! here (their "new version" source is the release updater, `updater.rs`) — and
//! `BuildState` stays `Idle`.

use super::LucidosEngine;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a `source_behind_head` verdict is reused before the underlying
/// `git diff` is re-run. version-status is polled ~4s per client; a shared TTL
/// bounds the git fork to at most one per interval regardless of client count.
/// Short enough that a freshly-merged engine change surfaces within a few seconds.
const SOURCE_BEHIND_TTL: Duration = Duration::from_secs(3);

/// Max background rebuilds the self-heal driver auto-triggers for a single HEAD
/// before giving up (until HEAD moves). Bounds a genuinely broken `main` so it
/// can't spin builds forever; an Apply or a manual rebuild is always still
/// available. Debug builds are ~1 min, so this is a few minutes of retrying.
const SELF_HEAL_MAX_ATTEMPTS_PER_HEAD: u32 = 5;

/// Dev background-rebuild state. `Idle` in packaged (no source rebuild) and
/// between Applies; the Phase-2 build orchestration drives the transitions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BuildState {
    /// No rebuild running and none pending.
    #[default]
    Idle,
    /// A background `cargo build` is in progress; the old engine keeps serving.
    Building,
    /// The rebuild finished; a newer on-disk binary is ready to switch onto.
    Ready,
    /// The rebuild failed (compile error). The old engine keeps running; the
    /// error is surfaced as "Build failed — view log".
    Failed,
}

impl BuildState {
    /// Kebab-case wire tag for the version-status response.
    pub fn as_wire(&self) -> &'static str {
        match self {
            BuildState::Idle => "idle",
            BuildState::Building => "building",
            BuildState::Ready => "ready",
            BuildState::Failed => "failed",
        }
    }
}

/// Memoized on-disk binary build id, keyed by the running binary's last-seen
/// mtime. A `None` mtime means "not yet checked"; a `None` `disk_build_id` means
/// the id couldn't be read (binary mid-rewrite / spawn failure / packaged).
#[derive(Default)]
pub struct UpdateCheck {
    last_mtime: Option<std::time::SystemTime>,
    disk_build_id: Option<String>,
}

/// Throttled cache of the `source_behind_head` verdict — see
/// [`LucidosEngine::source_behind_head`]. `checked_at == None` means "never
/// computed"; `behind` is the last verdict. TTL-gated (`SOURCE_BEHIND_TTL`), so
/// the git probe runs at most once per interval across all polling clients.
#[derive(Default)]
pub struct SourceBehindCache {
    checked_at: Option<Instant>,
    behind: bool,
}

/// Memoized ancestry verdict for the on-disk binary — see
/// [`LucidosEngine::disk_binary_is_upgrade`]. Keyed by the on-disk build id
/// (the RUNNING id is a compile-time constant, so the pair is fully identified
/// by the disk side). `is_strict_ancestor` is the cached
/// `git merge-base --is-ancestor <disk> <running>` answer; `None` means git
/// couldn't tell (unknown commit, no repo, command failure).
#[derive(Default)]
pub struct DiskDirectionCache {
    disk_id: Option<String>,
    is_strict_ancestor: Option<bool>,
}

/// Per-HEAD self-heal attempt bookkeeping — see
/// [`LucidosEngine::self_heal_engine_version_if_needed`]. `head` is the HEAD the
/// attempts were counted for; when HEAD moves the counter resets.
#[derive(Default)]
pub struct SelfHealState {
    head: Option<String>,
    attempts: u32,
}

/// Wire shape of `GET /api/v1/engine/version-status`.
#[derive(Serialize)]
pub struct VersionStatus {
    /// The running engine's baked build id.
    pub build_id: String,
    /// True when a newer engine version is ready to switch onto (dev: on-disk
    /// binary build-id differs; always false in packaged — see module docs).
    pub update_available: bool,
    /// The on-disk binary's build id (dev), or `None` when packaged / unreadable.
    /// The frontend keys the "Switch to new version" badge+toast dismissal on this
    /// so a dismiss sticks for THIS on-disk build but a genuinely newer build
    /// re-surfaces the switch (INV-C, plan 2026-07-02).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_build_id: Option<String>,
    /// True under the packaged desktop/service runtime. The frontend uses it to
    /// route to the release updater instead of the dev build/switch flow.
    pub packaged: bool,
    /// Dev background-rebuild state: `idle` | `building` | `ready` | `failed`.
    pub build_state: &'static str,
    /// True (dev only) when the engine SOURCE is behind HEAD by a
    /// restart-requiring change — i.e. a NEW engine version exists in source even
    /// if no fresh binary is on disk yet (rebuild failed / not run). Distinct from
    /// `update_available` (a fresh binary IS on disk). The frontend uses it to
    /// surface a pending new version + drive self-heal so the Switch can't be a
    /// dead-end. Always false packaged / when git is unavailable.
    pub source_behind_head: bool,
    /// True (dev only) when the checkout-shared engine-build lock is currently
    /// held — a background rebuild of the shared binary is in flight, by a
    /// CO-LOCATED peer engine's `run_engine_build` OR by this engine's own build.
    /// Co-located workspaces share ONE `target/` + ONE build lock, so a peer's
    /// build advances the binary THIS engine serves; without this flag a workspace
    /// that lost the lock (its own rebuild `SkippedLocked` → `build_state` back to
    /// `idle`) can't tell a build is running and wrongly shows the manual
    /// "Rebuild" escape hatch instead of the building spinner. Named "shared" (not
    /// "peer") because the probe can't distinguish this engine's own held lock
    /// from a peer's; the frontend disambiguates via `build_state`. Always false
    /// packaged (never rebuilds from source).
    pub shared_build_in_progress: bool,
}

impl LucidosEngine {
    /// Stash the device actor at switch-request time so the graceful-shutdown
    /// boundary emit (which runs in the signal handler, with no HTTP context) can
    /// attribute the switch to "You" and recovery can auto-resume in-flight
    /// coding-agent threads. Called by the `/api/v1/restart` switch handler.
    pub fn set_restart_actor(&self, actor: Option<crate::engine::thread_events::MessageOrigin>) {
        *self.restart_actor.lock().unwrap() = actor;
    }

    /// Take (and clear) the stashed switch actor. Cleared so a later non-switch
    /// stop (stop.sh, external SIGUSR1) doesn't reuse a stale device actor — it
    /// then falls back to System attribution + manual Continue.
    pub fn take_restart_actor(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.restart_actor.lock().unwrap().take()
    }

    /// Enqueue a thread for auto-resume after a user-initiated switch (recovery).
    pub(crate) fn enqueue_switch_resume(&self, thread_id: uuid::Uuid) {
        self.pending_switch_resumes.lock().unwrap().push(thread_id);
    }

    /// Emit `ContinuationRequested` for every thread recovery queued for
    /// auto-resume after a user-initiated switch, so the spawn dispatcher boots a
    /// `--resume`. Called by `main.rs` AFTER `SpawnDispatcher::spawn()` — which
    /// opens the broadcast subscription synchronously before it returns, so these
    /// emits are buffered by the receiver even while the dispatcher's startup
    /// backfill is still running (recovery itself runs before the dispatcher
    /// exists, so the emit can't happen inside recovery). Engine-attributed
    /// (`actor: None`): the resume is a recovery consequence, not a device click,
    /// and the "Resumed" boundary keeps the Lucidos-mark chip.
    pub async fn resume_pending_switches(&self) {
        let ids = std::mem::take(&mut *self.pending_switch_resumes.lock().unwrap());
        for thread_id in ids {
            // A prior boot may have left this thread an unactuated
            // ContinuationRequested (emitted but never spawned — the zombie
            // state). The dispatcher's startup orphan re-dispatch drives that
            // existing request; emitting a second one here would put two
            // request event ids on one thread — past the per-EVENT idempotency
            // guard — and double-spawn.
            if crate::engine::agent_session::spawn_dispatcher::thread_has_unactuated_continuation(
                self.pool(),
                thread_id,
            )
            .await
            {
                log!(
                    "[Recovery] thread {} already has an unactuated ContinuationRequested — \
                     the dispatcher's startup orphan re-dispatch owns the resume; skipping duplicate emit",
                    thread_id
                );
                continue;
            }
            crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON,
                None,
                "[Recovery] ContinuationRequested (auto-resume after switch)",
            )
            .await;
        }
    }

    /// Current background-rebuild state.
    pub fn build_state(&self) -> BuildState {
        self.build_state.read().unwrap().clone()
    }

    /// Set the background-rebuild state (Phase 2 build orchestration).
    pub fn set_build_state(&self, state: BuildState) {
        *self.build_state.write().unwrap() = state;
    }

    /// The on-disk engine binary's build id (dev), or `None` when packaged or the
    /// id can't be read (binary mid-rewrite / spawn failure). Cheap on the steady
    /// path: only forks `current_exe --build-id` when the binary's mtime has moved
    /// since the last check; otherwise reuses the cached id. Mirrors
    /// `gateway_update_available`'s memoization.
    pub async fn engine_disk_build_id(&self) -> Option<String> {
        if crate::runtime::is_packaged() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let mtime = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
        // Fast path: mtime unchanged since the last check → reuse the cached id.
        {
            let cache = self.update_check.lock().unwrap();
            if cache.last_mtime == mtime {
                return cache.disk_build_id.clone();
            }
        }
        // mtime moved (or first check): ask the on-disk binary for its build id.
        let disk_id = match tokio::process::Command::new(&exe)
            .arg("--build-id")
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                (!id.is_empty()).then_some(id)
            }
            // Unreadable (mid-rewrite / failure): leave the cache untouched so the
            // next poll retries once the mtime settles.
            _ => return self.update_check.lock().unwrap().disk_build_id.clone(),
        };
        let mut cache = self.update_check.lock().unwrap();
        cache.last_mtime = mtime;
        cache.disk_build_id = disk_id.clone();
        disk_id
    }

    /// Is the on-disk binary a genuine UPGRADE target for this running engine —
    /// i.e. is switching onto it a step FORWARD?
    ///
    /// The naive test is `disk_build_id != ENGINE_BUILD_ID`, but *different* is not
    /// *newer*. Co-located dev workspaces share one checkout and one published
    /// launch binary (`target/<profile>/launch/<variant>/lucidos-engine`, ADR 0022),
    /// so the binary on disk is routinely written by something other than this
    /// workspace — a peer's `--engine-build`, or a build whose `build.rs` ran
    /// before an Apply moved HEAD. When what lands is OLDER than the running
    /// engine, the difference test announces "New version available" for a
    /// **downgrade**: switching respawns onto the old binary, which is then
    /// genuinely behind HEAD, so self-heal rebuilds every 10s, re-surfaces the
    /// switch, and the pair ping-pongs forever (the 2026-07-26 endless-toast loop;
    /// `docs/plans/2026-07-26-downgrade-switch-toast-loop.md`). Publishing per
    /// build variant removed the *wrong-configuration* half of that (ADR 0022 /
    /// `docs/plans/2026-07-27-launch-binary-published-per-variant.md`); this guard
    /// remains the authority on direction.
    ///
    /// So direction is decided by git ancestry over the two ids' commit prefixes:
    /// a disk commit that is a STRICT ANCESTOR of the running commit is provably
    /// older and vetoes the update. Everything indeterminate — unrelated commits, a
    /// `src-…` (no-git) id, an unparseable id, git unavailable — falls back to the
    /// difference test, so this removes a false positive without adding a way to
    /// MISS a real update (a missed update strands the user on an old engine, which
    /// is the worse failure).
    ///
    /// Cheap: the ancestry answer is memoized per on-disk build id
    /// ([`DiskDirectionCache`]) — the running id is a compile-time constant, so the
    /// pair is identified by the disk side alone — and only consulted when the ids
    /// actually differ. `git merge-base` therefore runs at most once per distinct
    /// on-disk binary, not once per ~4s poll.
    pub async fn disk_binary_is_upgrade(&self) -> bool {
        let disk = self.engine_disk_build_id().await;
        self.disk_id_is_upgrade(disk.as_deref()).await
    }

    /// [`Self::disk_binary_is_upgrade`] for an on-disk id the caller already read,
    /// so `version_status` reports the id and the verdict from ONE read — they
    /// can't straddle an mtime change and disagree.
    async fn disk_id_is_upgrade(&self, disk: Option<&str>) -> bool {
        let Some(disk) = disk else {
            return false; // packaged / unreadable — nothing to switch onto
        };
        if disk == crate::ENGINE_BUILD_ID {
            return false; // identical build — no update, no git needed
        }
        disk_upgrade_verdict(
            Some(disk),
            crate::ENGINE_BUILD_ID,
            self.disk_is_older(disk).await,
        )
    }

    /// Cached `git merge-base --is-ancestor <disk-commit> <running-commit>`:
    /// `Some(true)` = the on-disk binary is provably OLDER, `Some(false)` = it
    /// isn't, `None` = git couldn't tell. Logs the downgrade case once per distinct
    /// on-disk build id (the cache makes that natural) so the wedge is diagnosable
    /// from the engine log rather than only from a `version-status` fetch.
    async fn disk_is_older(&self, disk_id: &str) -> Option<bool> {
        {
            let cache = self.disk_direction_cache.lock().unwrap();
            if cache.disk_id.as_deref() == Some(disk_id) {
                return cache.is_strict_ancestor;
            }
        }
        let verdict = match (
            build_id_commit(disk_id),
            build_id_commit(crate::ENGINE_BUILD_ID),
        ) {
            (Some(disk_commit), Some(running_commit)) if disk_commit != running_commit => {
                match crate::paths::repo_root() {
                    Ok(root) => commit_is_strict_ancestor(&root, disk_commit, running_commit).await,
                    Err(_) => None,
                }
            }
            // Same commit (differing only in the uncommitted-diff suffix) → the disk
            // binary is not an older COMMIT; a rebuilt dirty tree is a real update.
            (Some(_), Some(_)) => Some(false),
            // A `src-…` / empty id on either side → no commit to compare.
            _ => None,
        };
        if verdict == Some(true) {
            crate::log!(
                "[Rebuild] on-disk engine binary ({}) is OLDER than the running engine ({}) — \
                 not offering a downgrade as a new version. Something rebuilt an earlier checkout \
                 state over target/debug; `web-dev.sh -w <ws> -b` rebuilds it forward.",
                disk_id,
                crate::ENGINE_BUILD_ID
            );
        }
        let mut cache = self.disk_direction_cache.lock().unwrap();
        cache.disk_id = Some(disk_id.to_string());
        cache.is_strict_ancestor = verdict;
        verdict
    }

    /// Whether the engine SOURCE is behind HEAD by a restart-requiring change —
    /// a NEW engine version exists in source even if no fresh binary is on disk
    /// yet. Reuses [`Self::engine_source_matches_head`] (the SAME git classifier
    /// the frontend-only-Apply veto uses), so this signal and that veto agree by
    /// construction. TTL-cached ([`SOURCE_BEHIND_TTL`]) so the `git diff` runs at
    /// most once per interval across all polling clients. Dev-only: false packaged.
    ///
    /// Direction-guarded for the same reason [`Self::disk_binary_is_upgrade`] is:
    /// `engine_source_matches_head` compares the two trees with a two-dot
    /// `git diff <running-commit> HEAD`, which is symmetric — it reports a
    /// difference whether HEAD is ahead of the running engine or *behind* it. An
    /// engine running a commit that HEAD is an ancestor of is not behind anything,
    /// so claiming otherwise would pin a permanent "New engine version pending"
    /// toast and a 10s self-heal build storm on a workspace that is already current.
    pub async fn source_behind_head(&self) -> bool {
        if crate::runtime::is_packaged() {
            return false;
        }
        {
            let cache = self.source_behind_cache.lock().unwrap();
            if cache
                .checked_at
                .is_some_and(|at| at.elapsed() < SOURCE_BEHIND_TTL)
            {
                return cache.behind;
            }
        }
        // `Some(false)` = a restart-requiring change is pending between the running
        // engine's commit and HEAD (a new engine version). `Some(true)`/`None`
        // (frontend-only / git unavailable) → not behind.
        let behind = self.engine_source_matches_head().await == Some(false)
            && !self.running_is_ahead_of_head().await;
        let mut cache = self.source_behind_cache.lock().unwrap();
        cache.checked_at = Some(Instant::now());
        cache.behind = behind;
        behind
    }

    /// Is the running engine's commit a strict DESCENDANT of HEAD — i.e. is this
    /// engine ahead of the checkout rather than behind it? The direction guard for
    /// [`Self::source_behind_head`]; `false` whenever git can't tell, so an
    /// indeterminate answer leaves today's behavior untouched.
    async fn running_is_ahead_of_head(&self) -> bool {
        let Some(running_commit) = build_id_commit(crate::ENGINE_BUILD_ID) else {
            return false; // `src-…` / unstamped id — no commit to compare
        };
        let Ok(root) = crate::paths::repo_root() else {
            return false;
        };
        let Some(head) = current_head_sha().await else {
            return false;
        };
        commit_is_strict_ancestor(&root, &head, running_commit).await == Some(true)
    }

    /// Full version status for `GET /api/v1/engine/version-status`. A newer engine
    /// is available when the on-disk binary is readable, differs from the running
    /// one, and is not provably OLDER than it ([`Self::disk_binary_is_upgrade`] —
    /// always false in packaged, where `disk_build_id` is `None`).
    pub async fn version_status(&self) -> VersionStatus {
        let disk_build_id = self.engine_disk_build_id().await;
        let update_available = self.disk_id_is_upgrade(disk_build_id.as_deref()).await;
        let source_behind_head = self.source_behind_head().await;
        // Cheap non-blocking flock try (a couple of syscalls) — no TTL cache
        // needed, unlike `source_behind_head` which forks `git`. False packaged.
        // Uses the fail-OPEN-to-false probe (an indeterminate probe must not read
        // as "a build is running" — that would hide the Rebuild escape hatch).
        let shared_build_in_progress =
            !crate::runtime::is_packaged() && shared_engine_build_lock_held();
        VersionStatus {
            build_id: crate::ENGINE_BUILD_ID.to_string(),
            update_available,
            disk_build_id,
            packaged: crate::runtime::is_packaged(),
            build_state: self.build_state().as_wire(),
            source_behind_head,
            shared_build_in_progress,
        }
    }

    /// One periodic self-heal tick (dev only): if the engine SOURCE is behind
    /// HEAD by a restart-requiring change but no fresh binary is on disk,
    /// (re)trigger a background rebuild so the shared binary advances and the
    /// Switch surfaces — WITHOUT a manual `web-dev.sh -b`. Closes the wedge where a
    /// mixed Apply's rebuild failed (or was never run) and left every co-located
    /// workspace deferring frontend-only Applies to a Switch that never appeared.
    ///
    /// Coordinated + bounded:
    /// - Skips when a co-located engine is already building (shared-lock probe),
    ///   so the N workspaces don't stampede the shared `target/`.
    /// - Skips when a genuine UPGRADE is already on disk
    ///   ([`Self::disk_binary_is_upgrade`]) — the Switch is already surfaced via
    ///   `update_available`; switching, not rebuilding, is next. Deliberately the
    ///   SAME question `update_available` asks, so the two can never disagree: a
    ///   bare `disk != running` here would also read an OLDER on-disk binary as
    ///   "fresh" and suppress the very rebuild that would fix it.
    /// - Bounded to [`SELF_HEAL_MAX_ATTEMPTS_PER_HEAD`] per HEAD so a genuinely
    ///   broken `main` can't spin builds forever; the count resets when HEAD moves.
    /// - Gives up immediately once a rebuild it triggered SUCCEEDED without
    ///   advancing the binary ([`self_heal_is_wedged`]) — retrying cannot fix that.
    ///
    /// No-op packaged / when git is unavailable. Driven by the dev periodic loop
    /// (`frontend_refresh::spawn_served_frontend_sync`).
    pub(crate) async fn self_heal_engine_version_if_needed(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        // Is a new engine version pending in source?
        if !self.source_behind_head().await {
            *self.self_heal_state.lock().unwrap() = SelfHealState::default();
            return;
        }
        // A rebuild (self-heal's or the Apply path's) is already in flight → wait.
        if self.build_state() == BuildState::Building {
            return;
        }
        // An upgrade is already on disk → the Switch is surfaced via
        // `update_available`; don't rebuild (switching is the next step). An
        // unreadable disk id reads as "no upgrade" and falls through to the rebuild,
        // which is the safe direction: at worst we rebuild a binary that was already
        // fine, and the attempt cap bounds it.
        if self.disk_binary_is_upgrade().await {
            return;
        }
        let head = current_head_sha().await;
        // Read the build state at DECISION time, not before the awaits above — a
        // rebuild can start (Apply) or finish during them, and judging "did my
        // rebuild succeed?" from a stale value would give up on a live build.
        let build_state = self.build_state();
        {
            let mut sh = self.self_heal_state.lock().unwrap();
            if sh.head != head {
                sh.head = head;
                sh.attempts = 0;
            }
            // Budget spent for this HEAD → stay silent until a new commit lands.
            // Checked BEFORE the wedge branch below so the give-up is announced
            // exactly once: the wedge condition stays true on every later tick
            // (nothing clears Ready), and re-evaluating it first would re-log the
            // same line every 10s — the very noise this whole change removes.
            if sh.attempts >= SELF_HEAL_MAX_ATTEMPTS_PER_HEAD {
                return;
            }
            // A rebuild we triggered for this HEAD finished successfully and STILL
            // left no upgrade on disk. That is a wedged build configuration (a
            // no-op cargo run whose uplifted binary predates the running engine, a
            // peer overwriting `target/debug` from an older checkout state), and no
            // number of retries fixes it — burn the budget and say so once instead
            // of rebuilding every 10s forever (the 152-build storm of 2026-07-26).
            if self_heal_is_wedged(sh.attempts, &build_state) {
                sh.attempts = SELF_HEAL_MAX_ATTEMPTS_PER_HEAD;
                crate::log!(
                    "[Rebuild] self-heal: a rebuild succeeded but the on-disk binary is still not \
                     newer than the running engine ({}) — giving up for this HEAD. Rebuild \
                     manually with `web-dev.sh -w <ws> -b`.",
                    crate::ENGINE_BUILD_ID
                );
                return;
            }
            // Don't stampede a peer that's already building (racy probe; the
            // in-build lock in `run_engine_build` is the real guard). Quick
            // synchronous flock probe — no await while holding the state lock.
            if engine_build_in_progress_elsewhere() {
                return;
            }
            sh.attempts += 1;
        }
        crate::log!(
            "[Rebuild] self-heal: engine source is behind HEAD with a stale binary — triggering a background rebuild"
        );
        self.trigger_background_rebuild();
    }

    /// Kick off a background engine rebuild (dev only), the non-disruptive half of
    /// the *Switch to new version* flow. The running engine keeps serving; when the
    /// rebuild finishes, its on-disk binary's build-id differs from the running
    /// `ENGINE_BUILD_ID` (surfaced via `version_status`) → the frontend surfaces
    /// "New version available". A second call **coalesces**: the in-flight build is aborted (its
    /// child is `kill_on_drop`, so the cargo process dies) and a fresh build starts,
    /// so the build always reflects the latest merged source. No-op in packaged —
    /// it never rebuilds from source (its "new version" is the release updater).
    pub fn trigger_background_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let generation = self.build_generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Coalesce: abort any running build task; its child is kill_on_drop.
        if let Some(old) = self.build_task.lock().unwrap().take() {
            old.abort();
        }
        self.set_build_state(BuildState::Building);
        let engine = self.clone();
        let workspace = self.workspace_path().to_path_buf();
        let handle = tokio::spawn(async move {
            // Push `building` over SSE so a connected client shows the spinner
            // immediately — the 4s version-status poll alone misses this transient
            // window (iOS suspends the timer on a backgrounded PWA), so the spinner
            // badge never showed. See engine_version::emit_build_state_changed.
            engine.emit_build_state_changed(&BuildState::Building).await;
            let outcome = run_engine_build(&workspace).await;
            // Only the latest generation updates state — a superseded build's
            // completion is ignored (a newer build is already `Building`).
            if engine.build_generation.load(Ordering::SeqCst) == generation {
                let state = match outcome {
                    EngineBuildOutcome::Succeeded => BuildState::Ready,
                    EngineBuildOutcome::Failed => BuildState::Failed,
                    // A co-located engine holds the shared build lock — this is
                    // NOT our compile failure, so must not surface the
                    // build-failed toast. Fall back to Idle; the peer's build
                    // advances the shared binary (→ `update_available` surfaces
                    // the Switch), and the self-heal driver retries next tick if
                    // the peer's build doesn't land.
                    EngineBuildOutcome::SkippedLocked => BuildState::Idle,
                };
                engine.set_build_state(state.clone());
                engine.emit_build_state_changed(&state).await;
            }
        });
        *self.build_task.lock().unwrap() = Some(handle);
    }

    /// Emit the transient `EngineBuildStateChanged` UI poke so a connected client
    /// learns of a background-rebuild transition over the live SSE stream instead
    /// of waiting on the throttled 4s version-status poll (which iOS suspends on a
    /// backgrounded PWA — the reason the "building" spinner badge never showed).
    /// The frontend handler re-runs the authoritative `checkEngineVersion` GET, so
    /// this is a pure nudge; `state` is informational. Dev-only: the sole caller
    /// (`trigger_background_rebuild`) already no-ops packaged.
    async fn emit_build_state_changed(&self, state: &BuildState) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::EngineBuildStateChanged {
                        state: state.as_wire().to_string(),
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Rebuild] EngineBuildStateChanged",
            )
            .await;
    }
}

/// Outcome of a background engine build. `SkippedLocked` is distinct from
/// `Failed`: a co-located engine holds the checkout-shared build lock, so THIS
/// engine deliberately didn't build — not a compile failure (must not surface the
/// "build failed" toast).
enum EngineBuildOutcome {
    Succeeded,
    Failed,
    SkippedLocked,
}

/// Try to acquire the checkout-shared engine-build lock — an advisory `flock`
/// (auto-released on drop / process death) under the shared `target/`. Returns
/// the held file guard, or `None` when another co-located engine already holds it.
/// Co-located workspaces share ONE checkout + ONE `target/`, so only one
/// `web-dev.sh --engine-build` may run at a time — concurrent cargo builds on the
/// same target OOM/corrupt (CLAUDE.md). Keep the returned guard alive for the
/// whole build; dropping it releases the lock.
fn try_acquire_engine_build_lock() -> Option<std::fs::File> {
    try_lock_file(&engine_build_lock_path()?)
}

/// Path of the checkout-shared engine-build lock, or `None` when the checkout
/// can't be resolved (`repo_root` unavailable). Split out so `run_engine_build`
/// can distinguish "no checkout to coordinate on" (proceed uncoordinated) from
/// "a peer holds the lock" (skip) — the two must not be conflated.
fn engine_build_lock_path() -> Option<std::path::PathBuf> {
    Some(
        crate::paths::repo_root()
            .ok()?
            .join("target")
            .join(".lucidos-engine-build.lock"),
    )
}

/// Open (creating parents) and non-blocking advisory-`flock` `lock_path`, returning
/// the held guard or `None` when another open file description already holds it.
/// Pure w.r.t. the path so the single-holder invariant is unit-testable without a
/// repo checkout. `flock` is per-open-file-description on Unix, so a second open of
/// the same path — even in this process, and across processes — conflicts.
fn try_lock_file(lock_path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // The file is a pure `flock` handle — nothing is ever written into it,
        // so never truncate: a peer may already hold this same path open.
        .truncate(false)
        .open(lock_path)
        .ok()?;
    fs2::FileExt::try_lock_exclusive(&file).ok()?;
    Some(file)
}

/// Cheap "is a co-located engine building right now?" probe: try the shared build
/// lock and immediately release it. Racy by nature (the real guard is the lock
/// held across [`run_engine_build`]), it just lets the self-heal driver avoid
/// churning a build attempt while a peer is already building. Treats an
/// un-acquirable lock (peer holds it, or git/lockfile unavailable) as "busy" —
/// the driver then skips this tick and retries, which is safe.
fn engine_build_in_progress_elsewhere() -> bool {
    match try_acquire_engine_build_lock() {
        Some(file) => {
            let _ = fs2::FileExt::unlock(&file);
            false
        }
        None => true,
    }
}

/// Whether the checkout-shared engine-build lock is **definitely** held right now
/// — a build IS running (this engine's own or a co-located peer's). Drives the
/// `shared_build_in_progress` version-status field.
///
/// Deliberately the INVERSE fail-mode of [`engine_build_in_progress_elsewhere`]:
/// that one treats a probe it can't run (unresolvable repo root, unopenable lock
/// file) as "busy" so the self-heal driver errs toward NOT stampeding a peer.
/// This one **fails open to `false`** — the wire field suppresses the manual
/// "Rebuild" escape hatch and shows the building spinner, so an *indeterminate*
/// probe must never read as "a build is running": that would hide the escape
/// hatch indefinitely while no build can advance the binary (the genuinely-stuck
/// case must still surface Rebuild). Returns `true` ONLY when the non-blocking
/// `flock` fails with `WouldBlock` (the lock is genuinely held); a free lock or
/// any probe failure (no checkout, unopenable / unlockable-for-other-reasons
/// file) returns `false`.
fn shared_engine_build_lock_held() -> bool {
    match engine_build_lock_path() {
        Some(path) => lock_held_at(&path),
        None => false, // no checkout to coordinate on → not a shared build
    }
}

/// Path-parameterized core of [`shared_engine_build_lock_held`] — split out so the
/// held/free/indeterminate detection is unit-testable without a repo checkout.
/// Returns `true` ONLY when a non-blocking `flock` fails with `WouldBlock` (the
/// lock is genuinely held by some open file description). A free lock, an
/// unopenable file, or any other lock error returns `false` (fail open — see the
/// caller's doc comment).
fn lock_held_at(path: &std::path::Path) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // Probe-only open: never truncate the lock file a peer may be holding.
        .truncate(false)
        .open(path)
    else {
        return false; // can't open the probe file → indeterminate → fail open
    };
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            // Acquired → held by no one; release immediately.
            let _ = fs2::FileExt::unlock(&file);
            false
        }
        // WouldBlock == another open file description holds it → genuinely busy.
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
        // Any other error is indeterminate → fail open so Rebuild stays available.
        Err(_) => false,
    }
}

/// The commit prefix of an engine build id, or `None` when there isn't one.
///
/// `build.rs` stamps `<short-sha>` for a clean tree and `<short-sha>-<diffhash>`
/// when engine source is dirty, falling back to `src-<hash>` with no git. Only the
/// commit is comparable across two binaries, so split at the first `-` and reject
/// the no-git / unstamped forms. Pure — the ancestry decision's parsing half.
fn build_id_commit(id: &str) -> Option<&str> {
    if id.is_empty() || id.starts_with("src") {
        return None;
    }
    let commit = id.split('-').next().unwrap_or("");
    (!commit.is_empty()).then_some(commit)
}

/// Is the on-disk binary a genuine upgrade, given its id, the running id, and the
/// ancestry answer (`Some(true)` = disk commit is a strict ancestor of the running
/// commit, i.e. provably older; `None` = git couldn't tell)?
///
/// Pure so the whole decision is unit-testable without a repo. The rule: an
/// upgrade is a readable, DIFFERENT id that is not provably older. Indeterminate
/// ancestry keeps the historical difference test — missing a real update (the user
/// stranded on an old engine with no Switch) is worse than the occasional
/// unresolvable id being offered.
fn disk_upgrade_verdict(
    disk_id: Option<&str>,
    running_id: &str,
    disk_is_strict_ancestor: Option<bool>,
) -> bool {
    match disk_id {
        Some(disk) => disk != running_id && disk_is_strict_ancestor != Some(true),
        None => false,
    }
}

/// Has self-heal proved that rebuilding cannot help for this HEAD? True when it
/// already triggered a rebuild in this process (`attempts > 0`) and that rebuild
/// finished SUCCESSFULLY (`Ready`) — the caller only reaches this after
/// establishing that no upgrade is on disk, so a successful build that produced
/// nothing newer is a wedged configuration, not a transient miss.
///
/// `Failed` deliberately does NOT count: a compile error is exactly what self-heal
/// exists to retry (`docs/plans/2026-07-03-engine-version-switch-selfheal.md`), and
/// it keeps the per-HEAD attempt cap. Pure.
fn self_heal_is_wedged(attempts: u32, build_state: &BuildState) -> bool {
    attempts > 0 && *build_state == BuildState::Ready
}

/// Is `ancestor` a STRICT ancestor of `descendant` (`git merge-base
/// --is-ancestor`, plus the two being different commits)? `None` when git can't
/// answer — no repo, an unknown commit (shallow clone, force-pushed away), or a
/// command failure — so callers can distinguish "provably older" from "don't know".
///
/// The gateway carries a hand-synced copy of this (and of [`build_id_commit`] /
/// [`disk_upgrade_verdict`]) in `crates/lucidos-gateway/src/build_id.rs` — ADR
/// 0014 §1 keeps that crate free of any dependency on the engine. **Keep the two
/// in step**, as with `path_is_in_cc_worktree`'s three copies.
///
/// Takes `root` rather than resolving it internally so the tests can drive it
/// against a throwaway repo.
///
/// STRICT is enforced here, not by git: `git merge-base --is-ancestor X X` exits 0
/// (a commit is its own ancestor), so the same commit must be screened out first.
/// The screen is prefix-aware because the two sides arrive at different lengths —
/// build ids carry git's SHORT sha while `current_head_sha` returns the full one —
/// and a plain `==` would let `aa7075ee2` vs `aa7075ee2e8e…` through as "older".
async fn commit_is_strict_ancestor(
    root: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    if ancestor.starts_with(descendant) || descendant.starts_with(ancestor) {
        return Some(false); // same commit (possibly abbreviated) — not OLDER
    }
    let out = tokio::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .output()
        .await
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),  // is an ancestor
        Some(1) => Some(false), // is not an ancestor
        // Any other code is an error (bad object, not a repo) — not a verdict.
        _ => None,
    }
}

/// The checkout's current HEAD sha (`git rev-parse HEAD`), or `None` when git is
/// unavailable. Keys the self-heal attempt counter so a broken `main` gives up
/// per-HEAD and retries once new work lands.
async fn current_head_sha() -> Option<String> {
    let root = crate::paths::repo_root().ok()?;
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Run `web-dev.sh --engine-build -w <ws>` to rebuild the engine binary on disk,
/// appending output to the workspace engine log. `kill_on_drop` so a coalescing
/// abort kills the cargo process instead of orphaning it. Holds the
/// checkout-shared build lock for the whole build so co-located engines can't run
/// concurrent cargo builds on the same `target/`; returns `SkippedLocked` when a
/// peer already holds it.
async fn run_engine_build(workspace: &std::path::Path) -> EngineBuildOutcome {
    // Elect a single builder across co-located engines (see the lock helper).
    // Held (via `_build_lock`) until this function returns. When the checkout
    // can't be resolved there's no shared `target/` to coordinate on — proceed
    // uncoordinated (fail-open) rather than never building; only a genuinely
    // held lock means a peer is building (→ SkippedLocked).
    let _build_lock = match engine_build_lock_path() {
        Some(path) => match try_lock_file(&path) {
            Some(guard) => Some(guard),
            None => {
                crate::log!(
                    "[Rebuild] engine build already in progress on this checkout — skipping (a co-located workspace is building)"
                );
                return EngineBuildOutcome::SkippedLocked;
            }
        },
        None => None,
    };
    let script = match crate::paths::script("web-dev.sh") {
        Ok(s) => s,
        Err(e) => {
            crate::log!("[Rebuild] cannot locate web-dev.sh: {e}");
            return EngineBuildOutcome::Failed;
        }
    };
    let ws = workspace.to_string_lossy().to_string();
    let log_path = workspace.join(".lucidos/engine.log");
    let mut cmd = tokio::process::Command::new(&script);
    cmd.args(["-w", &ws, "--engine-build"]).kill_on_drop(true);
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        match f.try_clone() {
            Ok(f2) => {
                cmd.stdout(f).stderr(f2);
            }
            Err(_) => {
                cmd.stdout(f);
            }
        }
    }
    match cmd.status().await {
        Ok(status) if status.success() => EngineBuildOutcome::Succeeded,
        Ok(status) => {
            crate::log!("[Rebuild] engine build exited {status}");
            EngineBuildOutcome::Failed
        }
        Err(e) => {
            crate::log!("[Rebuild] failed to spawn engine build: {e}");
            EngineBuildOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_id_commit, commit_is_strict_ancestor, disk_upgrade_verdict, lock_held_at,
        self_heal_is_wedged, try_lock_file, BuildState,
    };

    /// Poll `cond` until it holds, up to ~2 s.
    ///
    /// Releasing an `flock` is only *eventually* observable in a process that
    /// spawns subprocesses. The lock belongs to the open file description, and
    /// `fork` hands the child a reference to that same description — so until
    /// the child reaches `exec` (where `O_CLOEXEC` drops it) the lock stays
    /// alive even though the owner closed its own fd. The suite forks
    /// constantly (`engine::tools::bash`, `runtime::python`, `core::shell`), and
    /// under full-suite load a forked child can be descheduled between fork and
    /// exec for long enough to be caught by a probe fired microseconds after a
    /// drop. That made both lock tests flake roughly one full run in three.
    ///
    /// The retry is not a weakened assertion: nothing about the release path is
    /// instantaneous by contract, and production agrees — the real consumer
    /// (`engine_build_in_progress_elsewhere`) treats an un-acquirable lock as
    /// "a peer is building, skip this tick and retry". The *held* direction is
    /// still asserted immediately; only the released direction is given time.
    fn eventually(cond: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// The load-bearing single-builder invariant: while one holder has the build
    /// lock, no other acquire succeeds; the lock releases on drop. `flock` is
    /// per-open-file-description on Unix, so this same-process check faithfully
    /// mirrors the cross-engine (cross-process) case that serializes rebuilds.
    #[test]
    fn build_lock_admits_a_single_holder_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("lucidos-buildlock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".lucidos-engine-build.lock");

        let held = eventually_acquire(&path);
        assert!(
            try_lock_file(&path).is_none(),
            "a second acquire must fail while the lock is held (single builder)"
        );
        drop(held);
        assert!(
            eventually(|| try_lock_file(&path).is_some()),
            "the lock must become acquirable again after release"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Take the lock, tolerating a transient hold by a concurrently-forked
    /// child that hasn't reached `exec` yet. See [`eventually`].
    fn eventually_acquire(path: &std::path::Path) -> std::fs::File {
        for _ in 0..200 {
            if let Some(file) = try_lock_file(path) {
                return file;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("could not acquire {} within 2 s", path.display());
    }

    /// The `shared_build_in_progress` fail-OPEN contract: the held-detection probe
    /// reports `true` ONLY while the lock is genuinely held, and `false` on a free
    /// lock — so an idle/free (or indeterminate) probe can never hide the manual
    /// "Rebuild" escape hatch behind a phantom spinner.
    #[test]
    fn lock_held_at_reports_held_only_while_genuinely_locked() {
        let dir = std::env::temp_dir().join(format!("lucidos-heldprobe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".lucidos-engine-build.lock");

        // Free lock → not held (fail open: escape hatch stays available).
        assert!(
            eventually(|| !lock_held_at(&path)),
            "an unlocked path must read as NOT held"
        );
        // Held by another open file description → genuinely busy (WouldBlock).
        // This direction is immediate — nothing can make a held lock look free.
        let held = eventually_acquire(&path);
        assert!(
            lock_held_at(&path),
            "a held lock must read as held (flock WouldBlock)"
        );
        // Released → free again (see `eventually`: a forked child can hold the
        // inherited description for a beat after we close ours).
        drop(held);
        assert!(
            eventually(|| !lock_held_at(&path)),
            "a released lock must read as NOT held again"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_id_commit_takes_the_sha_and_rejects_the_no_git_forms() {
        assert_eq!(build_id_commit("aa7075ee2"), Some("aa7075ee2"));
        // Dirty tree: `<sha>-<diffhash>` — only the sha is comparable.
        assert_eq!(
            build_id_commit("aa7075ee2-0badc0ffee123456"),
            Some("aa7075ee2")
        );
        // No git (shipped build) / unstamped → nothing to compare.
        assert_eq!(build_id_commit("src-0123456789abcdef"), None);
        assert_eq!(build_id_commit(""), None);
        assert_eq!(build_id_commit("-abc"), None);
    }

    /// The 2026-07-26 loop in one assertion set: a DIFFERENT on-disk binary is an
    /// update only when it isn't provably OLDER. Everything indeterminate keeps the
    /// historical difference test, so this can only remove a false positive.
    #[test]
    fn disk_upgrade_verdict_offers_only_a_step_forward() {
        // The live bug: disk `71c8d39b1` is an ancestor of running `aa7075ee2`.
        assert!(
            !disk_upgrade_verdict(Some("71c8d39b1"), "aa7075ee2", Some(true)),
            "an older on-disk binary is a DOWNGRADE and must not be offered"
        );
        // The normal case: a newer binary was built (not an ancestor).
        assert!(disk_upgrade_verdict(
            Some("bb1122334"),
            "aa7075ee2",
            Some(false)
        ));
        // Same id → nothing to switch onto, whatever git says.
        assert!(!disk_upgrade_verdict(
            Some("aa7075ee2"),
            "aa7075ee2",
            Some(false)
        ));
        // Unreadable disk id (packaged / mid-rewrite) → no update.
        assert!(!disk_upgrade_verdict(None, "aa7075ee2", None));
        // Indeterminate ancestry (unrelated commits, no repo, unknown object) →
        // fall back to "different is an update": never MISS a real one.
        assert!(disk_upgrade_verdict(Some("cc9988776"), "aa7075ee2", None));
        // Same commit, different uncommitted diff → a real rebuild, so an update.
        assert!(disk_upgrade_verdict(
            Some("aa7075ee2-0badc0ffee123456"),
            "aa7075ee2",
            Some(false)
        ));
    }

    /// Retrying a rebuild is only futile once one has SUCCEEDED without advancing
    /// the binary. A failed build keeps its retry budget — that is the case
    /// self-heal exists for.
    #[test]
    fn self_heal_gives_up_only_after_a_successful_build_changed_nothing() {
        assert!(self_heal_is_wedged(1, &BuildState::Ready));
        assert!(self_heal_is_wedged(3, &BuildState::Ready));
        // Nothing tried yet this process (e.g. a fresh start, or a peer built) →
        // the Ready state isn't ours to conclude from.
        assert!(!self_heal_is_wedged(0, &BuildState::Ready));
        // A compile error is retryable, not wedged.
        assert!(!self_heal_is_wedged(1, &BuildState::Failed));
        // Idle: no build outcome to judge (the caller already excluded Building).
        assert!(!self_heal_is_wedged(1, &BuildState::Idle));
        // Deliberately still true at the cap: nothing clears `Ready`, so the
        // predicate stays hot on every later tick. That is WHY the caller checks
        // the spent budget FIRST — otherwise the give-up line would re-log every
        // 10s instead of once. Reordering those two checks reintroduces a log storm.
        assert!(self_heal_is_wedged(
            super::SELF_HEAL_MAX_ATTEMPTS_PER_HEAD,
            &BuildState::Ready
        ));
    }

    /// The real git probe behind the direction check, against a throwaway repo:
    /// ancestor → `Some(true)`, the reverse → `Some(false)`, an unknown object →
    /// `None` (so an unresolvable id can't be mistaken for "provably older").
    #[tokio::test]
    async fn commit_is_strict_ancestor_reads_history_direction() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-ancestry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs in the test environment")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);
        let first = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "second"]);
        let second = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();

        assert_eq!(
            commit_is_strict_ancestor(&dir, &first, &second).await,
            Some(true),
            "the earlier commit IS a strict ancestor of the later one"
        );
        assert_eq!(
            commit_is_strict_ancestor(&dir, &second, &first).await,
            Some(false),
            "the later commit is NOT an ancestor of the earlier one"
        );
        assert_eq!(
            commit_is_strict_ancestor(&dir, &first, &first).await,
            Some(false),
            "a commit is not a STRICT ancestor of itself"
        );
        // The abbreviation trap: build ids carry the SHORT sha while HEAD arrives
        // full, and `git merge-base --is-ancestor X X` exits 0 — so without the
        // prefix-aware screen the same commit would read as "provably older".
        assert_eq!(
            commit_is_strict_ancestor(&dir, &second[..9], &second).await,
            Some(false),
            "the same commit abbreviated is still not a strict ancestor of itself"
        );
        assert_eq!(
            commit_is_strict_ancestor(&dir, "0000000000000000000000000000000000000000", &second)
                .await,
            None,
            "an unknown object is indeterminate, never 'provably older'"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
