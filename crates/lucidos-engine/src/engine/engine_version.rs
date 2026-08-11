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

/// How long a [`PendingCommits`] read is reused before `git log` is re-run. Same
/// rationale as [`SOURCE_BEHIND_TTL`] (bound the git forks to one per interval
/// no matter how many clients poll), and the same short window so a freshly
/// merged commit shows up in the toast within a few seconds.
const PENDING_COMMITS_TTL: Duration = Duration::from_secs(3);

/// How many commit descriptions each [`CommitGroup`] carries. PER GROUP, not
/// across the range: a flat cap let forty doc commits crowd out the one `feat`
/// the user actually wanted to read about. The rest are counted
/// ([`CommitGroup::total`]) rather than named, because the toast is a glance,
/// not a changelog.
const PENDING_COMMIT_DESCRIPTION_CAP: usize = 5;

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
    ///
    /// Carries the moment it started so the status toast can count up while it
    /// runs. In the variant rather than beside it: "building, but nobody knows
    /// since when" is not a state this engine should be able to represent, and a
    /// separate `Option<Instant>` field would drift the moment a future caller
    /// set the state without stamping it. In-memory only, like the rest of the
    /// build orchestration: a build dies with the engine, and a restart starts
    /// from `Idle`.
    Building { started_at: Instant },
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
            BuildState::Building { .. } => "building",
            BuildState::Ready => "ready",
            BuildState::Failed => "failed",
        }
    }

    /// A fresh `Building` stamped with now. The one constructor, so every build
    /// start records its own clock rather than inheriting a caller's.
    pub fn building_now() -> Self {
        BuildState::Building {
            started_at: Instant::now(),
        }
    }

    /// How long this build has been running, or `None` when it isn't one.
    pub fn elapsed(&self) -> Option<Duration> {
        match self {
            BuildState::Building { started_at } => Some(started_at.elapsed()),
            _ => None,
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

/// What a commit in the range IS, so the toast can describe the build rather
/// than reciting its log. Derived from the conventional-commit type; the
/// frontend owns the wording each kind renders as.
///
/// [`CommitGroupKind::Other`] is the honest home of a subject with no type we
/// recognize (a hand commit, a revert): guessing would be worse than saying so,
/// and dropping it would lose a commit that might be the interesting one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitGroupKind {
    /// `feat`.
    New,
    /// `fix`.
    Fixed,
    /// `perf`, `refactor`, `style`. In this repo `style` is UI design work, not
    /// whitespace, so it belongs with the user-visible improvements.
    Improved,
    /// No recognized conventional-commit type.
    Other,
    /// `docs`, `chore`, `test`, `ci`, `build`, `harden`. Counted, never listed:
    /// it is the bulk of this repo's log and none of it answers "what am I
    /// getting". [`CommitGroup::descriptions`] is always empty for this kind.
    Housekeeping,
}

impl CommitGroupKind {
    /// How many kinds there are, and the width of the tallies
    /// [`parse_pending_commits`] counts into.
    const COUNT: usize = 5;

    /// This kind's index in those tallies, and in [`COMMIT_GROUP_ORDER`].
    ///
    /// An exhaustive `match` rather than a lookup in the order array: adding a
    /// variant then has to answer this, at compile time, instead of panicking
    /// on a `position(...).unwrap()` inside a version-status poll.
    fn slot(self) -> usize {
        match self {
            CommitGroupKind::New => 0,
            CommitGroupKind::Fixed => 1,
            CommitGroupKind::Improved => 2,
            CommitGroupKind::Other => 3,
            CommitGroupKind::Housekeeping => 4,
        }
    }
}

/// One bucket of the pending range, with its own count so a capped list can say
/// how much it is not showing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitGroup {
    pub kind: CommitGroupKind,
    /// Every commit in this group, capped or not.
    pub total: usize,
    /// Newest-first, at most [`PENDING_COMMIT_DESCRIPTION_CAP`], and empty for
    /// [`CommitGroupKind::Housekeeping`]. Each line is the commit subject with
    /// its conventional-commit TYPE stripped and its scope kept as a lead-in
    /// (`fix(ui): the trash is sized by its ink` becomes `ui: the trash is
    /// sized by its ink`): the type is already carried by the group, and the
    /// scope names the area. Nothing is re-capitalized, since these subjects
    /// are authored lowercase and a mechanical uppercase turns `ui` into `Ui`.
    pub descriptions: Vec<String>,
}

/// The commits a *Switch to new version* would bring: every non-merge commit
/// between the running engine's commit and HEAD, grouped by what it is.
/// Surfaced on `version_status` so the status toast behind the spinning brand
/// badge can say what is being built instead of repeating its own tooltip.
///
/// **Merges are excluded** ([`LucidosEngine::read_pending_commits`] passes
/// `--no-merges`). An Apply lands as a merge whose subject is the branch name,
/// which describes no work, and everything it merged is already in this range
/// under its own subject. Listing them is pure duplication, and on a range
/// spanning several Applies they crowded out every real commit.
///
/// "commits", not "changes": a *change* is the coding-agent change the user
/// Applies (see `system-knowhow/glossary.md`), and not every commit in this
/// range is one (a hand commit, a revert), so naming them changes would be
/// wrong in both directions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingCommits {
    /// Every non-merge commit in the range. Computed from `groups` by
    /// [`Self::from_groups`], the only constructor, so the count the toast
    /// headlines can never drift from the list under it.
    pub total: usize,
    /// Non-empty groups, in the order the toast lists them.
    pub groups: Vec<CommitGroup>,
}

impl PendingCommits {
    /// The one constructor: derives `total` so it cannot disagree with the
    /// groups it summarizes.
    fn from_groups(groups: Vec<CommitGroup>) -> Self {
        PendingCommits {
            total: groups.iter().map(|g| g.total).sum(),
            groups,
        }
    }
}

/// Throttled cache of [`LucidosEngine::pending_commits`], mirroring
/// [`SourceBehindCache`]: version-status is polled ~every 4s per connected
/// client, and this forks `git log`. `checked_at == None` means "never
/// computed"; `commits == None` is a cached UNKNOWN (git could not answer),
/// which is deliberately cached too so a wedged git isn't re-forked per poll.
#[derive(Default)]
pub struct PendingCommitsCache {
    checked_at: Option<Instant>,
    commits: Option<PendingCommits>,
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
    /// How long THIS engine's own background rebuild has been running, in ms, or
    /// absent when no build of ours is in flight (including a co-located peer's
    /// build, whose clock we don't have, and packaged).
    ///
    /// ELAPSED rather than a start timestamp on purpose: the client renders a
    /// counter from it, and differencing an engine wall-clock against the
    /// browser's would show a wrong (or negative) duration whenever the two
    /// clocks disagree. The client anchors this to its own `Date.now()` at
    /// receipt and counts up locally, so skew cannot reach the number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_elapsed_ms: Option<u64>,
    /// The commits between the running engine's commit and HEAD, or absent when
    /// git could not say (see [`PendingCommits`]). Only read when a build is in
    /// flight or the source is behind HEAD, so an idle workspace forks no git.
    ///
    /// Absent is UNKNOWN, never "none pending": a `Some` with `total: 0` is the
    /// only way to say there is nothing to bring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_commits: Option<PendingCommits>,
}

/// The stash rule, extracted from [`LucidosEngine::stash_restart_actor`] so it
/// is testable without an engine: **first writer wins, and `None` is never
/// stored**. Returns whether `actor` was taken.
///
/// Two writers race for one slot on a single restart. The in-workspace *Switch*
/// (`/api/v1/restart`) stashes the device that clicked it and only THEN asks the
/// gateway to respawn the stack; the gateway now notifies the engine back before
/// it signals (`/api/v1/internal/restart-intent`), so the same restart arrives
/// twice. Keeping the first is what makes the second harmless: the two carry the
/// same device in the normal case, and where they could disagree the one holding
/// the actual HTTP context of the click is the honest answer.
///
/// A `None` is not an answer, it is the absence of one, so it must never erase a
/// stashed actor. That matters for the notify path in particular, whose caller
/// skips it entirely when it has no device to name.
fn stash_first_restart_actor(
    slot: &mut Option<crate::engine::thread_events::MessageOrigin>,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> bool {
    match (slot.is_some(), actor) {
        (false, Some(actor)) => {
            *slot = Some(actor);
            true
        }
        _ => false,
    }
}

/// Move the restart stash into the teardown slot: spend it exactly once, and
/// leave a COPY where every later emit in this teardown can read it.
///
/// The two halves are the whole point, and each answers a different failure:
///
/// * Spending it (`take`) is what `take_restart_actor` documents. A stash
///   belongs to the restart that made it, so a later teardown nobody asked for
///   cannot inherit a device actor and auto-resume work on the strength of it.
/// * KEEPING a copy is what stops the answer depending on timing. Before
///   2026-08-07 the take was all there was, so the pre-emit consumed the only
///   copy and the two emits that run after it hardcoded a system actor. One
///   *Switch to new version* then produced two verdicts, decided by whether a
///   thread became in-flight before or after the pre-emit's snapshot.
///
/// A free function over the two slots, like [`stash_first_restart_actor`] above,
/// so the rule is testable without standing up an engine.
fn open_teardown(
    restart_slot: &mut Option<crate::engine::thread_events::MessageOrigin>,
    teardown_slot: &mut Option<crate::engine::thread_events::MessageOrigin>,
) -> Option<crate::engine::thread_events::MessageOrigin> {
    let actor = restart_slot.take();
    *teardown_slot = actor.clone();
    actor
}

impl LucidosEngine {
    /// Stash the device actor at restart-request time so the graceful-shutdown
    /// boundary emit (which runs in the signal handler, with no HTTP context) can
    /// attribute the restart to "You" and recovery can auto-resume in-flight
    /// threads. Returns whether this call is the one that stashed.
    ///
    /// Two callers, one per way a user can ask for this engine to go down:
    /// the in-workspace *Switch to new version* handler (`/api/v1/restart`) and
    /// the gateway's restart-intent notify (`/api/v1/internal/restart-intent`),
    /// which fires just before the picker's Restart / Stop signals the process.
    /// Both go through the first-writer-wins rule in
    /// [`stash_first_restart_actor`]; see there for why.
    pub fn stash_restart_actor(
        &self,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> bool {
        stash_first_restart_actor(&mut self.restart_actor.lock().unwrap(), actor)
    }

    /// Take (and clear) the stashed restart actor. Cleared so a later teardown
    /// nobody asked for (stop.sh, an external SIGUSR1, a crash-respawn) doesn't
    /// reuse a stale device actor: it then falls back to System attribution and
    /// a manual Continue.
    ///
    /// Two callers, and the second is why this is not simply "read it at
    /// teardown": [`begin_teardown`](Self::begin_teardown), and `restart_engine`
    /// undoing its OWN stash when the respawn it asked for never happened. Under
    /// first-writer-wins an abandoned stash is not merely stale, it is a block on
    /// the next restart's actor.
    pub fn take_restart_actor(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.restart_actor.lock().unwrap().take()
    }

    /// Open the teardown: mark the engine shutting down, decide the teardown's
    /// actor ONCE, and return it for the boundary pre-emit.
    ///
    /// Called from `main.rs::shutdown_signal` and nowhere else. Everything after
    /// it reads the decision back through
    /// [`teardown_actor`](Self::teardown_actor) rather than re-deriving it, which
    /// is the whole point: **who tore the engine down is a property of the
    /// teardown, not of when a thread became in-flight.**
    ///
    /// It was not, until 2026-08-07. `main.rs` called `take_restart_actor()` and
    /// handed the only copy to `abort_in_flight_for_restart`, so the two emits
    /// that run after it hardcoded a system actor: `shutdown_active_threads` for
    /// a chat thread that reached `processing_thread_ids()` after the pre-emit's
    /// snapshot, and `emit_stop_terminal`'s abort arm for a coding-agent session
    /// registered after `shutdown_agent_sessions` took its flag pass. A device
    /// actor is half the switch fingerprint
    /// (`agent_recovery::SWITCH_TEARDOWN_ABORT_SQL`), so those threads lost the
    /// `paused` verdict, the withheld Continue button, and the auto-resume, all
    /// three from a timing difference of a second and a half. See
    /// `docs/plans/2026-08-07-teardown-actor-is-one-value-for-the-whole-teardown.md`.
    ///
    /// Still spends the stash rather than peeking at it, so the invariant
    /// `take_restart_actor` documents holds unchanged: a stash is consumed by the
    /// teardown it belongs to. See [`open_teardown`] for both halves of that.
    pub fn begin_teardown(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.mark_shutting_down();
        open_teardown(
            &mut self.restart_actor.lock().unwrap(),
            &mut self.teardown_actor.lock().unwrap(),
        )
    }

    /// The actor of the teardown under way, for an `EngineShutdown` abort that
    /// runs after the boundary pre-emit. `None` outside a teardown, and `None`
    /// for one nobody requested (bare `stop.sh`, an external SIGUSR1, ctrl-c);
    /// callers fall back to `MessageOrigin::system()` for both, which is the
    /// honest answer in either case.
    pub(crate) fn teardown_actor(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.teardown_actor.lock().unwrap().clone()
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
    ///
    /// Returns the thread ids whose resume this boot has taken responsibility
    /// for, which `settle_unresumed_switch_threads` needs to EXCLUDE. The
    /// dispatcher has not emitted `ContinuationStarted` yet when the floor runs,
    /// and `ContinuationRequested` is deliberately absent from
    /// `THREAD_START_EVENTS_SQL`, so a query-only exclusion would re-abort a
    /// thread that is resuming correctly. The skip branch below still counts as
    /// actuated: the dispatcher's startup orphan re-dispatch owns that resume.
    pub async fn resume_pending_switches(&self) -> Vec<uuid::Uuid> {
        let ids = std::mem::take(&mut *self.pending_switch_resumes.lock().unwrap());
        let mut actuated = Vec::with_capacity(ids.len());
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
                actuated.push(thread_id);
                continue;
            }
            // Only a request that actually PERSISTED counts as actuated. An emit
            // that errored or was rejected leaves nothing for the dispatcher to
            // act on, so the thread must stay visible to
            // `settle_unresumed_switch_threads`, which withdraws the promise and
            // gives the user their Continue button back. Pushing it regardless
            // would suppress that button forever on the one path where the
            // resume provably did not happen.
            let requested = crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON,
                None,
                "[Recovery] ContinuationRequested (auto-resume after switch)",
            )
            .await;
            if requested {
                actuated.push(thread_id);
            }
        }
        actuated
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
        let build_state = self.build_state();
        // Only look up what a switch would bring when there is something to
        // bring: a build in flight (ours or a co-located peer's) or a source
        // behind HEAD. At rest this is the difference between one `git log` per
        // poll per client and none at all.
        let pending_commits =
            if build_state.elapsed().is_some() || shared_build_in_progress || source_behind_head {
                self.pending_commits().await
            } else {
                None
            };
        VersionStatus {
            build_id: crate::ENGINE_BUILD_ID.to_string(),
            update_available,
            disk_build_id,
            packaged: crate::runtime::is_packaged(),
            build_state: build_state.as_wire(),
            source_behind_head,
            shared_build_in_progress,
            build_elapsed_ms: build_state
                .elapsed()
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            pending_commits,
        }
    }

    /// The non-merge commits between the running engine's commit and HEAD,
    /// grouped by what they are, or `None` when git could not answer. TTL-cached
    /// ([`PENDING_COMMITS_TTL`]) so the `git log` runs at most once per interval
    /// across every polling client.
    async fn pending_commits(&self) -> Option<PendingCommits> {
        {
            let mut cache = self.pending_commits_cache.lock().unwrap();
            if cache
                .checked_at
                .is_some_and(|at| at.elapsed() < PENDING_COMMITS_TTL)
            {
                return cache.commits.clone();
            }
            // Claim the refresh BEFORE dropping the lock and forking git, so a
            // client polling during the read reuses the previous answer instead
            // of starting a second `git log`. Stamping only on completion would
            // leave the TTL bounding nothing under concurrency: every client
            // that arrives while a read is in flight reads the same expired
            // timestamp and forks its own. The cost is that those callers see
            // one generation behind for the length of a single `git log`, which
            // for a 3s display value is nothing. (A read that never completes,
            // e.g. its request was cancelled mid-await, just means the previous
            // answer is served for one more TTL.)
            cache.checked_at = Some(Instant::now());
        }
        let commits = self.read_pending_commits().await;
        self.pending_commits_cache
            .lock()
            .unwrap()
            .commits
            .clone_from(&commits);
        commits
    }

    /// Uncached read behind [`Self::pending_commits`]. `None` at every step that
    /// cannot produce a trustworthy answer: packaged (nothing rebuilds from
    /// source), a `src-…` build id with no commit to range from, no resolvable
    /// checkout, or a `git log` that failed or timed out.
    async fn read_pending_commits(&self) -> Option<PendingCommits> {
        if crate::runtime::is_packaged() {
            return None;
        }
        let running = build_id_commit(crate::ENGINE_BUILD_ID)?;
        let root = crate::paths::repo_root().ok()?;
        let range = format!("{running}..HEAD");
        // `--no-merges`: an Apply lands as a merge whose subject is the branch
        // name, and everything it merged is in this same range under its own
        // subject. See [`PendingCommits`].
        classify_pending_commits(
            crate::engine::git_ops::git_cmd(&["log", "--no-merges", "--format=%s", &range], &root)
                .await,
        )
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
        // `matches!` rather than `==`: `Building` carries its start instant, so
        // two in-flight builds are never equal to each other.
        if matches!(self.build_state(), BuildState::Building { .. }) {
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
    /// "New version available". A second call **coalesces**: the in-flight build is
    /// aborted (killing its whole build process group) and a fresh build starts, so
    /// the build always reflects the latest merged source. No-op in packaged, which
    /// never rebuilds from source (its "new version" is the release updater).
    pub fn trigger_background_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let generation = self.build_generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Coalesce: abort any running build task. Its child dies with it
        // (`kill_on_drop` plus the process-group kill in `run_engine_build`).
        let superseded = self.build_task.lock().unwrap().take();
        if let Some(old) = &superseded {
            old.abort();
        }
        // Stamped once here, so the state the SSE poke reports and the state the
        // elapsed counter reads are the same start moment.
        let building = BuildState::building_now();
        self.set_build_state(building.clone());
        let engine = self.clone();
        let workspace = self.workspace_path().to_path_buf();
        let handle = tokio::spawn(async move {
            // Push `building` over SSE so a connected client shows the spinner
            // immediately — the 4s version-status poll alone misses this transient
            // window (iOS suspends the timer on a backgrounded PWA), so the spinner
            // badge never showed. See engine_version::emit_build_state_changed.
            engine.emit_build_state_changed(&building).await;
            // HAND THE BUILD LOCK OVER, don't race it. `abort()` only REQUESTS
            // cancellation: the superseded task drops its `flock` guard when it
            // next reaches an await point, which is strictly after `abort()`
            // returns. Probing the checkout-shared lock without waiting for that
            // reads the dying build's own guard as "a peer is building", returns
            // `SkippedLocked` → `BuildState::Idle`, and leaves NO build running at
            // all, so the user gets the manual "New engine version pending /
            // Rebuild" toast until the ~10s self-heal tick starts the build for
            // real. Three back-to-back Applies did exactly that on 2026-08-05.
            //
            // Awaiting the handle resolves only once the task has stopped and its
            // locals (the lock guard among them) are dropped. Bounded without a
            // timeout because every await in that task is cancel-safe and already
            // unblocked at cancellation: the SSE emit, the lock wait's sleep, and
            // `Child::wait`. An already-finished build resolves immediately.
            if let Some(old) = superseded {
                let _ = old.await;
            }
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

/// How long [`run_engine_build`] waits for the checkout-shared build lock before
/// concluding a co-located engine owns it.
///
/// A single instantaneous try is not a verdict, it is a sample. Two things make
/// a FREE lock read as held for a few milliseconds: a build this one just
/// superseded may still be dropping its guard, and any concurrently-forked
/// subprocess transiently inherits the open file description until it reaches
/// `exec` (the same effect the lock tests' `eventually` helper exists for). Both
/// resolve in milliseconds, while a genuine peer build runs for a minute or more,
/// so a short wait separates the two without slowing either down.
const BUILD_LOCK_WAIT: Duration = Duration::from_secs(3);

/// Poll interval while waiting for [`BUILD_LOCK_WAIT`]. `flock` has no async
/// notification, so the wait is a poll; fine at this granularity for something
/// that either resolves in milliseconds or not at all.
const BUILD_LOCK_POLL: Duration = Duration::from_millis(50);

/// [`try_lock_file`] with a bounded retry, for the one caller that must not
/// mistake a millisecond of contention for a peer build. Returns the held guard,
/// or `None` when `wait` elapsed with the lock still held. Path- and
/// duration-parameterized so the timing is unit-testable without a checkout.
async fn acquire_engine_build_lock_waiting(
    lock_path: &std::path::Path,
    wait: Duration,
) -> Option<std::fs::File> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(guard) = try_lock_file(lock_path) {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(BUILD_LOCK_POLL).await;
    }
}

/// SIGKILLs the build's process group if the build future is DROPPED, i.e. when
/// a coalescing Apply aborts it.
///
/// `kill_on_drop(true)` reaches only the direct child, which is `web-dev.sh`. The
/// `cargo` it spawned is a grandchild and survives, so before this guard a rapid
/// series of Applies left superseded builds compiling against the shared
/// `target/` alongside the live one, each one slowing the build the user is
/// actually waiting on (three overlapping runs finishing 1m30s apart were visible
/// in the dev engine log on 2026-08-05).
///
/// Disarmed the moment the child is reaped: after `wait` the pid, and with it the
/// group id, can be recycled, and signalling a recycled group would hit unrelated
/// processes (see `spawn_env::signal_child_process_group`).
///
/// SIGKILL is untrappable, so a kill landing inside the launch-binary publish
/// leaves its `*.tmp.<pid>` behind. That is disk only, never a corrupt binary:
/// the publish copies and signs a temp and reaches the launch path solely
/// through `mv -f`. `prune_dead_launch_temps` (`scripts/lib/workspace.sh`)
/// collects the residue on the next publish.
struct BuildProcessGroupGuard(Option<u32>);

impl BuildProcessGroupGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for BuildProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            crate::runtime::spawn_env::kill_child_process_group_now(pid);
        }
    }
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

/// Classify a `git log --no-merges --format=%s <range>` run into the grouped
/// commit list the status toast shows, keeping "git could not answer" apart
/// from "git answered none".
///
/// `Err` is a spawn failure or the [`GIT_TIMEOUT`](crate::engine::git_ops) ceiling,
/// and a non-zero exit means git refused the range (an unknown commit, no
/// repository); neither says anything about what is pending, so both are `None`.
/// Only a successful run yields a verdict, and an empty one is a real
/// `total: 0` with no groups. Reading an unanswerable probe as "nothing is
/// coming" is the failure this split exists to prevent
/// (`.claude/rules/rust.md`).
fn classify_pending_commits(
    result: Result<std::process::Output, String>,
) -> Option<PendingCommits> {
    match result {
        Ok(out) if out.status.success() => {
            Some(parse_pending_commits(&String::from_utf8_lossy(&out.stdout)))
        }
        Ok(_) | Err(_) => None,
    }
}

/// The order the toast lists the groups in. What the user is being GIVEN leads;
/// what was merely tidied trails.
const COMMIT_GROUP_ORDER: [CommitGroupKind; CommitGroupKind::COUNT] = [
    CommitGroupKind::New,
    CommitGroupKind::Fixed,
    CommitGroupKind::Improved,
    CommitGroupKind::Other,
    CommitGroupKind::Housekeeping,
];

/// Split a conventional-commit subject into its type, scope and description, or
/// `None` when it isn't one. Pure and deliberately strict: the type is `[a-z]+`
/// optionally followed by a `(scope)` and/or a breaking-change `!`, then `": "`.
/// A looser rule would read the colon in an ordinary English subject as a type
/// tag and eat the words before it, so "Note to self: don't" keeps its lead-in
/// and lands in [`CommitGroupKind::Other`] whole.
fn split_conventional(subject: &str) -> Option<(&str, &str, &str)> {
    let (head, rest) = subject.split_once(": ")?;
    let head = head.strip_suffix('!').unwrap_or(head);
    let (kind, scope) = match head.split_once('(') {
        Some((kind, scope)) => (kind, scope.strip_suffix(')')?),
        None => (head, ""),
    };
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some((kind, scope, rest))
}

/// Classify one commit subject into its group and the line the toast shows for
/// it. Pure, so the whole taxonomy is testable without a repository.
///
/// An unrecognized type keeps its WHOLE subject: we only strip a tag we
/// understood, since deleting a token we could not classify loses information
/// for nothing.
fn classify_commit_subject(subject: &str) -> (CommitGroupKind, String) {
    let Some((kind, scope, description)) = split_conventional(subject) else {
        return (CommitGroupKind::Other, subject.to_string());
    };
    let group = match kind {
        "feat" => CommitGroupKind::New,
        "fix" => CommitGroupKind::Fixed,
        "perf" | "refactor" | "style" => CommitGroupKind::Improved,
        "docs" | "chore" | "test" | "ci" | "build" | "harden" => CommitGroupKind::Housekeeping,
        _ => return (CommitGroupKind::Other, subject.to_string()),
    };
    let line = if scope.is_empty() {
        description.to_string()
    } else {
        format!("{scope}: {description}")
    };
    (group, line)
}

/// Parse `git log --no-merges --format=%s` stdout (newest first, one subject
/// per line) into the grouped list the status toast shows. Pure so the
/// taxonomy, the per-group cap, the counts and the ordering are testable
/// without a repository.
///
/// Blank lines are dropped: an empty subject would render as an empty bullet,
/// and it would inflate the count of what the user is waiting for. Empty groups
/// are omitted, so no heading is ever rendered over nothing.
fn parse_pending_commits(stdout: &str) -> PendingCommits {
    let mut totals = [0usize; CommitGroupKind::COUNT];
    let mut descriptions: [Vec<String>; CommitGroupKind::COUNT] = Default::default();
    for subject in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (kind, line) = classify_commit_subject(subject);
        let slot = kind.slot();
        totals[slot] += 1;
        // Housekeeping is counted only, so it collects no lines to send.
        if kind != CommitGroupKind::Housekeeping
            && descriptions[slot].len() < PENDING_COMMIT_DESCRIPTION_CAP
        {
            descriptions[slot].push(line);
        }
    }
    PendingCommits::from_groups(
        COMMIT_GROUP_ORDER
            .into_iter()
            .zip(totals)
            .zip(descriptions)
            .filter(|((_, total), _)| *total > 0)
            .map(|((kind, total), descriptions)| CommitGroup {
                kind,
                total,
                descriptions,
            })
            .collect(),
    )
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
/// appending output to the workspace engine log. The build runs in its own
/// process group so a coalescing abort kills `cargo` too, not just the script
/// (see [`BuildProcessGroupGuard`]). Holds the checkout-shared build lock for the
/// whole build so co-located engines can't run concurrent cargo builds on the
/// same `target/`; returns `SkippedLocked` when a peer already holds it.
async fn run_engine_build(workspace: &std::path::Path) -> EngineBuildOutcome {
    // Elect a single builder across co-located engines (see the lock helper).
    // Held (via `_build_lock`) until this function returns. When the checkout
    // can't be resolved there's no shared `target/` to coordinate on — proceed
    // uncoordinated (fail-open) rather than never building; only a genuinely
    // held lock means a peer is building (→ SkippedLocked).
    let _build_lock = match engine_build_lock_path() {
        Some(path) => match acquire_engine_build_lock_waiting(&path, BUILD_LOCK_WAIT).await {
            Some(guard) => Some(guard),
            None => {
                crate::log!(
                    "[Rebuild] the checkout-shared engine build lock stayed held for {}s, \
                     skipping this build (a co-located workspace is building; its build \
                     advances the shared binary for us too)",
                    BUILD_LOCK_WAIT.as_secs()
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
    // Own process group, so a coalescing abort can reach the `cargo` grandchild
    // and not just this script. See `BuildProcessGroupGuard`.
    crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
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
    // `spawn` + `wait` rather than `status`, so the pid is in hand for the
    // group-kill guard before the wait can be cancelled.
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            crate::log!("[Rebuild] failed to spawn engine build: {e}");
            return EngineBuildOutcome::Failed;
        }
    };
    let mut group_guard = BuildProcessGroupGuard(child.id());
    let status = child.wait().await;
    // Reaped: the pid may now be recycled, so the group must not be signalled.
    group_guard.disarm();
    match status {
        Ok(status) if status.success() => EngineBuildOutcome::Succeeded,
        Ok(status) => {
            crate::log!("[Rebuild] engine build exited {status}");
            EngineBuildOutcome::Failed
        }
        Err(e) => {
            crate::log!("[Rebuild] engine build could not be waited on: {e}");
            EngineBuildOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_engine_build_lock_waiting, build_id_commit, classify_commit_subject,
        classify_pending_commits, commit_is_strict_ancestor, disk_upgrade_verdict, lock_held_at,
        open_teardown, parse_pending_commits, self_heal_is_wedged, stash_first_restart_actor,
        try_lock_file, BuildProcessGroupGuard, BuildState, CommitGroupKind, COMMIT_GROUP_ORDER,
        PENDING_COMMIT_DESCRIPTION_CAP,
    };
    use crate::engine::thread_events::MessageOrigin;
    use std::time::Duration;

    // ── The restart-actor stash ──────────────────────────────────────────────

    fn device(id: &str) -> Option<MessageOrigin> {
        Some(MessageOrigin::Device {
            device_id: id.to_string(),
            label: format!("device {id}"),
        })
    }

    fn device_id_of(slot: &Option<MessageOrigin>) -> Option<&str> {
        match slot {
            Some(MessageOrigin::Device { device_id, .. }) => Some(device_id.as_str()),
            _ => None,
        }
    }

    #[test]
    fn the_first_restart_actor_wins_and_a_later_one_cannot_overwrite_it() {
        // The in-workspace Switch stashes the device that clicked it, then asks
        // the gateway to respawn the stack; the gateway notifies back before it
        // signals, so the SAME restart tries to stash twice. The click's own
        // actor is the one that must survive.
        let mut slot = None;
        assert!(stash_first_restart_actor(&mut slot, device("the-click")));
        assert!(
            !stash_first_restart_actor(&mut slot, device("the-notify")),
            "a second stash must report that it did not store"
        );
        assert_eq!(device_id_of(&slot), Some("the-click"));
    }

    #[test]
    fn stashing_none_never_erases_an_actor_and_never_fills_an_empty_slot() {
        // `None` is the absence of an answer, not an answer. The notify path
        // skips itself when it has no device to name, and nothing else may turn
        // that silence into a cleared stash.
        let mut slot = device("the-click");
        assert!(!stash_first_restart_actor(&mut slot, None));
        assert_eq!(device_id_of(&slot), Some("the-click"));

        let mut empty = None;
        assert!(!stash_first_restart_actor(&mut empty, None));
        assert!(empty.is_none());
    }

    #[test]
    fn a_taken_slot_accepts_the_next_restarts_actor() {
        // First-writer-wins is per restart, not for the engine's lifetime, and
        // this is the half that makes the rule safe. `take_restart_actor` empties
        // the slot both at teardown and when a restart request FAILS before the
        // engine was signalled (`restart_engine` undoing its own stash), and the
        // freed slot has to be writable again: otherwise one abandoned stash
        // would refuse every later restart's actor for the life of the process.
        let mut slot = None;
        assert!(stash_first_restart_actor(&mut slot, device("first")));
        slot.take();
        assert!(stash_first_restart_actor(&mut slot, device("second")));
        assert_eq!(device_id_of(&slot), Some("second"));
    }

    // ── Opening the teardown ─────────────────────────────────────────────────

    #[test]
    fn opening_the_teardown_spends_the_stash_and_keeps_a_copy_for_every_later_emit() {
        // THE 2026-08-07 BUG. The pre-emit is not the only thing that emits an
        // `EngineShutdown` abort during a teardown: `shutdown_active_threads`
        // and `emit_stop_terminal`'s abort arm run after it, for threads that
        // became in-flight after its snapshot. All three must attribute the
        // teardown the same way, because a `Device` actor is half the switch
        // fingerprint and therefore decides the `paused` verdict and the
        // auto-resume. Handing the only copy to the first reader is what left a
        // chat thread on "Response interrupted" with a manual Continue while
        // its two siblings read "Paused by restart" and resumed by themselves.
        let mut restart = device("the-click");
        let mut teardown = None;

        let returned = open_teardown(&mut restart, &mut teardown);

        assert_eq!(
            device_id_of(&returned),
            Some("the-click"),
            "the pre-emit still gets the actor as its argument"
        );
        assert_eq!(
            device_id_of(&teardown),
            Some("the-click"),
            "and every later emit in the same teardown can still read it"
        );
        assert!(
            restart.is_none(),
            "the stash is still SPENT: a later teardown nobody asked for must \
             not inherit this device actor and auto-resume on the strength of it"
        );
    }

    #[test]
    fn a_teardown_nobody_requested_opens_with_no_actor() {
        // A bare `stop.sh`, an external SIGUSR1, ctrl-c. Every emit site falls
        // back to `MessageOrigin::system()`, so the threads settle `failed` and
        // keep their manual Continue: work that may have crashed the engine
        // can't be looped.
        let mut restart = None;
        let mut teardown = None;

        assert!(open_teardown(&mut restart, &mut teardown).is_none());
        assert!(teardown.is_none());
    }

    #[test]
    fn a_non_device_actor_is_still_stashable_by_this_rule() {
        // The device-only requirement is enforced at the HTTP boundary (the
        // restart-intent handler 400s a non-device caller), NOT here: the
        // in-workspace Switch legitimately stashes whatever `user_actor_resolved`
        // gave it, and downstream reads the actor's kind for itself. Pinned so a
        // later "tighten the stash" edit has to notice it would change that path.
        let mut slot = None;
        let api = Some(MessageOrigin::Api {
            user_agent: Some("curl".to_string()),
            mode: crate::engine::thread_events::ActorMode::Human,
            source_thread_id: None,
        });
        assert!(stash_first_restart_actor(&mut slot, api));
        assert!(slot.is_some());
    }

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

    /// A lock that frees DURING the wait is acquired, not reported as a peer
    /// build. This is the regression: a build superseded by a coalescing Apply
    /// is still dropping its guard when the replacement probes, and a single
    /// instantaneous try turned that millisecond into `SkippedLocked` →
    /// `BuildState::Idle`, leaving no build running at all (2026-08-05).
    #[tokio::test]
    async fn build_lock_wait_rides_out_a_holder_that_is_about_to_release() {
        let dir = std::env::temp_dir().join(format!("lucidos-lockwait-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".lucidos-engine-build.lock");

        let held = eventually_acquire(&path);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            drop(held);
        });
        assert!(
            acquire_engine_build_lock_waiting(&path, Duration::from_secs(5))
                .await
                .is_some(),
            "a lock released partway through the wait must be acquired, not read as a peer build"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other direction, so the wait can't paper over a genuine peer build:
    /// a lock held for the whole window still yields `None` (→ `SkippedLocked`,
    /// and the peer-build spinner rather than a second concurrent cargo).
    #[tokio::test]
    async fn build_lock_wait_gives_up_on_a_holder_that_never_releases() {
        let dir = std::env::temp_dir().join(format!("lucidos-lockheld-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".lucidos-engine-build.lock");

        let _held = eventually_acquire(&path);
        assert!(
            acquire_engine_build_lock_waiting(&path, Duration::from_millis(150))
                .await
                .is_none(),
            "a lock held for the whole wait must still report the peer build"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A coalescing abort must take the GRANDCHILD down with the script.
    /// `kill_on_drop` reaps only the direct child (`web-dev.sh`), so before the
    /// group guard a superseded build left its `cargo` compiling against the
    /// shared `target/` next to the live build. Modelled with a shell that
    /// backgrounds a `sleep` and waits on it, which is the same shape.
    #[tokio::test]
    async fn dropping_the_build_group_guard_kills_the_grandchild() {
        let dir = std::env::temp_dir().join(format!("lucidos-buildgroup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("grandchild.pid");

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                "sleep 60 & echo $! > {}; wait",
                pid_file.to_string_lossy()
            ))
            .kill_on_drop(true);
        crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn the group leader");

        let grandchild = read_pid_when_written(&pid_file).expect("grandchild pid");
        assert!(pid_is_alive(grandchild), "the grandchild must start alive");

        // Exactly what a cancelled build future does: guard first (declared
        // last), then the child.
        drop(BuildProcessGroupGuard(child.id()));
        child.start_kill().ok();
        child.wait().await.ok();

        assert!(
            eventually(|| !pid_is_alive(grandchild)),
            "the group kill must reach the grandchild; kill_on_drop alone leaves it running"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Poll for the pid the shell writes once it has backgrounded its child.
    fn read_pid_when_written(path: &std::path::Path) -> Option<i32> {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    return Some(pid);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// `ps -p` rather than `kill(pid, 0)`, to keep the test free of `unsafe`.
    /// The grandchild is reparented to init when its shell dies, so init reaps
    /// it and it leaves the table outright rather than lingering as a zombie.
    fn pid_is_alive(pid: i32) -> bool {
        std::process::Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
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

    /// The two halves of the group indexing have to agree: `slot()` is what
    /// tallies are counted into and `COMMIT_GROUP_ORDER` is what they are read
    /// back out as, so a kind counted at one index and emitted at another would
    /// silently attribute every `feat` to the wrong heading. The compiler
    /// already forces a new variant to answer `slot()`; this forces it into the
    /// order array at the matching position.
    #[test]
    fn every_group_reads_back_out_at_the_index_it_was_counted_into() {
        for (index, kind) in COMMIT_GROUP_ORDER.into_iter().enumerate() {
            assert_eq!(kind.slot(), index, "{kind:?} is misplaced in the order");
        }
    }

    /// The taxonomy: each conventional-commit type lands in the group the toast
    /// describes it under, the type tag is stripped, and the scope is kept as a
    /// lead-in so the line names its area.
    #[test]
    fn classify_commit_subject_maps_the_type_and_keeps_the_scope() {
        let cases = [
            (
                "feat(memory): one cache per user",
                CommitGroupKind::New,
                "memory: one cache per user",
            ),
            ("feat: no scope here", CommitGroupKind::New, "no scope here"),
            (
                "feat(api)!: a breaking one",
                CommitGroupKind::New,
                "api: a breaking one",
            ),
            (
                "fix(ui): the trash is sized by its ink",
                CommitGroupKind::Fixed,
                "ui: the trash is sized by its ink",
            ),
            (
                "style(triggers): the actions stack",
                CommitGroupKind::Improved,
                "triggers: the actions stack",
            ),
            (
                "perf: fewer probes",
                CommitGroupKind::Improved,
                "fewer probes",
            ),
            (
                "refactor: one writer",
                CommitGroupKind::Improved,
                "one writer",
            ),
            (
                "docs(plans): a plan",
                CommitGroupKind::Housekeeping,
                "plans: a plan",
            ),
            (
                "harden(ui): pinned",
                CommitGroupKind::Housekeeping,
                "ui: pinned",
            ),
        ];
        for (subject, kind, line) in cases {
            assert_eq!(
                classify_commit_subject(subject),
                (kind, line.to_string()),
                "{subject}"
            );
        }
    }

    /// A subject we could not classify keeps every word of itself. Stripping a
    /// tag we did not understand would lose information for nothing, and the
    /// commit that is hardest to categorize is often the interesting one.
    #[test]
    fn classify_commit_subject_leaves_an_unrecognized_subject_whole() {
        for subject in [
            "Merge branch 'main' into some-branch",
            "wip: an unknown type",
            "Revert \"feat(ui): a thing\"",
            "no colon at all",
            "Note to self: not a conventional type",
        ] {
            assert_eq!(
                classify_commit_subject(subject),
                (CommitGroupKind::Other, subject.to_string()),
                "{subject}"
            );
        }
    }

    /// Each group's list is capped so the toast stays glanceable, but the COUNTS
    /// are not: "and N more" is only honest if every commit was seen. The cap is
    /// PER GROUP, which is what stops a pile of doc commits from crowding out
    /// the one feature. Blank lines are dropped rather than counted, since an
    /// empty subject would both render as an empty bullet and inflate what the
    /// user is waiting for.
    #[test]
    fn parse_pending_commits_groups_and_caps_each_list_but_not_the_counts() {
        let mut log = String::new();
        for i in 1..=8 {
            log.push_str(&format!("fix: bug {i}\n"));
        }
        log.push_str("feat: the one feature\n");
        for i in 1..=4 {
            log.push_str(&format!("docs: page {i}\n"));
        }
        let parsed = parse_pending_commits(&log);

        assert_eq!(parsed.total, 13, "every commit counts toward the total");
        assert_eq!(
            parsed.total,
            parsed.groups.iter().map(|g| g.total).sum::<usize>(),
            "the headline count reconciles with the groups under it"
        );
        assert_eq!(
            parsed.groups.iter().map(|g| g.kind).collect::<Vec<_>>(),
            vec![
                CommitGroupKind::New,
                CommitGroupKind::Fixed,
                CommitGroupKind::Housekeeping,
            ],
            "listed in display order, and a group with nothing in it is omitted"
        );

        let fixed = &parsed.groups[1];
        assert_eq!(fixed.total, 8);
        assert_eq!(fixed.descriptions.len(), PENDING_COMMIT_DESCRIPTION_CAP);
        assert_eq!(
            fixed.descriptions[0], "bug 1",
            "git log order is preserved (newest first)"
        );
        assert_eq!(
            parsed.groups[0].descriptions,
            vec!["the one feature"],
            "the lone feature survives eight fixes ahead of it"
        );

        let housekeeping = &parsed.groups[2];
        assert_eq!(housekeeping.total, 4);
        assert!(
            housekeeping.descriptions.is_empty(),
            "housekeeping is counted, never listed"
        );

        // Blank and whitespace-only lines are not commits.
        let ragged = parse_pending_commits("fix: one\n\n   \nfix: two\n");
        assert_eq!(ragged.total, 2);
        assert_eq!(ragged.groups[0].descriptions, vec!["one", "two"]);

        // A genuinely empty range is a real answer: zero, with nothing to list.
        let none = parse_pending_commits("");
        assert_eq!(none.total, 0);
        assert!(none.groups.is_empty());
    }

    /// The distinction the whole field rests on: git saying "no commits" is
    /// `Some(total: 0)`, git failing to say anything is `None`. Collapsing the
    /// second into the first would tell the user nothing is coming while a build
    /// is running (`.claude/rules/rust.md`: unknown is never a no).
    #[test]
    fn classify_pending_commits_keeps_unknown_apart_from_none() {
        // A spawn failure or the git timeout is unknowable, so it is no verdict.
        assert_eq!(
            classify_pending_commits(Err("git log timed out after 30s".to_string())),
            None
        );
    }

    /// Elapsed exists exactly while a build does, so the toast cannot show a
    /// timer for work that is not running.
    #[test]
    fn build_state_reports_elapsed_only_while_building() {
        assert!(BuildState::building_now().elapsed().is_some());
        assert!(BuildState::Idle.elapsed().is_none());
        assert!(BuildState::Ready.elapsed().is_none());
        assert!(BuildState::Failed.elapsed().is_none());
        assert_eq!(BuildState::building_now().as_wire(), "building");
    }

    /// The real range against a throwaway repo: `<running>..HEAD` lists what a
    /// switch would bring, newest first, with the running commit itself
    /// excluded and every MERGE dropped (its subject is a branch name, and what
    /// it merged is already in the range). And a range git cannot resolve
    /// classifies as UNKNOWN rather than as empty.
    #[tokio::test]
    async fn pending_commits_reads_the_range_between_the_running_commit_and_head() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-pending-commits-{}-{:?}",
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
        git(&["commit", "-qm", "running: the version in use"]);
        let running = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        for subject in ["fix: the older one", "feat: the newer one"] {
            std::fs::write(dir.join("a.txt"), subject).unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", subject]);
        }
        // A side branch merged back in, exactly as an Apply lands: the merge
        // subject names the branch and must not reach the toast, while the work
        // it brought must.
        git(&["checkout", "-q", "-b", "side", &running]);
        std::fs::write(dir.join("b.txt"), "side work").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "fix(side): work done on a branch"]);
        // `-` rather than a branch NAME: `init.defaultBranch` is the user's
        // config, so this repo's trunk is `master` on one machine and `main` on
        // the next.
        git(&["checkout", "-q", "-"]);
        git(&[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge branch 'side'",
            "side",
        ]);

        let range = format!("{running}..HEAD");
        let commits = classify_pending_commits(
            crate::engine::git_ops::git_cmd(&["log", "--no-merges", "--format=%s", &range], &dir)
                .await,
        )
        .expect("a resolvable range is a real answer");
        assert_eq!(
            commits.total, 3,
            "the merge is not a commit the user is waiting for; its content is"
        );
        let group = |kind| {
            commits
                .groups
                .iter()
                .find(|g| g.kind == kind)
                .unwrap_or_else(|| panic!("{kind:?} group is present"))
        };
        assert_eq!(
            group(CommitGroupKind::New).descriptions,
            vec!["the newer one"],
            "the running commit is not part of what is coming"
        );
        // Not order-asserted across the two: `git log` sorts by commit date, and
        // the branch commit is younger than the trunk commits it merges beside.
        let mut fixed = group(CommitGroupKind::Fixed).descriptions.clone();
        fixed.sort();
        assert_eq!(
            fixed,
            vec!["side: work done on a branch", "the older one"],
            "the branch's own work is listed, under its own subject"
        );
        assert!(
            !commits
                .groups
                .iter()
                .flat_map(|g| g.descriptions.iter())
                .any(|d| d.contains("Merge branch")),
            "no merge subject reaches the toast"
        );

        // A range git refuses (unknown object) exits non-zero: unknown, not empty.
        assert_eq!(
            classify_pending_commits(
                crate::engine::git_ops::git_cmd(
                    &[
                        "log",
                        "--no-merges",
                        "--format=%s",
                        "0000000000000000000000000000000000000000..HEAD",
                    ],
                    &dir,
                )
                .await
            ),
            None,
            "a range git cannot resolve says nothing about what is pending"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
