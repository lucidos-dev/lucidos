//! Engine-version identity + "new version available" detection.
//!
//! The dev half of the unified *Switch to new version* flow (see
//! `docs/plans/2026-07-01-new-engine-version-switch-flow.md`). An *Apply*
//! rebuilds the engine binary on disk in the background. A running engine then
//! detects that the on-disk binary differs from the one it runs, and surfaces
//! the switch. Mirrors the gateway self-reload (`GATEWAY_BUILD_ID`,
//! `gateway_update_available`).
//!
//! Packaged builds never rebuild from source, so `update_available` is always
//! false here and `BuildState` stays `Idle`. Their "new version" source is the
//! release updater, `updater.rs`.

use super::background_build::{BackgroundBuild, BuildHost, FinishedBuild};
use super::event_bus::{BusEvent, EventBus, SystemEvent};
use super::LucidosEngine;
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ENGINE_BUILD_LOG_PATH: &str = ".lucidos/engine-build.log";

/// How long a `source_behind_head` verdict is reused before the underlying
/// `git diff` is re-run. Events can prompt several clients to read; a shared TTL
/// bounds the git fork to at most one per interval regardless of client count.
/// Short enough that a freshly-merged engine change surfaces within a few seconds.
const SOURCE_BEHIND_TTL: Duration = Duration::from_secs(3);

/// How long a [`PendingCommits`] read is reused before `git log` is re-run.
/// Same rationale and the same window as [`SOURCE_BEHIND_TTL`].
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
    /// runs. In the variant rather than beside it, so "building, but nobody
    /// knows since when" is unrepresentable. In-memory only: a build dies with
    /// the engine, and a restart starts from `Idle`.
    Building { started_at: Instant },
    /// Checking the published executable after a successful build.
    Verifying,
    /// The published executable is newer than the running engine and covers HEAD.
    ///
    /// Carries the HEAD whose delivery was proved, rather than assuming that
    /// a successful script built everything present when it finished.
    Ready {
        built_head: Option<String>,
        deferred: Option<DeferredRequest>,
    },
    /// An automatic build waiting for coding threads.
    Deferred(DeferredRequest),
    /// The rebuild failed. The old engine keeps running.
    ///
    /// Carries WHY, for the same reason `Building` carries its instant. The
    /// toast has to tell the user something they can act on. "Failed, but
    /// nobody knows why" is not representable: a failure no recognizer names
    /// still reports its exit status and last output line.
    Failed { reason: BuildFailure },
}

/// Ephemeral waiting request. Ready may retain one without hiding Switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredRequest {
    pub since: Instant,
    pub retrying: Option<BuildFailure>,
}

/// Why a background build failed, reduced to what a toast can carry.
///
/// A concise cause for the toast; full build output is available separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BuildFailure {
    /// The first real error line from the build, already trimmed.
    pub summary: String,
    /// A likely fix, when the output looks like a class we recognize. A
    /// SUGGESTION shown beside the error, never a verdict, so it does not
    /// decide whether Retry is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
    /// Whether an identical recognized artifact failure survived a retry.
    /// Diagnostic only: the user can always request another build.
    ///
    /// **Proof takes a recognized failure AND an observed repeat**, because
    /// either alone over-fires. See the two paragraphs in
    /// [`classify_build_failure`] for the false positive each one prevents.
    ///
    /// So an unreadable, unrecognized, or first-time failure is `false`.
    pub repeatable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<BuildFailureKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildFailureKind {
    NotDelivered,
    UnsupportedLaunch,
}

/// The fresh retry budget belongs to the observed HEAD, including unknown.
#[derive(Default)]
struct DeliveryRetry {
    attempted: std::collections::HashSet<Option<String>>,
}
impl DeliveryRetry {
    fn can_fresh(&self, head: Option<&str>) -> bool {
        !self.attempted.contains(&head.map(str::to_owned))
    }

    fn fresh_for(&mut self, head: Option<&str>) -> bool {
        let head = head.map(str::to_owned);
        self.attempted.insert(head)
    }
}

/// Linux retains the unlinked inode as `/proc/self/exe` after atomic publish.
/// Probe the published pathname, rather than the old inode's deleted spelling.
pub fn published_engine_exe() -> Option<std::path::PathBuf> {
    Some(published_engine_path(
        std::env::current_exe().ok()?,
        cfg!(target_os = "linux"),
    ))
}

fn published_engine_path(exe: std::path::PathBuf, linux: bool) -> std::path::PathBuf {
    if linux && exe.file_name() == Some(std::ffi::OsStr::new("lucidos-engine (deleted)")) {
        return exe.with_file_name("lucidos-engine");
    }
    exe
}

/// A source build and a standalone Switch must target the same published layout.
pub fn published_launch_dir(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    (dir.parent()?.parent()?.file_name()? == std::ffi::OsStr::new(".launch"))
        .then(|| dir.to_path_buf())
}

async fn uncached_engine_disk_build_id() -> Option<String> {
    disk_build_id_at(&published_engine_exe()?).await
}

#[doc(hidden)]
pub async fn disk_build_id_at(exe: &std::path::Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(exe)
            .arg("--build-id")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!id.is_empty()).then_some(id)
}

impl BuildFailure {
    pub fn delivery(kind: BuildFailureKind, summary: &str) -> Self {
        Self {
            kind: Some(kind),
            ..Self::plain(summary.into())
        }
    }

    /// A failure with a summary and nothing else: no remedy to suggest, and
    /// never proved repeatable, so Retry stays offered.
    pub fn plain(summary: String) -> Self {
        BuildFailure {
            summary,
            remedy: None,
            repeatable: false,
            kind: None,
        }
    }
}

/// Longest summary a toast should carry. Past this the line stops being
/// readable on a phone, which is the device this whole shape exists for.
const BUILD_FAILURE_SUMMARY_CAP: usize = 200;

/// Reduce a failed build's output to the one line worth showing, and decide
/// whether retrying could ever change the answer.
///
/// `previous_summary` is the failure the build being reported was retrying, or
/// `None` when it was not retrying one. It is half of what sets
/// [`BuildFailure::repeatable`]; the recognized shape is the other half, and
/// the body says why neither is sufficient alone.
///
/// Pure, so every recognizer is testable without running a build.
///
/// Three recognizers, tried most specific first: a build-script panic, a cargo
/// error line, then the wrapper script's own `ERROR:` for a build that died
/// before cargo ran. See [`script_error`] for why the last one exists.
///
/// `None` when the output carries no error line at all. This function never
/// invents a cause; the caller reports what it saw instead, through
/// [`unrecognized_build_failure`].
pub fn classify_build_failure(
    output: &str,
    previous_summary: Option<&str>,
) -> Option<BuildFailure> {
    // The `error:` line and the panic message are two different things, and
    // for a build script the panic is the informative one: cargo's own line
    // only ever says "failed to run custom build command".
    let error_line = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("error:") || l.starts_with("error["));
    let panic_message = output
        .lines()
        .skip_while(|l| !l.contains("panicked at"))
        .nth(1)
        .map(str::trim)
        .filter(|l| !l.is_empty());

    let summary = cap_summary(
        panic_message
            .or(error_line)
            .or_else(|| script_error(output))?,
    );

    // BOTH halves are required, and each answers a way the other is wrong.
    //
    // Repetition alone over-fires, because a summary is not an identity. Two
    // unrelated type errors both read `error[E0308]: mismatched types`, so a
    // user fixing one and meeting the next would lose the button mid-progress.
    //
    // The shape alone over-fires too. A build script reporting a missing file
    // may be missing a real input, which a rebuild fixes once it is back.
    //
    // Record a specific recognized failure observed to survive a retry.
    let remedy = stale_artifact_remedy(output);
    Some(BuildFailure {
        repeatable: remedy.is_some() && previous_summary == Some(summary.as_str()),
        remedy,
        summary,
        kind: None,
    })
}

/// The failure no recognizer names, described by what the engine saw: the exit
/// status, and the last line the build printed.
///
/// This is what keeps a silent failure honest. A build can die before it
/// prints anything, and the toast then said the output could not be read. The
/// output had been read; there was none. Pure, so both shapes are testable.
pub fn unrecognized_build_failure(exit: &str, last_line: Option<&str>) -> BuildFailure {
    let summary = match last_line {
        Some(line) => format!("the build stopped ({exit}) after printing \"{line}\""),
        None => format!("the build stopped ({exit}) without printing anything"),
    };
    BuildFailure::plain(cap_summary(&summary))
}

/// Trim a summary to [`BUILD_FAILURE_SUMMARY_CAP`] without splitting a
/// character.
fn cap_summary(summary: &str) -> String {
    summary
        .get(..summary.floor_char_boundary(BUILD_FAILURE_SUMMARY_CAP))
        .unwrap_or(summary)
        .trim_end()
        .to_string()
}

/// The wrapper script's own refusal, for a build that died before cargo ran.
///
/// `web-dev.sh --engine-build` is a shell script and can fail on its own terms.
/// A missing tool, a Docker daemon that is down, a preflight that refuses. Each
/// prints `ERROR:` at line start, the convention every `scripts/lib/*.sh` uses,
/// and none is a cargo diagnostic. The two recognizers above therefore find
/// nothing, while the script's own line names the cause.
///
/// RANKED LAST, after the panic and the cargo error, and that ordering is the
/// point. A compile failure is the more specific cause whenever both appear.
///
/// Returns the text AFTER the marker. `ERROR:` is how the script shouts, not
/// part of what it says, and the toast already frames the line as a failure. A
/// blank remainder is no answer, so it falls through to the next `ERROR:` line.
fn script_error(output: &str) -> Option<&str> {
    output
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("ERROR:"))
        .map(str::trim)
        .find(|l| !l.is_empty())
}

/// `cargo clean` for a build script that failed on a path which is not there.
/// That is the shape of a stale cached artifact (ADR 0079). Such a binary is
/// reused across checkouts sharing a target directory. It can therefore name a
/// tree that is gone, which cargo calls fresh forever.
///
/// A SUGGESTION, never a proof. The same output comes from a build script whose
/// input is genuinely missing. There a clean fixes nothing and restoring the
/// file fixes everything. So this names a likely fix beside the error, and
/// never decides whether Retry is offered.
fn stale_artifact_remedy(output: &str) -> Option<String> {
    let build_script_failed = output.contains("failed to run custom build command");
    let path_missing = output.contains("No such file or directory") || output.contains("NotFound");
    if !(build_script_failed && path_missing) {
        return None;
    }
    Some(match failing_package(output) {
        Some(pkg) => format!("cargo clean -p {pkg}"),
        None => "cargo clean".to_string(),
    })
}

/// The package name out of cargo's ``failed to run custom build command for
/// `name v1.2.3 (/path)` `` line, so the remedy can name `-p <package>`.
///
/// `None` rather than a guess when the line is not shaped as expected: a wrong
/// `-p` would send the user to clean a package that is not the broken one.
fn failing_package(output: &str) -> Option<&str> {
    let tail = output.split("custom build command for `").nth(1)?;
    let name = tail.split([' ', '`']).next()?;
    (!name.is_empty()).then_some(name)
}

impl BuildState {
    /// Kebab-case wire tag for the version-status response.
    pub fn as_wire(&self) -> &'static str {
        match self {
            BuildState::Idle => "idle",
            BuildState::Deferred(_) => "deferred",
            BuildState::Building { .. } => "building",
            BuildState::Verifying => "verifying",
            BuildState::Ready { .. } => "ready",
            BuildState::Failed { .. } => "failed",
        }
    }

    /// Why this build failed, or `None` when it is not a failed build.
    pub fn failure(&self) -> Option<&BuildFailure> {
        match self {
            BuildState::Failed { reason } => Some(reason),
            BuildState::Deferred(deferred) => deferred.retrying.as_ref(),
            _ => None,
        }
    }

    pub fn deferred(&self) -> Option<&DeferredRequest> {
        match self {
            Self::Deferred(deferred)
            | Self::Ready {
                deferred: Some(deferred),
                ..
            } => Some(deferred),
            _ => None,
        }
    }

    pub(crate) fn with_deferral(self) -> Self {
        if self.deferred().is_some() {
            return self;
        }
        let deferred = DeferredRequest {
            since: Instant::now(),
            retrying: self.failure().cloned(),
        };
        match self {
            Self::Ready { built_head, .. } => Self::Ready {
                built_head,
                deferred: Some(deferred),
            },
            _ => Self::Deferred(deferred),
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

    /// A finished build stamped with the HEAD it was started from. The one
    /// constructor for `Ready`, so a completion site cannot forget the stamp.
    pub fn ready_from(built_head: Option<String>) -> Self {
        BuildState::Ready {
            built_head,
            deferred: None,
        }
    }

    /// A failed build carrying why. The one constructor for `Failed`, matching
    /// its two siblings above.
    pub fn failed_with(reason: BuildFailure) -> Self {
        BuildState::Failed { reason }
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

/// May an observation that began at `started` replace the cached one, stamped
/// `stored`? Only when no later observation already wrote. A slow probe that
/// began before HEAD moved would otherwise overwrite the watcher's newer
/// verdict, and the watcher would never announce the change again.
fn observation_is_newest(stored: Option<Instant>, started: Instant) -> bool {
    stored.is_none_or(|at| at <= started)
}

/// Throttled cache of the `source_behind_head` verdict (see
/// [`LucidosEngine::source_behind_head`]). `checked_at == None` means "never
/// computed". TTL-gated by `SOURCE_BEHIND_TTL`, so the git probe runs at most
/// once per interval across all status readers.
#[derive(Default)]
pub struct SourceBehindCache {
    checked_at: Option<Instant>,
    behind: bool,
}

/// Throttled cache of the checkout's HEAD sha (see [`LucidosEngine::head_sha`]).
/// `checked_at == None` means "never read". A cached `sha` of `None` is a cached
/// UNKNOWN, deliberately cached so a broken git is not re-forked on every poll.
/// TTL-gated by [`SOURCE_BEHIND_TTL`], the window the sibling probes use.
#[derive(Default)]
pub struct HeadShaCache {
    checked_at: Option<Instant>,
    sha: Option<String>,
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
    /// [`group_commit_subjects`] counts into.
    const COUNT: usize = 5;

    /// This kind's index in those tallies, and in [`COMMIT_GROUP_ORDER`].
    ///
    /// An exhaustive `match` rather than a lookup in the order array: adding a
    /// variant then has to answer this, at compile time, instead of panicking
    /// on a `position(...).unwrap()` inside a version-status read.
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

/// The commits a *Switch to new version* would bring, grouped by what they are.
/// Surfaced on `version_status`, and read by two client surfaces: the status
/// toast behind the spinning brand badge, which says what is being BUILT, and
/// the new-version confirm, which says what the switch would BRING.
///
/// **Only what the switch adds** ([`pending_commits_since`]): the list leaves
/// out whatever the served client already carries.
///
/// **Merges are excluded** ([`pending_commits_since`] passes `--no-merges`). An
/// Apply lands as a merge named after its branch, which describes no work. What
/// it merged is already in this range under its own subjects.
///
/// "commits", not "changes". A *change* is the coding-agent change the user
/// Applies (see `system-knowhow/glossary.md`), and not every commit here is one
/// (a hand commit, a revert).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingCommits {
    /// Every commit the switch brings. Computed from `groups` by
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
/// [`SourceBehindCache`]. `checked_at == None` means "never computed", and
/// `commits == None` is a cached UNKNOWN, cached so a wedged git is not
/// re-forked per poll. `checked_at` is the latest claim and gates the TTL;
/// `observed_at` is when the read behind `commits` began.
#[derive(Default)]
pub struct PendingCommitsCache {
    checked_at: Option<Instant>,
    observed_at: Option<Instant>,
    commits: Option<PendingCommits>,
}

impl PendingCommitsCache {
    /// Store a finished read unless a later-started read already stored its
    /// answer. A claim still reading does not block it: the watcher announces
    /// what it read, so the cache must serve that until something newer lands.
    fn record(&mut self, started: Instant, commits: Option<PendingCommits>) {
        if observation_is_newest(self.observed_at, started) {
            self.observed_at = Some(started);
            self.commits = commits;
        }
    }
}

/// Memoized ancestry verdict for the on-disk binary (see
/// [`LucidosEngine::disk_binary_is_upgrade`]). Keyed by the on-disk build id
/// alone, because the RUNNING id is a compile-time constant.
/// `is_strict_ancestor` is the cached
/// `git merge-base --is-ancestor <disk> <running>` answer; `None` means git
/// could not tell.
#[derive(Default)]
pub struct DiskDirectionCache {
    disk_id: Option<String>,
    is_strict_ancestor: Option<bool>,
}

/// The variable that pins an engine to the build it runs. Only
/// `scripts/lib/e2e.sh` sets it, so a commit landing in the checkout mid-run
/// cannot rebuild the e2e engine or raise a version toast over its specs.
pub const PIN_ENGINE_VERSION_ENV: &str = "LUCIDOS_PIN_ENGINE_VERSION";

/// Does this dev engine look past its own build for a newer version?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionTracking {
    /// A newer HEAD or on-disk binary is a new version, and self-heal rebuilds
    /// toward it. Every engine but the e2e one.
    FollowCheckout,
    /// The build this engine runs is the newest it will ever report. Nothing
    /// newer surfaces, and self-heal never rebuilds.
    PinnedToBuild,
}

impl VersionTracking {
    /// Read [`PIN_ENGINE_VERSION_ENV`]. Once, at construction: a run is pinned
    /// for its whole life or not at all.
    pub fn from_env() -> Self {
        Self::from_flag(std::env::var_os(PIN_ENGINE_VERSION_ENV).as_deref())
    }

    /// Only `1` pins, the convention `LUCIDOS_PACKAGED` uses. Anything else
    /// follows the checkout, so a stray value can never freeze a dev engine.
    fn from_flag(flag: Option<&std::ffi::OsStr>) -> Self {
        if flag.is_some_and(|v| v == "1") {
            VersionTracking::PinnedToBuild
        } else {
            VersionTracking::FollowCheckout
        }
    }
}

/// May an engine report a version newer than the one it runs?
///
/// The one gate three probes share: [`LucidosEngine::source_behind_head`]
/// (which also drives self-heal), the on-disk binary check, and the
/// served-frontend peer sync. Packaged engines learn of new versions from the
/// release updater instead, so they answer no too.
fn newer_version_visible(tracking: VersionTracking, packaged: bool) -> bool {
    tracking == VersionTracking::FollowCheckout && !packaged
}

/// Per-HEAD self-heal attempt bookkeeping (see
/// [`LucidosEngine::self_heal_engine_version_if_needed`]). `head` is the HEAD
/// the attempts were counted for; when HEAD moves the counter resets.
#[derive(Default)]
pub struct SelfHealState {
    head: Option<String>,
    attempts: u32,
}

impl SelfHealState {
    fn request(
        &mut self,
        head: Option<String>,
        state: &BuildState,
        deferral: &super::build_deferral::BuildDeferral,
    ) -> bool {
        if self.head != head {
            self.head = head;
            self.attempts = 0;
        }
        if self.attempts >= SELF_HEAL_MAX_ATTEMPTS_PER_HEAD
            || (self.attempts > 0 && state.failure().is_some())
            || matches!(state, BuildState::Building { .. } | BuildState::Verifying)
        {
            return false;
        }
        if matches!(deferral, super::build_deferral::BuildDeferral::Admit) {
            self.attempts += 1;
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FrontendBuildWatchStopped {
    pub since_ms: i64,
    pub cause: Option<String>,
}

/// Wire shape of `GET /api/v1/engine/version-status`.
#[derive(Serialize)]
pub struct VersionStatus {
    /// The running engine's baked build id.
    pub build_id: String,
    /// True when a newer engine version is ready to switch onto (dev: the
    /// on-disk binary build id differs). Always false packaged.
    pub update_available: bool,
    /// The on-disk binary's build id (dev), or `None` when packaged or
    /// unreadable. The frontend keys the "Switch to new version" dismissal on
    /// this. A dismiss then sticks for THIS on-disk build, while a genuinely
    /// newer build re-surfaces the switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_build_id: Option<String>,
    /// True under the packaged desktop/service runtime. The frontend uses it to
    /// route to the release updater instead of the dev build/switch flow.
    pub packaged: bool,
    /// Dev background-rebuild state, including `deferred` and `verifying`.
    pub build_state: &'static str,
    /// True (dev only) when the engine SOURCE is behind HEAD by a
    /// restart-requiring change, so a NEW engine version exists in source even
    /// with no fresh binary on disk. Distinct from `update_available`, which
    /// means a fresh binary IS on disk. The frontend uses it to surface a
    /// pending version and drive self-heal, so the Switch cannot dead-end.
    /// Always false packaged or when git is unavailable.
    pub source_behind_head: bool,
    /// True (dev only) when the checkout-shared engine-build lock is held, by a
    /// CO-LOCATED peer engine's `run_engine_build` or by this engine's own
    /// build. Co-located workspaces share one `target/` and one build lock, so a
    /// peer's build advances the binary THIS engine serves. A workspace that
    /// lost the lock would otherwise read as idle and wrongly offer the manual
    /// "Rebuild" escape hatch instead of the spinner.
    ///
    /// Named "shared" rather than "peer" because the probe cannot distinguish
    /// this engine's own held lock from a peer's. The frontend disambiguates via
    /// `build_state`. Always false packaged.
    pub shared_build_in_progress: bool,
    /// The checkout's HEAD sha, or absent when git could not say, when
    /// packaged, or when nothing is pending. **Identity, never display**: the
    /// frontend pins a dismissal of the *pending* version toast to it, the way
    /// it pins the *Switch* toast to `disk_build_id`. A pending version has no
    /// on-disk build id to key on, which is precisely what makes it pending.
    /// When HEAD moves there is something new to announce, and the old dismissal
    /// stops matching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    /// Why the last build failed, absent unless `build_state` is `failed`.
    ///
    /// The toast shows the cause alongside a link to build output.
    /// Always present on a failure from this engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_failure: Option<BuildFailure>,
    /// How long THIS engine's own background rebuild has been running, in ms,
    /// or absent when no build of ours is in flight. A co-located peer's build
    /// is absent too, since we do not have its clock.
    ///
    /// ELAPSED rather than a start timestamp on purpose. Differencing an engine
    /// wall-clock against the browser's shows a wrong or negative duration
    /// whenever the two clocks disagree. The client anchors this to its own
    /// `Date.now()` at receipt and counts up locally, so skew cannot reach the
    /// number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_elapsed_ms: Option<u64>,
    /// Present while THIS engine's rebuild waits for a *build slot* rather
    /// than compiling, naming what holds the slots. Absent otherwise, and
    /// always absent for a co-located peer's build. See [`queued_behind`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_queued: Option<QueuedBuild>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_deferred: Option<DeferredBuild>,
    /// The commits the switch would bring, or absent when git could not say
    /// (see [`PendingCommits`]). Read only when a surface
    /// will show it, so an idle workspace forks no git: see
    /// [`wants_pending_commits`] for the four reasons.
    ///
    /// Absent is UNKNOWN, never "none pending": a `Some` with `total: 0` is the
    /// only way to say there is nothing to bring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_commits: Option<PendingCommits>,
    /// How long THIS engine has been waiting for the build-watch to republish
    /// `dist/` after a frontend-only Apply, in ms, or absent when it is not.
    /// Elapsed rather than a timestamp, for the reason `build_elapsed_ms` gives.
    /// Always absent packaged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_refresh_elapsed_ms: Option<u64>,
    /// Sustained build-watch downtime in the checkout this engine serves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_build_watch_stopped: Option<FrontendBuildWatchStopped>,
}

/// Waiting on workspace coding activity, independently of the build slots.
#[derive(Clone, Debug, Serialize)]
pub struct DeferredBuild {
    pub coding_threads: i64,
    pub threads: Vec<super::build_deferral::ActiveThread>,
    pub deferred_ms: u64,
}

/// A rebuild that is waiting for a *build slot*, as the version-status wire
/// carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueuedBuild {
    /// What holds each slot, by the label its wrapper recorded.
    pub holders: Vec<String>,
}

/// Stands in for a holder that recorded no label.
const UNLABELLED_HOLDER: &str = "a build";

/// Is the build running in process group `our_group` queued for a slot?
///
/// Queued means every slot is held and no holder is in our group. A slot's
/// holder is the `lucidos build-slot` wrapper, which stays in the build's group
/// on purpose (ADR 0070). So a holder in our group means we compile.
/// Pure over the probe results, so each case is testable without a pool.
fn queued_behind(
    our_group: Option<u32>,
    slots: &[lucidos_build_slot::SlotState],
    group_of: impl Fn(u32) -> Option<u32>,
) -> Option<QueuedBuild> {
    let our_group = our_group?;
    if slots.is_empty() {
        return None;
    }
    let mut holders = Vec::with_capacity(slots.len());
    for slot in slots {
        let holder = slot.holder.as_ref()?;
        if holder.pid.and_then(&group_of) == Some(our_group) {
            return None;
        }
        holders.push(if holder.label.is_empty() {
            UNLABELLED_HOLDER.to_string()
        } else {
            holder.label.clone()
        });
    }
    Some(QueuedBuild { holders })
}

/// The stash rule, extracted from [`LucidosEngine::stash_restart_actor`] so it
/// is testable without an engine: **first writer wins, and `None` is never
/// stored**. Returns whether `actor` was taken.
///
/// Two writers race for one slot on a single restart. The in-workspace *Switch*
/// (`/api/v1/restart`) stashes the device that clicked it and only THEN asks the
/// gateway to respawn the stack. The gateway notifies the engine back before it
/// signals (`/api/v1/internal/restart-intent`), so the same restart arrives
/// twice. Keeping the first makes the second harmless: where the two could
/// disagree, the one holding the click's HTTP context is the honest answer.
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
/// * KEEPING a copy is what stops the answer depending on timing. Without it,
///   the pre-emit consumes the only copy and every later emit in the same
///   teardown falls back to a system actor. One *Switch to new version* then
///   produces two verdicts, decided by when a thread became in-flight.
///
/// A teardown that promises an auto-resume also sets `handover`. The agents'
/// processes then outlive it, for the sessions that resume them (ADR 0478).
///
/// A free function over the slots, like [`stash_first_restart_actor`] above,
/// so the rule is testable without standing up an engine.
fn open_teardown(
    restart_slot: &mut Option<crate::engine::thread_events::MessageOrigin>,
    teardown_slot: &mut Option<crate::engine::thread_events::MessageOrigin>,
    handover: &crate::runtime::agent_run_marker::TeardownHandover,
) -> Option<crate::engine::thread_events::MessageOrigin> {
    let actor = restart_slot.take();
    *teardown_slot = actor.clone();
    if crate::engine::thread_events::AbortCause::EngineShutdown.promises_auto_resume(actor.as_ref())
    {
        handover.hand_over();
    }
    actor
}

impl LucidosEngine {
    /// Stash the device actor at restart-request time. The graceful-shutdown
    /// boundary emit runs in the signal handler with no HTTP context. Without
    /// the stash it cannot attribute the restart to "You", and recovery cannot
    /// auto-resume in-flight threads. Returns whether this call stashed.
    ///
    /// Two callers, one per way a user can ask this engine to go down: the
    /// in-workspace *Switch to new version* handler (`/api/v1/restart`), and
    /// the gateway's restart-intent notify (`/api/v1/internal/restart-intent`),
    /// which fires just before the picker's Restart or Stop signals the process.
    /// Both go through [`stash_first_restart_actor`].
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
    /// A device actor is half the switch fingerprint
    /// (`crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL`). A thread that reaches a
    /// later emit with a system actor therefore loses the `paused` verdict, the
    /// withheld Continue button, and the auto-resume. See
    /// `docs/plans/2026-08-07-teardown-actor-is-one-value-for-the-whole-teardown.md`.
    ///
    /// Still spends the stash rather than peeking at it, so the invariant
    /// `take_restart_actor` documents holds unchanged: a stash is consumed by the
    /// teardown it belongs to. See [`open_teardown`] for both halves of that.
    pub fn begin_teardown(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.mark_shutting_down();
        // A teardown abort can complete a child. Its parent's turn would open on
        // an engine that is leaving, so the wake waits for the next boot's refire.
        self.event_bus.hold_parent_wakes();
        open_teardown(
            &mut self.restart_actor.lock().unwrap(),
            &mut self.teardown_actor.lock().unwrap(),
            &self.teardown_handover,
        )
    }

    /// The actor of the teardown under way, for an `EngineShutdown` abort that
    /// runs after the boundary pre-emit. `None` outside a teardown, and `None`
    /// for one nobody requested (bare `stop.sh`, an external SIGUSR1, ctrl-c).
    /// Callers fall back to `MessageOrigin::system()` for both.
    pub fn teardown_actor(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.teardown_actor.lock().unwrap().clone()
    }

    /// Enqueue a thread for auto-resume after a user-initiated switch (recovery).
    /// Idempotent: two recovery sources can name one thread, and a second
    /// request would double-spawn it.
    pub fn enqueue_switch_resume(&self, thread_id: uuid::Uuid) {
        let mut queue = self.pending_switch_resumes.lock().unwrap();
        if !queue.contains(&thread_id) {
            queue.push(thread_id);
        }
    }

    /// Whether recovery queued this thread for auto-resume. Only answers before
    /// [`resume_pending_switches`](Self::resume_pending_switches) drains the
    /// queue, so a caller must run between agent recovery and that drain.
    pub(crate) fn switch_resume_queued(&self, thread_id: uuid::Uuid) -> bool {
        self.pending_switch_resumes
            .lock()
            .unwrap()
            .contains(&thread_id)
    }

    /// Emit `ContinuationRequested` for every thread recovery queued for
    /// auto-resume after a user-initiated switch, so the spawn dispatcher boots
    /// a `--resume`. Each request waits on the bus's directed channel until the
    /// dispatcher drains it. Engine-attributed (`actor: None`), because the
    /// resume is a recovery consequence, not a device click.
    ///
    /// Returns the thread ids whose resume this boot has taken responsibility
    /// for, which `settle_unresumed_switch_threads` must EXCLUDE. The dispatcher
    /// has not emitted `ContinuationStarted` yet when the floor runs, and
    /// `ContinuationRequested` is deliberately absent from
    /// `THREAD_START_EVENTS_SQL`, so a query-only exclusion would re-abort a
    /// thread that is resuming correctly. The skip branch below still counts as
    /// actuated: the dispatcher's startup orphan re-dispatch owns that resume.
    pub async fn resume_pending_switches(&self) -> Vec<uuid::Uuid> {
        let ids = std::mem::take(&mut *self.pending_switch_resumes.lock().unwrap());
        let mut actuated = Vec::with_capacity(ids.len());
        for thread_id in ids {
            // A prior boot may have left this thread an unactuated
            // ContinuationRequested: emitted but never spawned. The
            // dispatcher's startup orphan re-dispatch drives that existing
            // request. Emitting a second one here would put two request event
            // ids on one thread, past the per-EVENT idempotency guard, and
            // double-spawn.
            if crate::engine::continuation_queries::thread_has_unactuated_continuation(
                self.pool(),
                thread_id,
            )
            .await
            {
                log_legacy!(
                    "[Recovery] thread {} already has an unactuated ContinuationRequested: \
                     the dispatcher's startup orphan re-dispatch owns the resume; skipping duplicate emit",
                    thread_id
                );
                actuated.push(thread_id);
                continue;
            }
            // Only a request that actually PERSISTED counts as actuated. An
            // emit that errored leaves nothing for the dispatcher to act on, so
            // the thread must stay visible to
            // `settle_unresumed_switch_threads`, which withdraws the promise and
            // gives the user their Continue button back.
            let requested = crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::continuation_reason::AUTO_RESUME_AFTER_SWITCH_REASON,
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

    /// [`newer_version_visible`] for this engine.
    pub(crate) fn newer_version_visible(&self) -> bool {
        newer_version_visible(self.version_tracking, crate::runtime::is_packaged())
    }

    pub fn register_engine_build_runner(&mut self, runner: impl EngineBuildRunner + 'static) {
        self.engine_build_runner = Box::new(runner);
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
        let exe = published_engine_exe()?;
        let mtime = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
        {
            let cache = self.update_check.lock().unwrap();
            if cache.last_mtime == mtime {
                return cache.disk_build_id.clone();
            }
        }
        let Some(disk_id) = uncached_engine_disk_build_id().await else {
            // Observation cache may retain its last value; delivery proof never does.
            return self.update_check.lock().unwrap().disk_build_id.clone();
        };
        let disk_id = Some(disk_id);
        let mut cache = self.update_check.lock().unwrap();
        cache.last_mtime = mtime;
        cache.disk_build_id = disk_id.clone();
        disk_id
    }

    /// Is the on-disk binary a genuine UPGRADE target for this running engine,
    /// so that switching onto it is a step FORWARD?
    ///
    /// The naive test is `disk_build_id != ENGINE_BUILD_ID`, but *different* is
    /// not *newer*. Co-located dev workspaces share one checkout and one
    /// published launch binary (ADR 0022 and 0063). The binary on disk is
    /// therefore written routinely by something other than this workspace. When
    /// what lands is OLDER, the difference test announces a **downgrade** as a
    /// new version. Switching then leaves the engine behind HEAD, so self-heal
    /// rebuilds and the pair ping-pongs forever.
    ///
    /// Direction is therefore decided by git ancestry over the two ids' commit
    /// prefixes. A disk commit that is a STRICT ANCESTOR of the running commit
    /// is provably older and vetoes the update. Everything indeterminate falls
    /// back to the difference test. That removes a false positive without adding
    /// a way to MISS a real update, which is the worse failure.
    ///
    /// The ancestry answer is memoized per on-disk build id, in
    /// [`DiskDirectionCache`], and only consulted when the ids differ. So
    /// `git merge-base` runs at most once per distinct on-disk binary.
    pub async fn disk_binary_is_upgrade(&self) -> bool {
        let disk = self.engine_disk_build_id().await;
        self.disk_id_is_upgrade(disk.as_deref()).await
    }

    /// [`Self::disk_binary_is_upgrade`] for an on-disk id the caller already
    /// read. `version_status` then reports the id and the verdict from ONE
    /// read, so they cannot straddle an mtime change and disagree.
    async fn disk_id_is_upgrade(&self, disk: Option<&str>) -> bool {
        if !self.newer_version_visible() {
            return false;
        }
        let Some(disk) = disk else {
            return false; // packaged or unreadable: nothing to switch onto
        };
        if disk == crate::build_identity().id {
            return false; // identical build, no git needed
        }
        disk_upgrade_verdict(
            Some(disk),
            crate::build_identity().id,
            self.disk_is_older(disk).await,
        )
    }

    /// Cached `git merge-base --is-ancestor <disk-commit> <running-commit>`.
    /// `Some(true)` means the on-disk binary is provably OLDER, and `None` means
    /// git could not tell. Logs the downgrade case once per distinct on-disk
    /// build id, so a downgrade is diagnosable from the engine log.
    async fn disk_is_older(&self, disk_id: &str) -> Option<bool> {
        {
            let cache = self.disk_direction_cache.lock().unwrap();
            if cache.disk_id.as_deref() == Some(disk_id) {
                return cache.is_strict_ancestor;
            }
        }
        let verdict = match (
            build_id_commit(disk_id),
            build_id_commit(crate::build_identity().id),
        ) {
            (Some(disk_commit), Some(running_commit)) if disk_commit != running_commit => {
                match crate::paths::repo_root() {
                    Ok(root) => commit_is_strict_ancestor(&root, disk_commit, running_commit).await,
                    Err(_) => None,
                }
            }
            // Same commit, differing only in the uncommitted-diff suffix. A
            // rebuilt dirty tree is a real update, not an older commit.
            (Some(_), Some(_)) => Some(false),
            // A `src-…` or empty id on either side: no commit to compare.
            _ => None,
        };
        if verdict == Some(true) {
            crate::log_legacy!(
                "[Rebuild] on-disk engine binary ({}) is OLDER than the running engine ({}): \
                 not offering a downgrade as a new version. Something rebuilt an earlier checkout \
                 state over target/debug; `web-dev.sh -w <ws> -b` rebuilds it forward.",
                disk_id,
                crate::build_identity().id
            );
        }
        let mut cache = self.disk_direction_cache.lock().unwrap();
        cache.disk_id = Some(disk_id.to_string());
        cache.is_strict_ancestor = verdict;
        verdict
    }

    /// Whether the engine SOURCE is behind HEAD by a restart-requiring change.
    /// A NEW engine version then exists in source even with no fresh binary on
    /// disk. Reuses [`Self::engine_source_matches_head`], the SAME git
    /// classifier the frontend-only-Apply veto uses, so this signal and that
    /// veto agree by construction. TTL-cached by [`SOURCE_BEHIND_TTL`].
    /// Dev-only: false packaged.
    ///
    /// Direction-guarded for the same reason [`Self::disk_binary_is_upgrade`]
    /// is. `engine_source_matches_head` compares the two trees with a two-dot
    /// `git diff <running-commit> HEAD`, which is symmetric: it reports a
    /// difference whether HEAD is ahead of the running engine or behind it. An
    /// engine running a commit that HEAD is an ancestor of is not behind
    /// anything. Claiming otherwise pins a permanent pending-version toast and a
    /// self-heal build storm on a workspace that is already current.
    ///
    /// Always false for a pinned engine, which is what keeps self-heal from
    /// rebuilding it: see [`VersionTracking::PinnedToBuild`].
    pub async fn source_behind_head(&self) -> bool {
        if !self.newer_version_visible() {
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
        // `Some(false)` means a restart-requiring change is pending between the
        // running engine's commit and HEAD. `Some(true)` (frontend-only) and
        // `None` (git unavailable) both read as not behind.
        let started = Instant::now();
        let behind = self.engine_source_matches_head().await == Some(false)
            && !self.running_is_ahead_of_head().await;
        let mut cache = self.source_behind_cache.lock().unwrap();
        if observation_is_newest(cache.checked_at, started) {
            cache.checked_at = Some(started);
            cache.behind = behind;
        }
        behind
    }

    /// The checkout's HEAD sha, TTL-cached ([`SOURCE_BEHIND_TTL`]), or `None`
    /// when git could not say.
    ///
    /// The version-status response and self-heal driver's per-HEAD budget
    /// share this cache. Uncached, that is a
    /// `git rev-parse` per caller per version-status read, on a
    /// question whose answer only changes when someone commits. Dev-only: no
    /// caller reaches it packaged.
    pub(crate) async fn head_sha(&self) -> Option<String> {
        {
            let cache = self.head_sha_cache.lock().unwrap();
            if cache
                .checked_at
                .is_some_and(|at| at.elapsed() < SOURCE_BEHIND_TTL)
            {
                return cache.sha.clone();
            }
        }
        let started = Instant::now();
        let sha = current_head_sha().await;
        let mut cache = self.head_sha_cache.lock().unwrap();
        if observation_is_newest(cache.checked_at, started) {
            cache.checked_at = Some(started);
            cache.sha.clone_from(&sha);
        }
        sha
    }

    /// Is the running engine's commit a strict DESCENDANT of HEAD, so that this
    /// engine is ahead of the checkout rather than behind it? The direction
    /// guard for [`Self::source_behind_head`]. `false` whenever git cannot tell,
    /// so an indeterminate answer changes nothing.
    async fn running_is_ahead_of_head(&self) -> bool {
        let Some(running_commit) = build_id_commit(crate::build_identity().id) else {
            return false; // `src-…` or unstamped id: no commit to compare
        };
        let Ok(root) = crate::paths::repo_root() else {
            return false;
        };
        let Some(head) = self.head_sha().await else {
            return false;
        };
        commit_is_strict_ancestor(&root, &head, running_commit).await == Some(true)
    }

    /// Full version status for `GET /api/v1/engine/version-status`. A newer
    /// engine is available when the on-disk binary is readable, differs from the
    /// running one, and is not provably OLDER than it. See
    /// [`Self::disk_binary_is_upgrade`], which is always false packaged.
    pub async fn version_status(&self) -> VersionStatus {
        let disk_build_id = self.engine_disk_build_id().await;
        let update_available = self.disk_id_is_upgrade(disk_build_id.as_deref()).await;
        let source_behind_head = self.source_behind_head().await;
        // Fail-OPEN-to-false probe: an indeterminate answer must not read as "a
        // build is running", which would hide the Rebuild escape hatch.
        let shared_build_in_progress = !crate::runtime::is_packaged()
            && self.engine_build_runner.shared_engine_build_lock_held();
        let build_state = self.build_state();
        let pending_commits = if wants_pending_commits(
            build_state.elapsed().is_some(),
            shared_build_in_progress,
            source_behind_head,
            update_available,
        ) {
            self.pending_commits().await
        } else {
            None
        };
        // The HEAD rides the SAME gate, for the same reason. It only ever
        // answers "which pending version is this?", so a workspace with nothing
        // pending has no question to answer.
        let head_commit = if source_behind_head {
            self.head_sha().await
        } else {
            None
        };
        let build_deferred = if let Some(deferred) = build_state.deferred() {
            match super::build_deferral::active_coding_threads(self.pool()).await {
                Ok(active) => Some(DeferredBuild {
                    coding_threads: active.count,
                    threads: active.threads,
                    deferred_ms: u64::try_from(deferred.since.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                }),
                Err(error) => {
                    super::build_deferral::log_unreadable_once(&error);
                    None
                }
            }
        } else {
            None
        };
        VersionStatus {
            build_id: crate::build_identity().id.to_string(),
            update_available,
            disk_build_id,
            packaged: crate::runtime::is_packaged(),
            build_state: build_state.as_wire(),
            source_behind_head,
            head_commit,
            build_failure: build_state.failure().cloned(),
            shared_build_in_progress,
            build_elapsed_ms: build_state
                .elapsed()
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            build_queued: self.build_queued(),
            build_deferred,
            pending_commits,
            frontend_build_watch_stopped: self.frontend_build_watch_stopped.lock().unwrap().clone(),
            frontend_refresh_elapsed_ms: self
                .frontend_refresh_elapsed()
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        }
    }

    /// Whether this engine's own rebuild is waiting for a build slot. Reads
    /// the pool only while a build of ours runs, so an idle engine probes
    /// nothing. An unreadable pool is not a queue.
    ///
    /// The build also runs steps that hold no slot, such as publishing and the
    /// SDK build. A priority waiter must be up, or those would read as queued.
    fn build_queued(&self) -> Option<QueuedBuild> {
        if crate::runtime::is_packaged() {
            return None;
        }
        let group = self.build_process_group.load(Ordering::SeqCst);
        if group == 0 {
            return None;
        }
        let pool = lucidos_build_slot::BuildSlotPool::open().ok()?;
        if !pool.priority_waiting() {
            return None;
        }
        queued_behind(
            Some(group),
            &pool.status(),
            crate::runtime::spawn_env::process_group_of,
        )
    }

    /// The [`PendingCommits`] a switch would bring, or `None` when git could
    /// not answer. TTL-cached
    /// ([`PENDING_COMMITS_TTL`]) so the `git log` runs at most once per interval
    /// across all status readers.
    async fn pending_commits(&self) -> Option<PendingCommits> {
        let claimed = Instant::now();
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
            // of starting a second `git log`. Stamping only on completion leaves
            // the TTL bounding nothing under concurrency: every client arriving
            // mid-read sees the same expired timestamp and forks its own. The
            // cost is one stale generation for the length of a single `git log`.
            cache.checked_at = Some(claimed);
        }
        let commits = self.read_pending_commits().await;
        self.pending_commits_cache
            .lock()
            .unwrap()
            .record(claimed, commits.clone());
        commits
    }

    /// Uncached read behind [`Self::pending_commits`]. `None` at every step
    /// that cannot produce a trustworthy answer: packaged, a `src-…` build id
    /// with no commit to range from, no resolvable checkout, or a failed
    /// `git log`.
    async fn read_pending_commits(&self) -> Option<PendingCommits> {
        if crate::runtime::is_packaged() {
            return None;
        }
        let running = build_id_commit(crate::build_identity().id)?;
        let root = crate::paths::repo_root().ok()?;
        let served = self.served_frontend_source_commit();
        pending_commits_since(running, served.as_deref(), &root).await
    }

    /// One periodic self-heal tick (dev only). Retriggers a background rebuild
    /// when the engine SOURCE is behind HEAD with no fresh binary on disk. The
    /// shared binary then advances and the Switch surfaces, with no manual
    /// `web-dev.sh -b`.
    ///
    /// Coordinated and bounded:
    /// - Skips when a co-located engine is already building (shared-lock probe),
    ///   so the N workspaces do not stampede the shared `target/`.
    /// - Skips when a genuine UPGRADE is already on disk
    ///   ([`Self::disk_binary_is_upgrade`]), since switching is next rather than
    ///   rebuilding. Deliberately the SAME question `update_available` asks, so
    ///   the two can never disagree. A bare `disk != running` here would read an
    ///   OLDER binary as fresh and suppress the rebuild that would fix it.
    /// - Bounded to [`SELF_HEAL_MAX_ATTEMPTS_PER_HEAD`] per HEAD, so a broken
    ///   `main` cannot spin builds forever. The count resets when HEAD moves.
    /// - Stops after a named failure for this HEAD; explicit Retry stays available.
    ///
    /// No-op packaged, in manual build mode, or when git is unavailable. Driven by the dev periodic
    /// loop in `frontend_tasks`.
    pub async fn self_heal_engine_version_if_needed(self: &Arc<Self>) {
        if crate::runtime::is_packaged() || !self.automatic_source_builds_enabled().await {
            return;
        }
        if !self.source_behind_head().await {
            *self.self_heal_state.lock().unwrap() = SelfHealState::default();
            return;
        }
        // `matches!` rather than `==`: `Building` carries its start instant, so
        // two in-flight builds are never equal to each other.
        if matches!(
            self.build_state(),
            BuildState::Building { .. } | BuildState::Verifying | BuildState::Deferred(_)
        ) {
            return;
        }
        // An unreadable disk id reads as "no upgrade" and falls through to the
        // rebuild, which is the safe direction: at worst we rebuild a binary
        // that was already fine, and the attempt cap bounds it.
        if self.disk_binary_is_upgrade().await {
            return;
        }
        // Share one admission result with the request: a hold spends no attempt.
        let deferral = super::build_deferral::automatic_build_deferral(self.pool()).await;
        let head = self.head_sha().await;
        // Read the build state at DECISION time, not before the awaits above. A
        // rebuild can start or finish during them, and judging "did my rebuild
        // succeed?" from a stale value gives up on a live build.
        let build_state = self.build_state();
        {
            // Racy probe; the in-build lock in `run_engine_build` is the real
            // guard. Synchronous, so no await is held across the state lock.
            if self
                .engine_build_runner
                .engine_build_in_progress_elsewhere()
            {
                return;
            }
            if !self
                .self_heal_state
                .lock()
                .unwrap()
                .request(head, &build_state, &deferral)
            {
                return;
            }
        }
        crate::log_legacy!(
            "[Rebuild] self-heal: engine source is behind HEAD with a stale binary, triggering a background rebuild"
        );
        super::background_build::automatic_with_deferral(self, deferral).await;
    }

    /// Kick off a background engine rebuild (dev only), the non-disruptive half
    /// of the *Switch to new version* flow. The running engine keeps serving.
    /// When the rebuild finishes, the on-disk build id differs from the running
    /// `ENGINE_BUILD_ID` and `version_status` surfaces the switch.
    ///
    /// For an explicit Build now request. A build in flight is **joined**, never
    /// aborted: it builds once more if HEAD moved past it (ADR 0412). No-op
    /// packaged.
    pub fn join_or_start_background_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        crate::engine::background_build::join_or_start(self);
    }

    pub(crate) async fn automatic_background_rebuild(
        self: &Arc<Self>,
    ) -> super::build_deferral::BuildDeferral {
        if crate::runtime::is_packaged() {
            return super::build_deferral::BuildDeferral::Admit;
        }
        super::background_build::automatic(self).await
    }

    pub async fn start_deferred_build_if_admitted(self: &Arc<Self>) {
        super::background_build::start_deferred_build_if_admitted(self).await;
    }

    /// The explicit Rebuild (`POST /api/v1/engine/rebuild`): abort the build in
    /// flight, killing its whole process group, and start over. The user's
    /// escape from a hung build. No-op packaged.
    pub fn restart_background_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        crate::engine::background_build::restart(self);
    }

    /// Record the running executable and the router's initial frontend once
    /// per process boot, before any peer refresh can replace that snapshot.
    pub async fn record_engine_version_activated(&self) {
        let identity = crate::build_identity();
        let frontend_source_commit = self.activated_frontend_commit.lock().unwrap().clone();
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::EngineVersionActivated {
                    build_id: identity.id.into(),
                    engine_version: identity.engine_version.into(),
                    release: identity.release.into(),
                    release_dirty: identity.release_dirty,
                    frontend_source_commit,
                }),
                "[Engine] EngineVersionActivated",
            )
            .await;
    }

    /// Emit the transient `EngineBuildStateChanged` UI poke. A connected client
    /// then learns of a background-rebuild transition over the live SSE stream.
    /// The frontend handler re-runs the
    /// authoritative `checkEngineVersion` GET, so this is a pure nudge and
    /// `state` is informational.
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

/// Injectable build and delivery observations; production and regressions use
/// the same bounded loop rather than testing only the retry counter.
#[async_trait::async_trait]
trait DeliveryHost: BuildHost {
    async fn head(&self) -> Option<String>;
    async fn build(&self, previous_failure: Option<&str>, fresh: bool) -> EngineBuildOutcome;
    async fn delivered(&self, head: Option<&str>) -> Result<bool, BuildFailure>;
    /// Whether the build published a binary of `start` or a later snapshot.
    /// Looser than [`DeliveryHost::delivered`], so it only picks between a
    /// follow-up build and the fresh retry, and never makes a build `Ready`.
    async fn published_since(&self, start: Option<&str>) -> Result<bool, BuildFailure>;
}

fn delivery_probe_failure(cause: &str) -> BuildFailure {
    BuildFailure::plain(format!(
        "Could not verify engine delivery: {cause}. Retry build."
    ))
}

async fn publish_build_progress(host: &impl BuildHost, generation: u64, state: BuildState) {
    if super::background_build::with_current_build(host, generation, || {
        host.set_build_state(state.clone())
    })
    .is_some()
    {
        host.emit_build_state_changed(&state).await;
    }
}

async fn build_and_verify(
    host: &impl DeliveryHost,
    previous_failure: Option<&str>,
    generation: u64,
) -> FinishedBuild {
    let mut retry = DeliveryRetry::default();
    let mut fresh = false;
    // Bound HEAD churn too: a busy checkout must eventually yield a named
    // failure rather than keep this task rebuilding forever.
    for _ in 0..6 {
        let mut built_head = host.head().await;
        if fresh && built_head.is_none() {
            return FinishedBuild {
                state: BuildState::failed_with(delivery_probe_failure("cannot read checkout HEAD")),
                built_head,
            };
        }
        if fresh && !retry.fresh_for(built_head.as_deref()) {
            // Charge the HEAD actually built, since a checkout may advance or
            // return to an earlier HEAD after the retry was scheduled.
            return FinishedBuild {
                state: BuildState::failed_with(BuildFailure::delivery(
                    BuildFailureKind::NotDelivered,
                    "This HEAD already used its fresh build retry. Retry build to start a new attempt.",
                )),
                built_head,
            };
        }
        let outcome = host.build(previous_failure, fresh).await;
        let state = match outcome {
            EngineBuildOutcome::Succeeded => {
                publish_build_progress(host, generation, BuildState::Verifying).await;
                let build_head = built_head.clone();
                let head = host.head().await;
                let head_moved = head.is_some() && head != built_head;
                built_head = head.clone();
                let delivered = match host.delivered(head.as_deref()).await {
                    Ok(delivered) => delivered,
                    Err(reason) => {
                        return FinishedBuild {
                            state: BuildState::failed_with(reason),
                            built_head,
                        }
                    }
                };
                let earlier_delivered = if !delivered && head_moved && build_head.is_some() {
                    match host.published_since(build_head.as_deref()).await {
                        Ok(delivered) => delivered,
                        Err(reason) => {
                            return FinishedBuild {
                                state: BuildState::failed_with(reason),
                                built_head,
                            }
                        }
                    }
                } else {
                    false
                };
                if delivered {
                    BuildState::ready_from(head)
                } else if earlier_delivered {
                    fresh = false;
                    publish_build_progress(host, generation, BuildState::building_now()).await;
                    continue;
                } else if retry.can_fresh(head.as_deref()) {
                    fresh = true;
                    publish_build_progress(host, generation, BuildState::building_now()).await;
                    continue;
                } else {
                    BuildState::failed_with(BuildFailure::delivery(
                            BuildFailureKind::NotDelivered,
                            "The build did not publish a new executable covering HEAD after a fresh retry.",
                        ))
                }
            }
            EngineBuildOutcome::Failed(reason) => BuildState::failed_with(reason),
            EngineBuildOutcome::SkippedLocked => BuildState::Idle,
        };
        return FinishedBuild { state, built_head };
    }
    FinishedBuild {
        state: BuildState::failed_with(BuildFailure::plain(
            "HEAD kept moving during the build. Retry build when the checkout settles.".into(),
        )),
        built_head: host.head().await,
    }
}

async fn run_recorded_build(
    host: &impl DeliveryHost,
    bus: &EventBus,
    previous_failure: Option<&str>,
    generation: u64,
) -> FinishedBuild {
    let started = Instant::now();
    let finished = build_and_verify(host, previous_failure, generation).await;
    let outcome = match &finished.state {
        BuildState::Ready { .. } => "succeeded",
        BuildState::Failed { .. } => "failed",
        BuildState::Idle => "skipped-locked",
        _ => unreachable!("build_and_verify returns a terminal state"),
    };
    let event = SystemEvent::EngineBuildCompleted {
        outcome: outcome.into(),
        duration_ms: started.elapsed().as_millis() as u64,
        built_head: match finished.state {
            BuildState::Idle => None,
            _ => finished.built_head.clone(),
        },
    };
    // A manual restart can abort the build task at an await. Once verification
    // finishes, the event write must survive cancellation of that task.
    let bus = bus.clone();
    let _ = tokio::spawn(async move {
        bus.emit_or_log(BusEvent::System(event), "[Rebuild] EngineBuildCompleted")
            .await;
    })
    .await;
    finished
}

#[async_trait::async_trait]
impl DeliveryHost for LucidosEngine {
    async fn head(&self) -> Option<String> {
        current_head_sha().await
    }
    async fn build(&self, previous_failure: Option<&str>, fresh: bool) -> EngineBuildOutcome {
        self.engine_build_runner
            .run(
                self.workspace_path(),
                previous_failure,
                &self.build_process_group,
                fresh,
            )
            .await
    }
    async fn delivered(&self, head: Option<&str>) -> Result<bool, BuildFailure> {
        let head = head.ok_or_else(|| delivery_probe_failure("cannot read checkout HEAD"))?;
        let Some(commit) = newly_published_commit().await? else {
            return Ok(false);
        };
        let root = source_checkout()?;
        build_covers_commit(&commit, head, &root)
            .await
            .ok_or_else(|| delivery_probe_failure("source coverage probe failed"))
    }
    async fn published_since(&self, start: Option<&str>) -> Result<bool, BuildFailure> {
        let start = start.ok_or_else(|| delivery_probe_failure("cannot read checkout HEAD"))?;
        let Some(commit) = newly_published_commit().await? else {
            return Ok(false);
        };
        let root = source_checkout()?;
        snapshot_since(&commit, start, &root)
            .await
            .ok_or_else(|| delivery_probe_failure("source coverage probe failed"))
    }
}

/// The commit the published binary was built from, or `None` when the build
/// published nothing new: the running binary's id, or an unstamped one.
async fn newly_published_commit() -> Result<Option<String>, BuildFailure> {
    let disk = uncached_engine_disk_build_id()
        .await
        .ok_or_else(|| delivery_probe_failure("published executable build-ID probe failed"))?;
    if disk == crate::build_identity().id {
        return Ok(None);
    }
    Ok(build_id_commit(&disk).map(str::to_string))
}

fn source_checkout() -> Result<std::path::PathBuf, BuildFailure> {
    crate::paths::repo_root().map_err(|_| delivery_probe_failure("cannot resolve source checkout"))
}

/// Whether a binary built from `commit` serves `head`: the same commit, or no
/// restart-requiring file between them. `None` when git could not answer.
async fn build_covers_commit(commit: &str, head: &str, root: &std::path::Path) -> Option<bool> {
    if head.starts_with(commit) {
        return Some(true);
    }
    no_restart_between(commit, head, root).await
}

/// Whether a binary built from `commit` is the snapshot of `start` or a later
/// one. The script snapshots the checkout as cargo starts, so a queued build
/// can publish a descendant of the HEAD the engine read (ADR 0668).
async fn snapshot_since(commit: &str, start: &str, root: &std::path::Path) -> Option<bool> {
    if commit_is_strict_ancestor(root, start, commit).await == Some(true) {
        return Some(true);
    }
    build_covers_commit(commit, start, root).await
}

#[async_trait::async_trait]
impl BuildHost for LucidosEngine {
    async fn deferred_build_needed(&self) -> bool {
        if !self.source_behind_head().await {
            return false;
        }
        let (Some(built), Some(head), Ok(root)) = (
            uncached_engine_disk_build_id()
                .await
                .as_deref()
                .and_then(build_id_commit)
                .map(str::to_string),
            current_head_sha().await,
            crate::paths::repo_root(),
        ) else {
            return true;
        };
        !build_covers_head(&built, &head, &root).await
    }

    fn background_build(&self) -> &BackgroundBuild {
        &self.background_build
    }

    fn build_state(&self) -> BuildState {
        LucidosEngine::build_state(self)
    }

    fn set_build_state(&self, state: BuildState) {
        LucidosEngine::set_build_state(self, state);
    }

    async fn emit_build_state_changed(&self, state: &BuildState) {
        LucidosEngine::emit_build_state_changed(self, state).await;
    }

    async fn automatic_deferral(&self) -> super::build_deferral::BuildDeferral {
        super::build_deferral::automatic_build_deferral(self.pool()).await
    }

    async fn run_build(&self, previous_failure: Option<&str>, generation: u64) -> FinishedBuild {
        let finished =
            run_recorded_build(self, &self.event_bus, previous_failure, generation).await;
        if matches!(finished.state, BuildState::Failed { .. }) {
            let _ = super::background_build::with_current_build(self, generation, || {
                let mut sh = self.self_heal_state.lock().unwrap();
                sh.head = finished.built_head.clone();
                sh.attempts = SELF_HEAL_MAX_ATTEMPTS_PER_HEAD;
            });
        }
        finished
    }

    /// A success is judged by the binary it published, which the build
    /// script's late snapshot may have moved past `built_head`.
    async fn covers_head(&self, finished: &FinishedBuild) -> bool {
        let built = match &finished.state {
            // A peer holds the build lock and builds for us. Self-heal follows
            // that build up, as it did before any join.
            BuildState::Idle => return true,
            BuildState::Ready { .. } => uncached_engine_disk_build_id()
                .await
                .as_deref()
                .and_then(build_id_commit)
                .map(str::to_string),
            _ => finished.built_head.clone(),
        };
        let (Some(built), Some(head), Ok(root)) =
            (built, current_head_sha().await, crate::paths::repo_root())
        else {
            return false;
        };
        build_covers_head(&built, &head, &root).await
    }
}

/// Whether a build of commit `built` already reflects `head`: the same commit,
/// or no restart-requiring file between them. An unknown answer reads as
/// "not covered", since one more build costs less than a binary behind HEAD.
async fn build_covers_head(built: &str, head: &str, root: &std::path::Path) -> bool {
    build_covers_commit(built, head, root).await == Some(true)
}

/// Outcome of a background engine build. `SkippedLocked` is distinct from
/// `Failed`: a co-located engine holds the checkout-shared build lock, so THIS
/// engine deliberately did not build. Not a compile failure, so it must not
/// surface the "build failed" toast.
pub enum EngineBuildOutcome {
    Succeeded,
    /// Carries why, so the toast can say it.
    Failed(BuildFailure),
    SkippedLocked,
}

#[async_trait::async_trait]
pub trait EngineBuildRunner: Send + Sync {
    async fn run(
        &self,
        workspace: &std::path::Path,
        previous_failure: Option<&str>,
        published_group: &AtomicU32,
        fresh: bool,
    ) -> EngineBuildOutcome;

    fn engine_build_in_progress_elsewhere(&self) -> bool;
    fn shared_engine_build_lock_held(&self) -> bool;
}

pub(super) struct UnregisteredEngineBuildRunner;

#[async_trait::async_trait]
impl EngineBuildRunner for UnregisteredEngineBuildRunner {
    async fn run(
        &self,
        _: &std::path::Path,
        _: Option<&str>,
        _: &AtomicU32,
        _: bool,
    ) -> EngineBuildOutcome {
        EngineBuildOutcome::Failed(BuildFailure::plain(
            "No engine build runner registered.".into(),
        ))
    }

    fn engine_build_in_progress_elsewhere(&self) -> bool {
        false
    }

    fn shared_engine_build_lock_held(&self) -> bool {
        false
    }
}

/// The commit prefix of an engine build id, or `None` when there isn't one.
///
/// `build.rs` stamps `<short-sha>` for a clean tree and `<short-sha>-<diffhash>`
/// when engine source is dirty, falling back to `src-<hash>` with no git. Only
/// the commit is comparable across two binaries, so split at the first `-` and
/// reject the no-git and unstamped forms.
pub(crate) fn build_id_commit(id: &str) -> Option<&str> {
    if id.is_empty() || id.starts_with("src") {
        return None;
    }
    let commit = id.split('-').next().unwrap_or("");
    (!commit.is_empty()).then_some(commit)
}

/// Is anyone going to read the pending range this poll? Pure, so the four
/// reasons are one expression rather than a condition inlined in
/// [`LucidosEngine::version_status`].
///
/// The range is read only when something will show it, since an at-rest
/// workspace would otherwise fork a `git log` per poll per connected client.
/// Each term names a surface that describes the range:
///
/// - `build_running`: this engine's own rebuild, narrated by the status toast.
/// - `shared_building`: a co-located peer's, which advances the same binary.
/// - `source_behind`: new code with nothing built behind it, the pending toast.
/// - `update_available`: a built version waiting to be switched onto. This is
///   the one the *new version* confirm reads, and it is not implied by any of
///   the others: a finished rebuild leaves nothing building, and a binary can
///   differ from the running one without the source being behind HEAD.
fn wants_pending_commits(
    build_running: bool,
    shared_building: bool,
    source_behind: bool,
    update_available: bool,
) -> bool {
    build_running || shared_building || source_behind || update_available
}

/// The [`PendingCommits`] a switch from the `running` commit would bring, or
/// `None` when git could not answer.
///
/// Every commit since `running` that the served client lacks. `served` is the
/// commit its snapshot was built from
/// (`LucidosEngine::served_frontend_source_commit`). Excluding both keeps the
/// list inside `running..HEAD` even when `served` is older. With no `served`
/// commit known, or one past an engine change, it lists everything since
/// `running`.
///
/// `--no-merges`: an Apply lands as a merge whose subject is the branch name,
/// and everything it merged is in this same range under its own subject.
async fn pending_commits_since(
    running: &str,
    served: Option<&str>,
    root: &std::path::Path,
) -> Option<PendingCommits> {
    let served = match served {
        Some(commit) => compatible_served_commit(running, commit, root).await,
        None => None,
    };
    let mut args = vec![
        "log".to_string(),
        "--no-merges".to_string(),
        "--format=%s".to_string(),
        "HEAD".to_string(),
        format!("^{running}"),
    ];
    args.extend(served.map(|commit| format!("^{commit}")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    classify_pending_commits(crate::engine::git_ops::git_cmd(&args, root).await)
}

/// `candidate` when a client built from it runs on the `running` engine, else
/// `None`. That is INV-A's git test: no file `files_require_restart` flags
/// differs between the two commits.
///
/// Excluding a served commit only through this check means an engine change
/// is never left out of the list as already served.
async fn compatible_served_commit(
    running: &str,
    candidate: &str,
    root: &std::path::Path,
) -> Option<String> {
    (no_restart_between(running, candidate, root).await == Some(true))
        .then(|| candidate.to_string())
}

/// Whether no file `files_require_restart` flags differs between `from` and
/// `to`. `None` when git could not answer: a failed spawn, the timeout, or a
/// commit this checkout does not know.
pub(crate) async fn no_restart_between(
    from: &str,
    to: &str,
    root: &std::path::Path,
) -> Option<bool> {
    let out = crate::engine::git_ops::git_cmd(&["diff", "--name-only", from, to], root)
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let files: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Some(!crate::engine::git_ops::files_require_restart(&files))
}

/// Whether this binary still serves the engine source at HEAD: no file
/// `files_require_restart` flags differs between its build commit and HEAD.
/// `None` for an unstamped id, no checkout, or a git probe that could not run.
pub(crate) async fn own_source_matches_head() -> Option<bool> {
    let commit = build_id_commit(crate::build_identity().id)?;
    let root = crate::paths::repo_root().ok()?;
    no_restart_between(commit, "HEAD", &root).await
}

/// What `lucidos-engine --build-id --source-state` prints.
///
/// The dev build script reads it after a publish to note when HEAD moved past
/// the snapshot by engine source (`note_head_moved_past`,
/// `scripts/lib/workspace.sh`). It never rebuilds on the answer.
pub async fn own_source_state() -> &'static str {
    source_state_word(own_source_matches_head().await)
}

fn source_state_word(no_restart: Option<bool>) -> &'static str {
    match no_restart {
        Some(true) => "current",
        Some(false) => "stale",
        None => "unknown",
    }
}

/// Classify a [`pending_commits_since`] run into the grouped commit list the
/// status toast shows, keeping "git could not answer" apart from "git answered
/// none".
///
/// `Err` is a spawn failure or the [`GIT_TIMEOUT`](crate::engine::git_ops)
/// ceiling, and a non-zero exit means git refused the range. Neither says
/// anything about what is pending, so both are `None`. Only a successful run
/// yields a verdict, and an empty one is a real `total: 0` with no groups.
/// Reading an unanswerable probe as "nothing is coming" is the failure this
/// split exists to prevent (`.claude/rules/rust.md`).
fn classify_pending_commits(
    result: Result<std::process::Output, String>,
) -> Option<PendingCommits> {
    match result {
        Ok(out) if out.status.success() => Some(group_commit_subjects(
            String::from_utf8_lossy(&out.stdout).lines(),
        )),
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

/// Group commit subjects (newest first) into the list the status toast shows.
/// Pure so the taxonomy, the per-group cap, the counts and the ordering are
/// testable without a repository.
///
/// Blank subjects are dropped: they would render as an empty bullet, and they
/// would inflate the count of what the user is waiting for. Empty groups are
/// omitted, so no heading is ever rendered over nothing.
fn group_commit_subjects<'a>(subjects: impl IntoIterator<Item = &'a str>) -> PendingCommits {
    let mut totals = [0usize; CommitGroupKind::COUNT];
    let mut descriptions: [Vec<String>; CommitGroupKind::COUNT] = Default::default();
    for subject in subjects
        .into_iter()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
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

/// Is the on-disk binary a genuine upgrade, given its id, the running id, and
/// the ancestry answer? `Some(true)` means the disk commit is provably older,
/// and `None` means git could not tell.
///
/// An upgrade is a readable, DIFFERENT id that is not provably older.
/// Indeterminate ancestry keeps the plain difference test, because stranding
/// the user on an old engine with no Switch is the worse failure.
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

/// Is `ancestor` a STRICT ancestor of `descendant`? That is `git merge-base
/// --is-ancestor` plus the two being different commits. `None` when git cannot
/// answer, so callers can tell "provably older" from "don't know".
///
/// The gateway carries a hand-synced copy of this, and of [`build_id_commit`]
/// and [`disk_upgrade_verdict`], in `crates/lucidos-gateway/src/build_id.rs`.
/// ADR 0014 keeps that crate free of any dependency on the engine, so **keep
/// the two in step**.
///
/// Takes `root` rather than resolving it internally so the tests can drive it
/// against a throwaway repo.
///
/// STRICT is enforced here, not by git: `git merge-base --is-ancestor X X`
/// exits 0, so the same commit must be screened out first. The screen is
/// prefix-aware because build ids carry git's SHORT sha while
/// `current_head_sha` returns the full one.
async fn commit_is_strict_ancestor(
    root: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    if ancestor.starts_with(descendant) || descendant.starts_with(ancestor) {
        return Some(false); // same commit, possibly abbreviated, so not OLDER
    }
    let out = crate::engine::git_ops::git_cmd(
        &["merge-base", "--is-ancestor", ancestor, descendant],
        root,
    )
    .await
    .ok()?;
    match out.status.code() {
        Some(0) => Some(true),  // is an ancestor
        Some(1) => Some(false), // is not an ancestor
        // Any other code is an error (bad object, not a repo), not a verdict.
        _ => None,
    }
}

/// The checkout's current HEAD sha (`git rev-parse HEAD`), or `None` when git is
/// unavailable. Keys the self-heal attempt counter so a broken `main` gives up
/// per-HEAD and retries once new work lands.
async fn current_head_sha() -> Option<String> {
    let root = crate::paths::repo_root().ok()?;
    crate::engine::git_ops::current_head_sha(&root).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_older_observation_never_replaces_a_newer_one() {
        let older = std::time::Instant::now();
        let newer = older + std::time::Duration::from_millis(5);
        assert!(super::observation_is_newest(None, older));
        assert!(super::observation_is_newest(Some(older), newer));
        assert!(!super::observation_is_newest(Some(newer), older));
    }

    /// A read that finishes while a later claim is still reading must still
    /// replace an older answer: the watcher announces what it read, and a
    /// client refreshing on that announcement must be served it.
    #[test]
    fn pending_commits_serve_the_newest_finished_read() {
        let first = std::time::Instant::now();
        let second = first + std::time::Duration::from_millis(5);
        let third = second + std::time::Duration::from_millis(5);
        let mut cache = super::PendingCommitsCache::default();
        cache.record(first, None);
        cache.checked_at = Some(third);
        cache.record(second, Some(super::PendingCommits::from_groups(Vec::new())));
        assert!(cache.commits.is_some());
        cache.record(first, None);
        assert!(cache.commits.is_some());
        assert_eq!(cache.checked_at, Some(third));
    }

    #[test]
    fn version_status_wire_keeps_deferred_details_beside_ready() {
        use super::*;
        let deferred = DeferredBuild {
            coding_threads: 2,
            threads: Vec::new(),
            deferred_ms: 840_000,
        };
        for state in ["deferred", "ready"] {
            let status = VersionStatus {
                build_id: "test".into(),
                update_available: state == "ready",
                disk_build_id: None,
                packaged: false,
                build_state: state,
                source_behind_head: true,
                shared_build_in_progress: false,
                head_commit: None,
                build_failure: None,
                build_elapsed_ms: None,
                build_queued: None,
                build_deferred: Some(deferred.clone()),
                pending_commits: None,
                frontend_refresh_elapsed_ms: None,
                frontend_build_watch_stopped: None,
            };
            let wire = serde_json::to_value(&status).unwrap();
            assert_eq!(wire["build_state"], state);
            assert_eq!(wire["build_deferred"]["coding_threads"], 2);
            assert_eq!(wire["build_deferred"]["deferred_ms"], 840_000);
            assert_eq!(wire["build_deferred"]["threads"], serde_json::json!([]));
        }
    }

    #[test]
    fn self_heal_holds_spend_no_attempts_and_cannot_bypass_exhaustion() {
        use super::{BuildState, SelfHealState, SELF_HEAL_MAX_ATTEMPTS_PER_HEAD};
        use crate::engine::build_deferral::{ActiveThreads, BuildDeferral};
        let deferral = BuildDeferral::Defer(ActiveThreads {
            count: 1,
            threads: vec![],
        });
        let mut budget = SelfHealState::default();
        let head = Some("head-a".to_string());
        for _ in 0..100 {
            assert!(budget.request(head.clone(), &BuildState::Idle, &deferral));
        }
        assert_eq!(budget.attempts, 0);
        for _ in 0..SELF_HEAL_MAX_ATTEMPTS_PER_HEAD {
            assert!(budget.request(head.clone(), &BuildState::Idle, &BuildDeferral::Admit));
        }
        assert!(!budget.request(head.clone(), &BuildState::Idle, &deferral));
        assert!(!budget.request(head, &BuildState::Idle, &BuildDeferral::Admit));
        assert!(budget.request(Some("head-b".into()), &BuildState::Idle, &deferral));
        assert_eq!(budget.attempts, 0);
    }

    use super::{
        build_covers_commit, build_id_commit, classify_build_failure, classify_commit_subject,
        classify_pending_commits, commit_is_strict_ancestor, compatible_served_commit,
        disk_upgrade_verdict, group_commit_subjects, newer_version_visible, no_restart_between,
        open_teardown, pending_commits_since, queued_behind, snapshot_since, source_state_word,
        stash_first_restart_actor, unrecognized_build_failure, wants_pending_commits, BuildFailure,
        BuildState, CommitGroupKind, QueuedBuild, VersionTracking, BUILD_FAILURE_SUMMARY_CAP,
        COMMIT_GROUP_ORDER, PENDING_COMMIT_DESCRIPTION_CAP, UNLABELLED_HOLDER,
    };
    use crate::engine::thread_events::MessageOrigin;

    // ── Why a build failed ───────────────────────────────────────────────────

    /// The real output from the incident that produced this code path: a build
    /// script carrying a path into a checkout that had been deleted.
    const STALE_BUILD_SCRIPT_OUTPUT: &str = "\
   Compiling lucidos-engine v0.1.0 (/repo/crates/lucidos-engine)
error: failed to run custom build command for `lucidos-engine v0.1.0 (/repo/crates/lucidos-engine)`

Caused by:
  process didn't exit successfully: `/repo/target/debug/build/lucidos-engine-7d7f/build-script-build` (exit status: 101)
  --- stderr

  thread 'main' (11466974) panicked at crates/lucidos-engine/build.rs:52:45:
  Failed to create default VERSION: Os { code: 2, kind: NotFound, message: \"No such file or directory\" }
  note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
";

    #[test]
    fn the_stale_build_script_failure_is_named_with_its_likely_remedy() {
        // The class this whole change exists for: the panic message is the
        // cause, and cargo's own line only says "custom build command".
        let f = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, None)
            .expect("a build-script panic must classify");
        assert!(
            f.summary.contains("Failed to create default VERSION"),
            "the panic message is the cause, not cargo's generic line: {}",
            f.summary
        );
        assert_eq!(f.remedy.as_deref(), Some("cargo clean -p lucidos-engine"));
    }

    #[test]
    fn a_recognized_shape_alone_never_retires_the_retry_button() {
        // The shape is a GUESS. A build script reporting a missing file may be
        // missing a real input, which a rebuild fixes once it is back, and
        // `cargo clean` does not. So a first sighting keeps Retry.
        let f = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, None).expect("must classify");
        assert!(
            !f.repeatable,
            "a pattern is not proof; only a repeat is: {f:?}"
        );
    }

    #[test]
    fn an_identical_repeat_is_recorded_for_diagnostics() {
        // Record the repeated artifact failure without withholding Retry.
        let first = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, None).expect("must classify");
        let second = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, Some(&first.summary))
            .expect("must classify");
        assert!(second.repeatable);
    }

    #[test]
    fn a_repeating_generic_error_line_never_retires_retry_on_its_own() {
        // A summary is not an identity. Two unrelated type errors both read
        // `error[E0308]: mismatched types`. Fix one, meet the next, and the
        // button would go while the user is making progress. So repetition
        // only counts for a failure we actually recognize.
        let out = "error[E0308]: mismatched types\n --> src/b.rs:9:1\n";
        let f = classify_build_failure(out, Some("error[E0308]: mismatched types"))
            .expect("must classify");
        assert!(
            !f.repeatable,
            "a repeated generic line is not a repeated failure: {f:?}"
        );
    }

    #[test]
    fn a_build_that_fails_differently_re_arms_retry() {
        // Something changed between the two attempts, so the next one is
        // worth taking. A restored input lands here.
        let previous = "error[E0308]: mismatched types";
        let f = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, Some(previous))
            .expect("must classify");
        assert!(!f.repeatable);
    }

    #[test]
    fn an_ordinary_compile_error_keeps_the_retry_button() {
        let f = classify_build_failure(
            "error[E0308]: mismatched types\n --> crates/lucidos-engine/src/foo.rs:12:5\n",
            None,
        )
        .expect("a compile error must classify");
        assert_eq!(f.summary, "error[E0308]: mismatched types");
        assert_eq!(f.remedy, None);
        assert!(!f.repeatable);
    }

    #[test]
    fn output_with_no_error_line_is_not_given_a_cause() {
        // The classifier never invents a cause. The caller describes what it
        // saw instead, through `unrecognized_build_failure`.
        assert!(classify_build_failure("", None).is_none());
        assert!(classify_build_failure("   Compiling lucidos-engine v0.1.0\n", None).is_none());
    }

    #[test]
    fn a_missing_path_outside_a_build_script_gets_no_clean_remedy() {
        // `cargo clean` is not the fix for a source file that genuinely is not
        // there, so the recognizer must not claim it.
        let f = classify_build_failure(
            "error: couldn't read src/gone.rs: No such file or directory (os error 2)\n",
            None,
        )
        .expect("must classify");
        assert_eq!(f.remedy, None);
    }

    #[test]
    fn an_unparseable_package_line_falls_back_to_a_bare_clean() {
        // A wrong `-p` would send the user to clean the wrong package. So an
        // unrecognized line drops the flag rather than guess at a name.
        let out = "error: failed to run custom build command\n\
                   No such file or directory\n";
        let f = classify_build_failure(out, None).expect("must classify");
        assert_eq!(f.remedy.as_deref(), Some("cargo clean"));
    }

    #[test]
    fn a_runaway_error_line_is_capped_without_splitting_a_character() {
        // The summary goes in a toast on a phone. Capping by BYTE index would
        // panic on a multi-byte character straddling the boundary.
        let long = format!("error: {}", "\u{e9}".repeat(400));
        let f = classify_build_failure(&long, None).expect("must classify");
        assert!(f.summary.len() <= BUILD_FAILURE_SUMMARY_CAP);
    }

    /// The real output from the incident that produced the script recognizer: a
    /// background rebuild that died in the port allocator, before cargo. The
    /// workspace path is a placeholder; everything else is verbatim.
    const PORT_ALLOCATOR_OUTPUT: &str = "\
ports.sh: refusing to signal protected host pid 39004
ERROR: pinned port for workspace '/Users/me/workspaces/dev' is occupied: vite 5173 (pid 39004)
       Source: lucidos.toml vite=5173 (vite=5173, vite-internal=3000).
       Free that port or change the pin in /Users/me/workspaces/dev/lucidos.toml.
";

    #[test]
    fn a_build_that_died_before_cargo_still_names_its_cause() {
        // The class this recognizer exists for. The output WAS captured, and
        // its first error line said what went wrong. No cargo diagnostic
        // appeared, so the toast reported the cause as unreadable.
        let f = classify_build_failure(PORT_ALLOCATOR_OUTPUT, None)
            .expect("a script refusal must classify");
        assert!(
            f.summary.starts_with("pinned port for workspace"),
            "the marker is how the script shouts, not part of the cause: {}",
            f.summary
        );
        assert_eq!(f.remedy, None);
        assert!(
            !f.repeatable,
            "no remedy recognized, so Retry stays offered"
        );
    }

    #[test]
    fn a_repeated_script_refusal_still_keeps_the_retry_button() {
        // Same rule as every other unrecognized shape: repetition alone is not
        // proof. Retiring Retry takes a recognized remedy AND an observed
        // repeat, and this recognizer deliberately offers no remedy.
        let first = classify_build_failure(PORT_ALLOCATOR_OUTPUT, None).expect("must classify");
        let second = classify_build_failure(PORT_ALLOCATOR_OUTPUT, Some(&first.summary))
            .expect("must classify");
        assert!(!second.repeatable, "{second:?}");
    }

    #[test]
    fn a_compile_error_beats_a_script_error_line() {
        // Ordering is the point. A script that shouts on its way out while
        // cargo has already said what is wrong must not replace the diagnosis.
        let out = "ERROR: the build slot broker went away\n\
                   error[E0433]: failed to resolve: use of undeclared crate `foo`\n";
        let f = classify_build_failure(out, None).expect("must classify");
        assert_eq!(
            f.summary,
            "error[E0433]: failed to resolve: use of undeclared crate `foo`"
        );
    }

    #[test]
    fn a_build_script_panic_beats_a_script_error_line_too() {
        // The panic outranks both, unchanged by this recognizer.
        let out = format!("ERROR: something the wrapper shouted\n{STALE_BUILD_SCRIPT_OUTPUT}");
        let f = classify_build_failure(&out, None).expect("must classify");
        assert!(
            f.summary.contains("Failed to create default VERSION"),
            "{}",
            f.summary
        );
    }

    #[test]
    fn a_bare_marker_with_nothing_after_it_is_not_a_cause() {
        // "ERROR:" alone says nothing, and a blank summary would render as a
        // toast with an empty cause. Fall through to the line that talks.
        let out = "ERROR:\nERROR:   the real problem\n";
        let f = classify_build_failure(out, None).expect("must classify");
        assert_eq!(f.summary, "the real problem");

        // ...and with no talking line at all, the cause stays unknown.
        assert!(classify_build_failure("ERROR:\nERROR:    \n", None).is_none());
    }

    #[test]
    fn a_marker_mid_line_is_prose_and_is_never_claimed() {
        // The marker is how these scripts shout, at line start. A sentence
        // mentioning one is not a diagnosis.
        assert!(classify_build_failure("the build hit an ERROR: somewhere\n", None).is_none());
    }

    #[test]
    fn a_silent_failure_says_it_printed_nothing() {
        // The incident: exit 1 in under a second, zero bytes. The output WAS
        // read; there was none. Saying it could not be read was the lie.
        let f = unrecognized_build_failure("exit status: 1", None);
        assert_eq!(
            f.summary,
            "the build stopped (exit status: 1) without printing anything"
        );
        assert_eq!(f.remedy, None);
        assert!(!f.repeatable, "an unknown cause never retires Retry");
    }

    #[test]
    fn an_unrecognized_failure_quotes_its_last_line() {
        let f = unrecognized_build_failure("signal: 9 (SIGKILL)", Some("Building engine..."));
        assert_eq!(
            f.summary,
            "the build stopped (signal: 9 (SIGKILL)) after printing \"Building engine...\""
        );
        let long = "\u{e9}".repeat(400);
        let f = unrecognized_build_failure("exit status: 1", Some(&long));
        assert!(f.summary.len() <= BUILD_FAILURE_SUMMARY_CAP);
    }

    #[test]
    fn the_failure_rides_in_the_variant_and_only_the_failed_variant_has_one() {
        // Same argument as `Building` carrying its instant: "failed, but nobody
        // knows why" should not be reachable through the state.
        let f = classify_build_failure(STALE_BUILD_SCRIPT_OUTPUT, None).expect("must classify");
        let state = BuildState::failed_with(f);
        assert_eq!(state.as_wire(), "failed");
        assert!(state.failure().is_some());
        assert!(BuildState::Idle.failure().is_none());
        assert!(BuildState::ready_from(None).failure().is_none());
        assert!(BuildState::building_now().failure().is_none());
    }

    // ── The restart-actor stash ──────────────────────────────────────────────

    fn device(id: &str) -> Option<MessageOrigin> {
        Some(MessageOrigin::Device {
            device_id: id.to_string(),
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
        // The pre-emit is not the only thing that emits an `EngineShutdown`
        // abort during a teardown: `shutdown_active_threads` and
        // `emit_stop_terminal`'s abort arm run after it, for threads that
        // became in-flight after its snapshot. All three must attribute the
        // teardown the same way. A `Device` actor is half the switch
        // fingerprint, so it decides the `paused` verdict and the auto-resume.
        // Handing the only copy to the first reader splits sibling threads
        // between "Paused by restart" and a manual Continue.
        let mut restart = device("the-click");
        let mut teardown = None;

        let returned = open_teardown(&mut restart, &mut teardown, &Default::default());

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

        assert!(open_teardown(&mut restart, &mut teardown, &Default::default()).is_none());
        assert!(teardown.is_none());
    }

    #[test]
    fn only_a_teardown_that_promises_a_resume_hands_the_agents_processes_over() {
        // A Switch auto-resumes its threads, so their agents' processes (an
        // early `/harden` suite run) must outlive it (ADR 0478). Any other
        // teardown ends the work, so ADR 0391's sweep still reaps.
        let api = MessageOrigin::Api {
            user_agent: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        for (actor, hands_over) in [
            (device("the-click"), true),
            (None, false),
            (Some(api), false),
        ] {
            let mut restart = actor.clone();
            let handover = crate::runtime::agent_run_marker::TeardownHandover::default();
            open_teardown(&mut restart, &mut None, &handover);
            assert_eq!(handover.is_handed_over(), hands_over, "actor {actor:?}");
        }
    }

    #[test]
    fn a_non_device_actor_is_still_stashable_by_this_rule() {
        // The device-only requirement is enforced at the HTTP boundary (the
        // restart-intent handler 400s a non-device caller), NOT here: the
        // in-workspace Switch legitimately stashes whatever `user_actor`
        // gave it, and downstream reads the actor's kind for itself. Pinned so a
        // later "tighten the stash" edit has to notice it would change that path.
        let mut slot = None;
        let api = Some(MessageOrigin::Api {
            user_agent: Some("curl".to_string()),
            mode: crate::engine::thread_events::ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        });
        assert!(stash_first_restart_actor(&mut slot, api));
        assert!(slot.is_some());
    }

    #[derive(Default)]
    struct DeliveryStub {
        background: super::BackgroundBuild,
        states: std::sync::Mutex<Vec<BuildState>>,
        fresh: std::sync::Mutex<Vec<bool>>,
        delivered: bool,
        probe_failure: bool,
        skip_locked: bool,
        delivered_head: Option<String>,
        published_since: Option<String>,
        heads: std::sync::Mutex<std::collections::VecDeque<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl super::BuildHost for DeliveryStub {
        async fn deferred_build_needed(&self) -> bool {
            true
        }
        fn background_build(&self) -> &super::BackgroundBuild {
            &self.background
        }
        fn build_state(&self) -> BuildState {
            self.states
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }
        fn set_build_state(&self, state: BuildState) {
            self.states.lock().unwrap().push(state);
        }
        async fn emit_build_state_changed(&self, _: &BuildState) {}
        async fn automatic_deferral(&self) -> crate::engine::build_deferral::BuildDeferral {
            crate::engine::build_deferral::BuildDeferral::Admit
        }
        async fn run_build(&self, previous: Option<&str>, generation: u64) -> super::FinishedBuild {
            super::build_and_verify(self, previous, generation).await
        }
        async fn covers_head(&self, _: &super::FinishedBuild) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl super::DeliveryHost for DeliveryStub {
        async fn head(&self) -> Option<String> {
            self.heads
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Some("head1".into()))
        }
        async fn build(&self, _: Option<&str>, fresh: bool) -> super::EngineBuildOutcome {
            self.fresh.lock().unwrap().push(fresh);
            if self.skip_locked {
                return super::EngineBuildOutcome::SkippedLocked;
            }
            // A real successful process that publishes nothing, the original
            // false-success contract. It must never be enough to report Ready.
            let status = tokio::process::Command::new("sh")
                .args(["-c", "true"])
                .status()
                .await
                .unwrap();
            assert!(status.success());
            super::EngineBuildOutcome::Succeeded
        }
        async fn delivered(&self, head: Option<&str>) -> Result<bool, super::BuildFailure> {
            if self.probe_failure {
                return Err(super::BuildFailure::plain(
                    "Delivery probe unavailable.".into(),
                ));
            }
            Ok(self.delivered
                || self
                    .delivered_head
                    .as_deref()
                    .is_some_and(|covered| head == Some(covered)))
        }
        async fn published_since(&self, start: Option<&str>) -> Result<bool, super::BuildFailure> {
            Ok(self.published_since.is_some() && self.published_since.as_deref() == start)
        }
    }

    #[tokio::test]
    async fn engine_build_fact_records_lock_skip_without_a_head() {
        assert_engine_build_fact(
            &DeliveryStub {
                skip_locked: true,
                ..Default::default()
            },
            "skipped-locked",
            None,
        )
        .await;
    }

    async fn assert_engine_build_fact(
        host: &DeliveryStub,
        outcome: &str,
        built_head: Option<&str>,
    ) -> super::FinishedBuild {
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _) = super::EventBus::new(pool.clone());
        let finished = super::run_recorded_build(host, &bus, None, 0).await;
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload->'data' FROM events WHERE event_type = 'EngineBuildCompleted'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["outcome"], outcome);
        assert_eq!(rows[0]["built_head"], serde_json::json!(built_head));
        assert!(rows[0]["duration_ms"].is_u64());
        drop(bus);
        teardown_test_db(&db).await;
        finished
    }

    #[tokio::test]
    async fn unavailable_delivery_probe_fails_without_cleaning_or_claiming_non_delivery() {
        let host = DeliveryStub {
            probe_failure: true,
            ..Default::default()
        };
        let finished = assert_engine_build_fact(&host, "failed", Some("head1")).await;
        let failure = finished.state.failure().unwrap();
        assert_eq!(failure.kind, None);
        assert!(failure.summary.contains("probe unavailable"));
        assert_eq!(*host.fresh.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn losing_head_before_a_fresh_build_does_not_clean_unknown_source() {
        let host = DeliveryStub::default();
        *host.heads.lock().unwrap() = [Some("head1".into()), Some("head1".into()), None]
            .into_iter()
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        let failure = finished.state.failure().unwrap();
        assert_eq!(failure.kind, None);
        assert!(failure.summary.contains("cannot read checkout HEAD"));
        assert_eq!(*host.fresh.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn successful_script_without_delivery_verifies_retries_once_and_fails() {
        let host = DeliveryStub::default();
        for _ in 0..2 {
            // A manual retry starts a new delivery budget.
            let finished = super::build_and_verify(&host, None, 0).await;
            assert_eq!(
                finished.state.failure().unwrap().kind,
                Some(super::BuildFailureKind::NotDelivered)
            );
            assert_eq!(finished.built_head.as_deref(), Some("head1"));
        }
        assert_eq!(*host.fresh.lock().unwrap(), [false, true, false, true]);
        let states = host.states.lock().unwrap();
        assert_eq!(
            states
                .iter()
                .filter(|s| matches!(s, BuildState::Verifying))
                .count(),
            4
        );
        assert!(!states.iter().any(|s| matches!(s, BuildState::Ready { .. })));
    }

    #[tokio::test]
    async fn head_moving_after_script_success_rebuilds_incrementally_before_cleaning() {
        let host = DeliveryStub {
            published_since: Some("old".into()),
            ..Default::default()
        };
        *host.heads.lock().unwrap() = ["old", "head1", "head1", "head1"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert_eq!(
            finished.state.failure().unwrap().kind,
            Some(super::BuildFailureKind::NotDelivered)
        );
        assert_eq!(*host.fresh.lock().unwrap(), [false, false, true]);
    }

    #[tokio::test]
    async fn late_snapshot_covering_head_is_ready_without_another_compile() {
        let host = DeliveryStub {
            delivered_head: Some("snapshot".into()),
            ..Default::default()
        };
        *host.heads.lock().unwrap() = ["start", "snapshot"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert!(matches!(finished.state, BuildState::Ready { .. }));
        assert_eq!(finished.built_head.as_deref(), Some("snapshot"));
        assert_eq!(*host.fresh.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn late_snapshot_behind_head_gets_one_incremental_follow_up_then_ready() {
        let host = DeliveryStub {
            published_since: Some("start".into()),
            delivered_head: Some("latest".into()),
            ..Default::default()
        };
        *host.heads.lock().unwrap() = ["start", "past-snapshot", "latest", "latest"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert!(matches!(finished.state, BuildState::Ready { .. }));
        assert_eq!(finished.built_head.as_deref(), Some("latest"));
        assert_eq!(*host.fresh.lock().unwrap(), [false, false]);
    }

    #[tokio::test]
    async fn head_moving_during_an_undelivered_build_still_gets_a_fresh_retry() {
        let host = DeliveryStub::default();
        *host.heads.lock().unwrap() = ["old", "head1", "head1", "head1"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert_eq!(
            finished.state.failure().unwrap().kind,
            Some(super::BuildFailureKind::NotDelivered)
        );
        assert_eq!(*host.fresh.lock().unwrap(), [false, true]);
    }

    #[tokio::test]
    async fn verified_delivery_is_ready_without_a_fresh_build() {
        let host = DeliveryStub {
            delivered: true,
            ..Default::default()
        };
        let finished = assert_engine_build_fact(&host, "succeeded", Some("head1")).await;
        assert!(matches!(finished.state, BuildState::Ready { .. }));
        assert_eq!(*host.fresh.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn returning_to_a_previously_cleaned_head_does_not_clean_it_again() {
        let host = DeliveryStub::default();
        *host.heads.lock().unwrap() = ["head1", "head1", "head1", "head2", "head1", "head2"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert_eq!(
            finished.state.failure().unwrap().kind,
            Some(super::BuildFailureKind::NotDelivered)
        );
        assert_eq!(*host.fresh.lock().unwrap(), [false, true]);
    }

    #[tokio::test]
    async fn head_advancing_before_fresh_build_consumes_its_retry_budget() {
        let host = DeliveryStub::default();
        *host.heads.lock().unwrap() = ["old", "old", "head1", "head1"]
            .into_iter()
            .map(|head| Some(head.to_owned()))
            .collect();
        let finished = super::build_and_verify(&host, None, 0).await;
        assert_eq!(
            finished.state.failure().unwrap().kind,
            Some(super::BuildFailureKind::NotDelivered)
        );
        assert_eq!(*host.fresh.lock().unwrap(), [false, true]);
    }

    #[test]
    fn launch_directory_requires_the_published_layout() {
        let published =
            std::path::Path::new("/checkout/.launch/release/real-embedder-tests/lucidos-engine");
        assert_eq!(
            super::published_launch_dir(published),
            published.parent().map(std::path::Path::to_path_buf)
        );
        assert_eq!(
            super::published_launch_dir(std::path::Path::new(
                "/checkout/target/debug/lucidos-engine"
            )),
            None
        );
        assert_eq!(
            super::published_launch_dir(std::path::Path::new(
                "/checkout/.launch/plain/lucidos-engine"
            )),
            None
        );
    }

    #[test]
    fn published_path_recovers_only_linux_deleted_engine_name() {
        let path =
            std::path::PathBuf::from("/checkout/.launch/release/plain/lucidos-engine (deleted)");
        assert_eq!(
            super::published_engine_path(path.clone(), true),
            path.with_file_name("lucidos-engine")
        );
        assert_eq!(super::published_engine_path(path.clone(), false), path);
        let other = path.with_file_name("other (deleted)");
        assert_eq!(super::published_engine_path(other.clone(), true), other);
    }

    #[test]
    fn verification_and_delivery_failure_have_distinct_wire_states() {
        assert_eq!(BuildState::Verifying.as_wire(), "verifying");
        let failure =
            BuildFailure::delivery(super::BuildFailureKind::NotDelivered, "stale executable");
        assert_eq!(
            serde_json::to_value(&failure).unwrap()["kind"],
            "not-delivered"
        );
        assert!(!failure.repeatable);
        assert_eq!(BuildState::failed_with(failure).as_wire(), "failed");
    }

    #[test]
    fn delivery_retry_is_bounded_and_rearms_for_a_new_head() {
        let mut retry = super::DeliveryRetry::default();
        assert!(retry.fresh_for(Some("head1")));
        assert!(!retry.fresh_for(Some("head1")));
        assert!(retry.fresh_for(Some("head2")));
        assert!(!retry.fresh_for(Some("head2")));
        assert!(!retry.fresh_for(Some("head1")));
        // Unknown HEAD must not open an unlimited retry budget.
        assert!(retry.fresh_for(None));
        assert!(!retry.fresh_for(None));
    }

    fn held(pid: u32, label: &str) -> lucidos_build_slot::SlotState {
        lucidos_build_slot::SlotState {
            index: 0,
            holder: Some(lucidos_build_slot::SlotHolder {
                pid: Some(pid),
                label: label.to_string(),
                ..Default::default()
            }),
        }
    }

    fn free() -> lucidos_build_slot::SlotState {
        lucidos_build_slot::SlotState {
            index: 0,
            holder: None,
        }
    }

    /// Pids 1xx are in group 100 (ours), 2xx in group 200.
    fn group_of(pid: u32) -> Option<u32> {
        Some(pid / 100 * 100)
    }

    #[test]
    fn a_build_is_queued_when_every_slot_is_held_by_someone_else() {
        let slots = [
            held(201, "make lint"),
            held(202, ""),
            held(203, "engine tests"),
        ];
        assert_eq!(
            queued_behind(Some(100), &slots, group_of),
            Some(QueuedBuild {
                holders: vec![
                    "make lint".into(),
                    UNLABELLED_HOLDER.into(),
                    "engine tests".into()
                ]
            })
        );
    }

    #[test]
    fn a_build_is_not_queued_while_it_holds_a_slot_or_one_is_free() {
        let ours = [held(201, "make lint"), held(101, "engine build")];
        assert_eq!(
            queued_behind(Some(100), &ours, group_of),
            None,
            "it compiles"
        );
        let room = [held(201, "make lint"), free()];
        assert_eq!(
            queued_behind(Some(100), &room, group_of),
            None,
            "a slot is free"
        );
    }

    #[test]
    fn no_build_of_ours_or_no_pool_is_never_queued() {
        let full = [held(201, "make lint")];
        assert_eq!(queued_behind(None, &full, group_of), None, "idle");
        assert_eq!(queued_behind(Some(100), &[], group_of), None, "no pool");
    }

    #[test]
    fn a_holder_whose_group_cannot_be_read_is_someone_else() {
        let slots = [held(101, "engine build")];
        assert_eq!(
            queued_behind(Some(100), &slots, |_| None),
            Some(QueuedBuild {
                holders: vec!["engine build".into()]
            })
        );
    }

    #[test]
    fn build_id_commit_takes_the_sha_and_rejects_the_no_git_forms() {
        assert_eq!(build_id_commit("aa7075ee2"), Some("aa7075ee2"));
        // Dirty tree, `<sha>-<diffhash>`: only the sha is comparable.
        assert_eq!(
            build_id_commit("aa7075ee2-0badc0ffee123456"),
            Some("aa7075ee2")
        );
        // No git (shipped build) or unstamped: nothing to compare.
        assert_eq!(build_id_commit("src-0123456789abcdef"), None);
        assert_eq!(build_id_commit(""), None);
        assert_eq!(build_id_commit("-abc"), None);
    }

    /// The build script matches these words exactly, so a rename strands it on
    /// the old behaviour of rebuilding after every move of HEAD.
    #[test]
    fn source_state_word_keeps_the_three_words_the_build_script_reads() {
        assert_eq!(source_state_word(Some(true)), "current");
        assert_eq!(source_state_word(Some(false)), "stale");
        assert_eq!(source_state_word(None), "unknown");
    }

    /// End to end over a real repo: a move by a stylesheet keeps a binary
    /// current, and a move by engine source makes it stale.
    #[tokio::test]
    async fn no_restart_between_passes_a_css_move_and_stops_a_rust_move() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        let commit_file = |path: &str| {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, uuid::Uuid::new_v4().to_string()).unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", path]);
            git(&["rev-parse", "--short", "HEAD"])
        };
        let built = commit_file("README.md");
        commit_file("crates/lucidos-app/src/styles/header-mark.css");
        assert_eq!(no_restart_between(&built, "HEAD", &dir).await, Some(true));
        commit_file("crates/lucidos-engine/src/lib.rs");
        assert_eq!(no_restart_between(&built, "HEAD", &dir).await, Some(false));
        assert_eq!(no_restart_between("0123456789ab", "HEAD", &dir).await, None);
    }

    /// A late snapshot past the start HEAD, even by engine source, counts as
    /// published since that start. An older binary does not, so it keeps the
    /// fresh retry, and `build_covers_commit` stays strict for `Ready`.
    #[tokio::test]
    async fn a_later_snapshot_is_published_since_its_start_but_does_not_cover_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        let commit_file = |path: &str| {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, uuid::Uuid::new_v4().to_string()).unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", path]);
            git(&["rev-parse", "--short", "HEAD"])
        };
        let older = commit_file("crates/lucidos-engine/src/a.rs");
        let start = commit_file("crates/lucidos-engine/src/b.rs");
        let later = commit_file("crates/lucidos-engine/src/c.rs");
        assert_eq!(snapshot_since(&start, &start, &dir).await, Some(true));
        assert_eq!(snapshot_since(&later, &start, &dir).await, Some(true));
        assert_eq!(snapshot_since(&older, &start, &dir).await, Some(false));
        assert_eq!(build_covers_commit(&later, &start, &dir).await, Some(false));
        assert_eq!(snapshot_since("0123456789ab", &start, &dir).await, None);
    }

    #[test]
    fn only_the_value_one_pins_the_engine_version() {
        use std::ffi::OsStr;
        assert_eq!(
            VersionTracking::from_flag(Some(OsStr::new("1"))),
            VersionTracking::PinnedToBuild
        );
        for unpinned in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert_eq!(
                VersionTracking::from_flag(unpinned.map(OsStr::new)),
                VersionTracking::FollowCheckout,
                "{unpinned:?} must leave a dev engine following its checkout"
            );
        }
    }

    /// The e2e engine's regression: a commit landing mid-run made it rebuild
    /// itself and raise a version toast over unrelated specs.
    #[test]
    fn a_pinned_engine_never_sees_a_newer_version() {
        assert!(!newer_version_visible(
            VersionTracking::PinnedToBuild,
            false
        ));
        assert!(newer_version_visible(
            VersionTracking::FollowCheckout,
            false
        ));
        assert!(!newer_version_visible(
            VersionTracking::FollowCheckout,
            true
        ));
        assert!(!newer_version_visible(VersionTracking::PinnedToBuild, true));
    }

    /// The gate only pins anything if every probe asks it first. Self-heal and
    /// the pending toast read `source_behind_head`, the Switch reads the disk
    /// probe, and the peer sync raises the Refresh toast. A probe that skips
    /// the gate re-opens a mid-run rebuild or toast.
    #[test]
    fn every_newer_version_probe_asks_the_gate_first() {
        // Split so this test's own source is not a match.
        let gate = concat!("if !self.newer_version", "_visible() {");
        for (src, probe) in [
            (
                include_str!("engine_version.rs"),
                "pub async fn source_behind_head(&self) -> bool {",
            ),
            (
                include_str!("engine_version.rs"),
                "async fn disk_id_is_upgrade(&self, disk: Option<&str>) -> bool {",
            ),
            (
                include_str!("frontend_refresh.rs"),
                "async fn sync_served_frontend_if_safe(self: &Arc<Self>) {",
            ),
        ] {
            let start = src.find(probe).expect("probe exists") + probe.len();
            let first_line = src[start..]
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .expect("probe has a body");
            assert_eq!(first_line, gate, "{probe} must ask the pin gate first");
        }
    }

    /// A DIFFERENT on-disk binary is an update only when it is not provably
    /// OLDER. Everything indeterminate keeps the plain difference test, so this
    /// can only remove a false positive.
    #[test]
    fn disk_upgrade_verdict_offers_only_a_step_forward() {
        // The downgrade: disk `71c8d39b1` is an ancestor of running `aa7075ee2`.
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
        // Same id: nothing to switch onto, whatever git says.
        assert!(!disk_upgrade_verdict(
            Some("aa7075ee2"),
            "aa7075ee2",
            Some(false)
        ));
        // Unreadable disk id (packaged, mid-rewrite): no update.
        assert!(!disk_upgrade_verdict(None, "aa7075ee2", None));
        // Indeterminate ancestry falls back to "different is an update", so a
        // real one is never MISSED.
        assert!(disk_upgrade_verdict(Some("cc9988776"), "aa7075ee2", None));
        // Same commit, different uncommitted diff: a real rebuild.
        assert!(disk_upgrade_verdict(
            Some("aa7075ee2-0badc0ffee123456"),
            "aa7075ee2",
            Some(false)
        ));
    }

    /// The real git probe behind the direction check, against a throwaway repo:
    /// ancestor → `Some(true)`, the reverse → `Some(false)`, an unknown object →
    /// `None` (so an unresolvable id can't be mistaken for "provably older").
    #[tokio::test]
    async fn commit_is_strict_ancestor_reads_history_direction() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
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
        // The abbreviation trap. Build ids carry the SHORT sha while HEAD
        // arrives full, and `git merge-base --is-ancestor X X` exits 0. Without
        // the prefix-aware screen the same commit reads as provably older.
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
    fn group_commit_subjects_groups_and_caps_each_list_but_not_the_counts() {
        let mut log = String::new();
        for i in 1..=8 {
            log.push_str(&format!("fix: bug {i}\n"));
        }
        log.push_str("feat: the one feature\n");
        for i in 1..=4 {
            log.push_str(&format!("docs: page {i}\n"));
        }
        let parsed = group_commit_subjects(log.lines());

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

        // Blank and whitespace-only subjects are not commits.
        let ragged = group_commit_subjects(["fix: one", "", "   ", "fix: two"]);
        assert_eq!(ragged.total, 2);
        assert_eq!(ragged.groups[0].descriptions, vec!["one", "two"]);

        // A genuinely empty range is a real answer: zero, with nothing to list.
        let none = group_commit_subjects([]);
        assert_eq!(none.total, 0);
        assert!(none.groups.is_empty());
    }

    /// A ready version is its own reason to read the range. The confirm behind
    /// *Switch to new version* lists what the switch brings, and that state
    /// implies none of the other three: the build has finished, no peer is
    /// building, and a binary can differ from the running one while the source
    /// is level with HEAD.
    #[test]
    fn a_switch_on_offer_wants_the_range_on_its_own() {
        assert!(wants_pending_commits(false, false, false, true));
        // ...and an at-rest workspace still forks no git.
        assert!(!wants_pending_commits(false, false, false, false));
        // The three pre-existing reasons are unchanged.
        assert!(wants_pending_commits(true, false, false, false));
        assert!(wants_pending_commits(false, true, false, false));
        assert!(wants_pending_commits(false, false, true, false));
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
        assert!(BuildState::ready_from(None).elapsed().is_none());
        assert!(
            BuildState::failed_with(BuildFailure::plain("error: boom".into()))
                .elapsed()
                .is_none()
        );
        assert_eq!(BuildState::building_now().as_wire(), "building");
        // The verified HEAD is internal bookkeeping; the wire state stays ready.
        assert_eq!(BuildState::ready_from(None).as_wire(), "ready");
        assert_eq!(
            BuildState::ready_from(Some("head1".into())).as_wire(),
            "ready"
        );
    }

    /// The real range against a throwaway repo, landed the way Apply lands: the
    /// branch merges the trunk in, then the trunk fast-forwards. The served
    /// client's commit decides: what it carries is left out, everything else
    /// since the running commit is listed. No MERGE subject is listed, and an
    /// unresolvable range is UNKNOWN.
    #[tokio::test]
    async fn pending_commits_leave_out_what_the_served_client_carries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs in the test environment")
        };
        let stdout = |args: &[&str]| {
            String::from_utf8(git(args).stdout)
                .unwrap()
                .trim()
                .to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "running: the version in use"]);
        let running = stdout(&["rev-parse", "HEAD"]);
        // `init.defaultBranch` is the user's config, so read the trunk's name.
        let trunk = stdout(&["symbolic-ref", "--short", "HEAD"]);
        let apply = |branch: &str, base: &str, commits: &[(&str, &str)]| {
            git(&["checkout", "-q", "-b", branch, base]);
            for (subject, file) in commits {
                let path = dir.join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, subject).unwrap();
                git(&["add", "."]);
                git(&["commit", "-qm", subject]);
            }
            git(&["merge", "-q", "--no-edit", &trunk]);
            git(&["checkout", "-q", &trunk]);
            git(&["merge", "-q", "--ff-only", branch]);
        };
        apply(
            "a",
            &running,
            &[("feat(ui): live, landed before any engine change", "app.tsx")],
        );
        // INV-A lets the served client advance to here, and no further.
        let served = stdout(&["rev-parse", "HEAD"]);
        assert_eq!(
            compatible_served_commit(&running, &served, &dir).await,
            Some(served.clone()),
            "a frontend-only landing can count as served"
        );
        apply(
            "b",
            &running,
            &[("fix(engine): the engine change", "src/lib.rs")],
        );
        apply(
            "c",
            &running,
            &[("feat(ui): held behind the engine change", "app2.tsx")],
        );
        apply(
            "d",
            &trunk,
            &[
                ("feat(ui): a frontend step", "app3.tsx"),
                ("feat: the newer one", "src/new.rs"),
            ],
        );

        // The served commit arrives as the snapshot's stamp, the way the
        // engine reads it.
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(
            snapshot.join(crate::codegen_inputs::SOURCE_COMMIT_FILE),
            format!("{served}\n"),
        )
        .unwrap();
        let stamped = crate::engine::frontend_snapshot::read_source_commit(&snapshot);
        assert_eq!(stamped.as_deref(), Some(served.as_str()));
        let commits = pending_commits_since(&running, stamped.as_deref(), &dir)
            .await
            .expect("a resolvable range is a real answer");
        assert_eq!(
            commits.total, 4,
            "the engine change and every landing after it wait for the switch; \
             the served frontend commit and the merges do not"
        );
        let group = |kind| {
            commits
                .groups
                .iter()
                .find(|g| g.kind == kind)
                .unwrap_or_else(|| panic!("{kind:?} group is present"))
        };
        // Sorted: commits made within one second share a date, so `git log`
        // order between them is not fixed.
        let mut new = group(CommitGroupKind::New).descriptions.clone();
        new.sort();
        assert_eq!(
            new,
            vec![
                "the newer one",
                "ui: a frontend step",
                "ui: held behind the engine change"
            ],
            "a frontend commit landed after the engine change waits for the switch"
        );
        assert_eq!(
            group(CommitGroupKind::Fixed).descriptions,
            vec!["engine: the engine change"]
        );
        assert!(
            !commits
                .groups
                .iter()
                .flat_map(|g| g.descriptions.iter())
                .any(|d| d.contains("Merge branch")),
            "no merge subject reaches the toast"
        );

        // The served client never advanced (a failed rebuild, or no served
        // commit known): the frontend commit is listed too.
        for served in [Some(running.as_str()), None] {
            let all = pending_commits_since(&running, served, &dir).await;
            assert_eq!(all.map(|c| c.total), Some(5), "served {served:?}");
        }

        // Running at HEAD: a real, empty answer.
        let head = stdout(&["rev-parse", "HEAD"]);
        assert_eq!(
            compatible_served_commit(&running, &head, &dir).await,
            None,
            "a commit past an engine change never counts as served"
        );
        // A snapshot stamped past the engine change: a client for the new
        // engine. The switch still brings the engine change, so all five show.
        let past_engine = pending_commits_since(&running, Some(&head), &dir).await;
        assert_eq!(
            past_engine.map(|c| c.total),
            Some(5),
            "an engine change is never left out as already served"
        );
        let level = pending_commits_since(&head, Some(&head), &dir).await;
        assert_eq!(level.map(|c| c.total), Some(0), "nothing to bring");

        // A range git refuses (unknown object) exits non-zero: unknown, not empty.
        assert_eq!(
            pending_commits_since("0000000000000000000000000000000000000000", None, &dir).await,
            None,
            "a range git cannot resolve says nothing about what is pending"
        );
    }
}
