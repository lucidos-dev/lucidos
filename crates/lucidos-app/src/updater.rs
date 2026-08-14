//! Auto-update wiring (`tauri-plugin-updater`). No-op in development.
//!
//! A packaged build surfaces updates INSIDE the workspace UI, not a native
//! launch dialog and not the picker. The web app polls [`check_app_update`] and
//! shows an in-app toast whose action calls
//! [`install_app_update_and_restart`]. That installs the new signed bundle and
//! restarts the WHOLE stack: the launchd background service AND the GUI client.
//!
//! **The client comes back frontmost**, via
//! `desktop::schedule_relaunch_after_exit` rather than `app.restart()`. ADR 0072
//! records why.
//!
//! **The install narrates itself.** Every step emits an [`AppUpdateProgress`]
//! frame on the [`PROGRESS_EVENT`] Tauri event, because a silent `await` across
//! a bundle download reads as a frozen app.
//!
//! **Only the download is cancellable.** Until the bytes are verified they
//! exist only in memory, so abandoning them changes nothing on disk. The
//! [`AppUpdateRun`] state machine makes that structural, and ADR 0073 covers a
//! swap that fails anyway.

use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::sync::Mutex;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

/// Tauri event carrying [`AppUpdateProgress`] frames to the page. Emitted with
/// `AppHandle::emit`, so every webview of the client sees it. Already covered
/// by `core:default` on both the local and the gateway origin.
const PROGRESS_EVENT: &str = "app-update-progress";

/// Minimum byte advance between two `downloading` frames when the server declared
/// no `Content-Length`. A known-size download steps by whole percentage points
/// instead; this is the fallback for the case where there is no percentage to step
/// on. Either way the frame count stays ~100 for a ~100 MB payload rather than one
/// per network chunk.
const PROGRESS_BYTE_STEP: u64 = 1024 * 1024;

/// Where an update run currently is. Ordered as the run proceeds; `Cancelled` and
/// `Failed` are terminal off-ramps.
///
/// Serialized internally-tagged so the page reads it as a discriminated union on
/// `phase`, with kebab-case wire values (CLAUDE.md: public API parameter values are
/// kebab-case). The TypeScript mirror is `AppUpdateProgress` in `utils/tauri.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum AppUpdatePhase {
    /// Asking the update endpoint whether there is anything newer.
    Checking,
    /// Streaming the new bundle. `total` is `None` when the server sent no
    /// `Content-Length` — the page must then show bytes without a percentage
    /// rather than invent one.
    Downloading { downloaded: u64, total: Option<u64> },
    /// Bytes are in; checking the updater signature over them.
    Verifying,
    /// Unpacking and swapping the `.app` bundle. No way back from here.
    Installing,
    /// Kicking the launchd background service onto the new binaries.
    RestartingServices,
    /// Re-execing the client. The page is about to be torn down.
    Relaunching,
    /// The user abandoned the download; nothing on disk changed.
    Cancelled,
    /// Terminal failure, carrying the reason (the page shows it — this runs on a
    /// click, so it is owed a real error, not a `console.warn`).
    Failed { message: String },
    /// The install left no runnable app behind, so the swap destroyed the old
    /// bundle without landing the new one. Raised from BOTH install outcomes: the
    /// destructive upstream case reports `Err` (see [`installed_bundle_fault`]),
    /// and the rarer one reports `Ok` over an app that is not there.
    ///
    /// Its own terminal phase rather than a longer [`AppUpdatePhase::Failed`]
    /// string, because the two need different handling. ADR 0073 records why.
    BundleSwapFailed { message: String },
}

/// One progress frame: the phase plus the version it applies to (`None` until the
/// check resolves, since we don't know what we're installing before then).
#[derive(Clone, Serialize)]
struct AppUpdateProgress {
    version: Option<String>,
    #[serde(flatten)]
    phase: AppUpdatePhase,
}

/// Announce a phase to the page. Best-effort by design: the last frames race
/// the client teardown, and a frame that fails to reach a webview must never
/// fail an update.
fn emit(app: &AppHandle, version: Option<&str>, phase: AppUpdatePhase) {
    let _ = app.emit(
        PROGRESS_EVENT,
        AppUpdateProgress {
            version: version.map(str::to_string),
            phase,
        },
    );
}

/// Announce a terminal failure AND return it as the command's error string.
/// Both halves matter: the event gives the page a terminal phase even if the
/// rejection races the teardown, and the string is what the `catch` reports.
///
/// `phase` picks WHICH terminal phase carries the message. Ordinary failures go
/// through [`fail`]; the bundle-swap case has its own phase because the page has
/// to narrate it differently.
fn fail_as(
    app: &AppHandle,
    version: Option<&str>,
    phase: fn(String) -> AppUpdatePhase,
    message: String,
) -> String {
    emit(app, version, phase(message.clone()));
    message
}

/// [`fail_as`] with the ordinary [`AppUpdatePhase::Failed`].
fn fail(app: &AppHandle, version: Option<&str>, message: String) -> String {
    fail_as(
        app,
        version,
        |message| AppUpdatePhase::Failed { message },
        message,
    )
}

/// A failed run, carrying the version it failed on when we had got far enough to
/// know one. Kept separate from the plain error string so the single [`fail`]
/// callsite can attribute the failure.
struct UpdateFailure {
    version: Option<String>,
    message: String,
}

impl UpdateFailure {
    fn new(version: Option<String>, message: String) -> Self {
        Self { version, message }
    }
}

/// One `downloading` frame worth announcing.
struct DownloadFrame {
    downloaded: u64,
    total: Option<u64>,
}

/// Byte-progress bookkeeping for a single download, kept out of the callback so
/// the throttle is unit-testable on its own.
///
/// The plugin invokes the chunk callback once per network chunk, thousands of
/// times for the bundle. Every frame is an IPC message plus a signal-graph
/// update. So a frame is only produced on a *meaningful* advance: a whole
/// percentage point when the size is known, [`PROGRESS_BYTE_STEP`] bytes when
/// it is not. The first chunk and the final byte count always produce one, so
/// the page never sits at "0 bytes" and never freezes one step short.
#[derive(Default)]
struct DownloadTracker {
    downloaded: u64,
    /// Size the server declared, remembered from the chunk callback so the
    /// end-of-stream frame reports the real value instead of inventing one.
    total: Option<u64>,
    /// Cumulative count carried by the last frame produced; `None` before the
    /// first.
    last_announced: Option<u64>,
}

impl DownloadTracker {
    /// Record `len` more bytes, returning a frame when the advance clears the
    /// throttle.
    fn chunk(&mut self, len: u64, total: Option<u64>) -> Option<DownloadFrame> {
        self.downloaded += len;
        if total.is_some() {
            self.total = total;
        }
        let Some(last) = self.last_announced else {
            return Some(self.announce());
        };
        if self.downloaded <= last {
            return None;
        }
        let worth_announcing = match self.total {
            Some(total) if total > 0 => {
                self.downloaded >= total || percent(self.downloaded, total) > percent(last, total)
            }
            _ => self.downloaded - last >= PROGRESS_BYTE_STEP,
        };
        worth_announcing.then(|| self.announce())
    }

    /// End-of-stream frame, unless the final count was already the last one
    /// announced.
    fn finish(&mut self) -> Option<DownloadFrame> {
        (self.last_announced != Some(self.downloaded)).then(|| self.announce())
    }

    fn announce(&mut self) -> DownloadFrame {
        self.last_announced = Some(self.downloaded);
        DownloadFrame {
            downloaded: self.downloaded,
            total: self.total,
        }
    }
}

/// Whole percent of `total` that `bytes` represents. `saturating_mul` keeps an
/// absurdly large payload from wrapping instead of pinning at 100.
fn percent(bytes: u64, total: u64) -> u64 {
    bytes.saturating_mul(100) / total
}

/// What the single update slot is doing. Modelled as a state machine because
/// the cancellation rule *is* a state question. A cancel must abort the
/// download and must not touch an install already under way, and no timing
/// accident may blur the two.
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// Slot claimed; the abortable task has not been handed over yet. A window of
    /// a few instructions, but a cancel can still land in it.
    Starting,
    /// A cancel landed during [`Phase::Starting`] — the task is aborted the moment
    /// it arrives.
    CancelPending,
    /// The check + download task, still abortable.
    Downloading(JoinHandle<()>),
    /// Past the download: the bundle swap and the restart cannot be undone, so a
    /// cancel from here is refused.
    Committed,
}

/// The single in-flight app-update run. Managed state (`lib.rs`), shared by
/// [`install_app_update_and_restart`] and [`cancel_app_update`].
#[derive(Default)]
pub struct AppUpdateRun(Mutex<Phase>);

impl AppUpdateRun {
    /// Claim the slot. `false` when a run is already under way. The caller must
    /// then bail out WITHOUT emitting a terminal phase, or it would wipe the
    /// live run's narration out of the UI.
    fn begin(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Idle) {
            return false;
        }
        *phase = Phase::Starting;
        true
    }

    /// Hand over the abortable download task. A cancel that landed during
    /// [`Phase::Starting`] is honoured here, which is what closes that window.
    fn armed(&self, task: JoinHandle<()>) {
        let mut phase = self.lock();
        match *phase {
            Phase::CancelPending => {
                task.abort();
                *phase = Phase::Idle;
            }
            _ => *phase = Phase::Downloading(task),
        }
    }

    /// The bytes are in hand — take the run past the point of no return.
    ///
    /// `false` when a cancel got there FIRST. A cancel landing between the
    /// download resolving and this command being scheduled has already been
    /// accepted, even though the buffered result still arrives. Committing
    /// anyway would silently install an update the user cancelled. Nothing is
    /// on disk yet, so the caller discards the bytes.
    fn commit(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Downloading(_)) {
            return false;
        }
        *phase = Phase::Committed;
        true
    }

    /// Free the slot after a run ended without restarting.
    ///
    /// Deliberately NOT called on the cancel path: [`Self::cancel`] already
    /// returned the slot to `Idle`, so a later release from the abandoned run
    /// could reset a *replacement* run that had since begun. Whoever transitions
    /// to `Idle` owns that transition.
    fn release(&self) {
        *self.lock() = Phase::Idle;
    }

    /// Abort an in-flight download. `false` when there is nothing abortable:
    /// either no run at all, or one that has already committed.
    fn cancel(&self) -> bool {
        let mut phase = self.lock();
        match std::mem::take(&mut *phase) {
            Phase::Starting => {
                *phase = Phase::CancelPending;
                true
            }
            Phase::Downloading(task) => {
                task.abort();
                *phase = Phase::Idle;
                true
            }
            other => {
                *phase = other;
                false
            }
        }
    }

    /// A poisoned lock here would mean a panic while holding it. The guarded
    /// state is a plain enum with no invariant a panic could have broken
    /// mid-write, so recovering beats propagating into every later attempt.
    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A newer signed build, and what is in it.
///
/// The notes are the ONLY way a client can say what a pending update contains.
/// The offered version postdates the binary doing the offering, so it is by
/// construction absent from the changelog baked into it. Showing the installed
/// changelog instead would name the version already running, which is worse
/// than showing none because it looks like it worked.
#[derive(Clone, Serialize)]
pub struct AppUpdateOffer {
    version: String,
    /// The release's notes as raw markdown, or `None` when the manifest carries
    /// none. `latest.json`'s `notes` is written from this repo's own
    /// `CHANGELOG.md` section for that release, so in practice it is present.
    /// Optional because nothing structurally guarantees it for a hand-cut or
    /// older release, and a missing note must degrade to no notes shown.
    notes: Option<String>,
}

/// Check GitHub Releases for a newer signed build. Returns the offer when one is
/// available, else `None`. The packaged workspace UI polls this (gated on running
/// in the Tauri client) to drive the in-app update toast.
/// No-op (`None`) in development.
#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> Result<Option<AppUpdateOffer>, String> {
    if tauri::is_dev() {
        return Ok(None);
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| AppUpdateOffer {
        version: u.version.clone(),
        // Blank is absent. An empty-string body would otherwise render as an
        // affordance that opens onto nothing.
        notes: u
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string),
    }))
}

/// Resolve the available update and stream it into memory, announcing `checking`,
/// `downloading` and `verifying` as it goes. Returns the update alongside its
/// bytes; the caller installs them.
///
/// Runs inside a spawned task so it can be aborted — which is why it reports
/// failures as a value rather than emitting them itself: an aborted task never
/// gets to run cleanup, so the single terminal-phase emit lives in the caller.
async fn check_and_download(
    app: &AppHandle,
    updater: Updater,
) -> Result<(Update, Vec<u8>), UpdateFailure> {
    let update = updater
        .check()
        .await
        .map_err(|e| UpdateFailure::new(None, e.to_string()))?
        .ok_or_else(|| UpdateFailure::new(None, "No update available".to_string()))?;
    let version = update.version.clone();

    // Announce the phase flip before the first chunk lands: on a slow link the
    // first chunk can be seconds away, and until then "Checking…" would be a lie.
    emit(
        app,
        Some(&version),
        AppUpdatePhase::Downloading {
            downloaded: 0,
            total: None,
        },
    );

    // Shared by both callbacks (one `FnMut`, one `FnOnce`), neither of which
    // awaits — so the guard is never held across a suspension point.
    let tracker = Mutex::new(DownloadTracker::default());
    let bytes = update
        .download(
            |chunk, total| {
                let frame = tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .chunk(chunk as u64, total);
                if let Some(frame) = frame {
                    emit(
                        app,
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
            },
            || {
                // Signature verification runs after the last chunk, still inside
                // `download` — so this hook is where `verifying` belongs.
                let frame = tracker.lock().unwrap_or_else(|e| e.into_inner()).finish();
                if let Some(frame) = frame {
                    emit(
                        app,
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
                emit(app, Some(&version), AppUpdatePhase::Verifying);
            },
        )
        .await
        .map_err(|e| UpdateFailure::new(Some(version.clone()), e.to_string()))?;

    Ok((update, bytes))
}

// ── Did the bundle swap actually land an app? ────────────────────────────────

/// The main executable inside the `.app`, relative to the bundle root. Tauri
/// derives `mainBinaryName` from the crate's binary name, NOT from
/// `productName`, so the bundle is `Lucidos.app` and the executable inside it
/// is `lucidos-app`. This is also the path launchd is handed as the job's
/// `ProgramArguments`, which is what makes its absence a crash loop.
///
/// Derived from `CARGO_PKG_NAME` rather than spelled out, because a spelled-out
/// name that drifted would not fail loudly. The check would look for a path
/// that cannot exist and block every update over a rename.
#[cfg(target_os = "macos")]
const BUNDLE_MAIN_EXECUTABLE: &str = concat!("Contents/MacOS/", env!("CARGO_PKG_NAME"));

/// What a post-install look at the swapped bundle found. Only
/// [`BundleVerdict::Runnable`] lets the run go on to restart the service.
#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum BundleVerdict {
    /// The bundle is there and its main executable is present and executable.
    Runnable,
    /// Nothing at the bundle path at all: the swap deleted the old app and never
    /// put a new one down.
    BundleMissing,
    /// The bundle directory exists but its main executable does not, so whatever
    /// landed is not an app.
    ExecutableMissing,
    /// The main executable is there with no execute bit, which launchd can start
    /// exactly as well as a missing one.
    ExecutableNotExecutable,
}

#[cfg(target_os = "macos")]
impl BundleVerdict {
    /// The half-sentence naming what is wrong, or `None` when nothing is.
    fn reason(&self) -> Option<String> {
        match self {
            Self::Runnable => None,
            Self::BundleMissing => Some("the application bundle is gone".to_string()),
            Self::ExecutableMissing => Some(format!(
                "the bundle is there but {BUNDLE_MAIN_EXECUTABLE} is missing"
            )),
            Self::ExecutableNotExecutable => Some(format!(
                "{BUNDLE_MAIN_EXECUTABLE} is there but is not executable"
            )),
        }
    }
}

/// Is there a runnable app at `bundle`? Split out from
/// [`installed_bundle_fault`] with the path as a parameter, so the rule can be
/// tested against a directory a test builds.
#[cfg(target_os = "macos")]
fn installed_bundle_verdict(bundle: &Path) -> BundleVerdict {
    use std::os::unix::fs::PermissionsExt;

    if !bundle.is_dir() {
        return BundleVerdict::BundleMissing;
    }
    let Ok(meta) = std::fs::metadata(bundle.join(BUNDLE_MAIN_EXECUTABLE)) else {
        return BundleVerdict::ExecutableMissing;
    };
    if !meta.is_file() {
        return BundleVerdict::ExecutableMissing;
    }
    // Any execute bit at all is the floor. The question is whether the swap
    // landed a real executable, not whether the permissions are ideal.
    if meta.permissions().mode() & 0o111 == 0 {
        return BundleVerdict::ExecutableNotExecutable;
    }
    BundleVerdict::Runnable
}

/// Where the plugin just swapped the bundle, derived the way the PLUGIN derives
/// it: its own public `extract_path_from_executable` over `current_exe()`.
///
/// Both halves are deliberate. `UpdaterBuilder::build` sets `extract_path` from
/// `current_exe()` whenever no `executable_path` override is given, and `lib.rs`
/// registers the plugin bare. So this resolves the same path `Update::install`
/// wrote to, and ADR 0073 records why a hardcoded one is wrong.
#[cfg(target_os = "macos")]
fn installed_bundle_path() -> Result<std::path::PathBuf, String> {
    let exe = tauri::utils::platform::current_exe()
        .map_err(|e| format!("cannot resolve this app's own executable: {e}"))?;
    tauri_plugin_updater::extract_path_from_executable(&exe).map_err(|e| {
        format!(
            "cannot resolve this app's bundle from {}: {e}",
            exe.display()
        )
    })
}

/// What is wrong with the app on disk after an install attempt, as the middle
/// of a sentence. `None` when there is a runnable app there.
///
/// Upstream `tauri-plugin-updater` moves the current `.app` into a `TempDir`
/// and has no restore branch if the final rename fails. A failed swap therefore
/// deletes the backup on the way out. ADR 0073 records the blast radius here
/// and both decisions below.
///
/// **Fail closed on a path we cannot resolve.** Not knowing where the bundle is
/// is exactly as informative as finding nothing there, and the recovery advice
/// is the same either way.
#[cfg(target_os = "macos")]
fn installed_bundle_fault() -> Option<String> {
    match installed_bundle_path() {
        Err(e) => Some(format!("Lucidos cannot tell where its own bundle is: {e}")),
        Ok(bundle) => installed_bundle_verdict(&bundle)
            .reason()
            .map(|reason| format!("no runnable app is at {}: {reason}", bundle.display())),
    }
}

/// The bundle layout above is a macOS one, and macOS is the only packaged shape
/// Lucidos ships, so there is nothing to check anywhere else.
#[cfg(not(target_os = "macos"))]
fn installed_bundle_fault() -> Option<String> {
    None
}

/// The user-facing [`AppUpdatePhase::BundleSwapFailed`] message.
///
/// Both callers compose through here so the recovery advice cannot drift apart,
/// but the OPENING differs and that difference is the point. `install_error` is
/// `Some` when the plugin itself reported failure, which is where the
/// destructive upstream case lands. Telling that user "the update reported
/// success" would be a lie. It is `None` for the rarer shape where `install`
/// returned `Ok` over an app that is not there.
///
/// Pure, so the wording the user reads is unit-tested rather than inspected.
fn bundle_swap_message(fault: &str, install_error: Option<&str>) -> String {
    let opening = match install_error {
        Some(e) => format!("The update failed and {fault}. The installer reported: {e}."),
        None => format!("The update reported success but {fault}."),
    };
    format!(
        "{opening} Lucidos has NOT been restarted, so the background service keeps running \
         the version it already loaded and your workspaces stay up until the machine reboots. \
         Reinstall Lucidos from the .dmg to recover."
    )
}

/// Install the available update and restart EVERYTHING onto the new version:
/// swap the bundle, restart the launchd background service so it runs the NEW
/// binaries, then relaunch the GUI client. Never returns on success.
///
/// Ordering is load-bearing: install first, then the service restart, then the
/// never-returning relaunch. The service restart is best-effort, since a
/// failure is logged and the service picks the new binary up on its next start.
#[tauri::command]
pub async fn install_app_update_and_restart(
    app: AppHandle,
    run: State<'_, AppUpdateRun>,
) -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Updates are only available in a packaged build".to_string());
    }
    if !run.begin() {
        // Deliberately no `failed` emit: that would replace the RUNNING update's
        // narration with an error about this duplicate request.
        return Err("An update is already in progress".to_string());
    }

    emit(&app, None, AppUpdatePhase::Checking);
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            run.release();
            return Err(fail(&app, None, e.to_string()));
        }
    };

    // The check + download run in a spawned task so `cancel_app_update` can abort
    // them. The result comes back over a channel, and a CLOSED channel is the
    // cancellation signal: aborting drops the task, which drops the sender.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let emitter = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let result = check_and_download(&emitter, updater).await;
        let _ = tx.send(result).await;
    });
    run.armed(task);

    let Some(result) = rx.recv().await else {
        // Aborted mid-download: the bytes are discarded and nothing on disk
        // changed. `cancel` already returned the slot to Idle — see
        // `AppUpdateRun::release`.
        emit(&app, None, AppUpdatePhase::Cancelled);
        return Ok(());
    };
    if !run.commit() {
        // A cancel landed in the gap between the download resolving and this
        // task being scheduled, so the buffered result arrived anyway. The
        // cancel was accepted; installing now would override it silently.
        let version = result.ok().map(|(update, _)| update.version);
        emit(&app, version.as_deref(), AppUpdatePhase::Cancelled);
        return Ok(());
    }

    // Committing BEFORE inspecting the result is deliberate, not an oversight: it
    // is what makes the `release` below unambiguously ours. Winning the commit
    // proves no cancel took the slot, so freeing it here cannot reset a
    // replacement run — the ownership rule `AppUpdateRun::release` documents.
    // Checking the result first would let a failed download release a slot a
    // cancel had already handed to someone else.
    let (update, bytes) = match result {
        Ok(downloaded) => downloaded,
        Err(e) => {
            run.release();
            return Err(fail(&app, e.version.as_deref(), e.message));
        }
    };
    let version = update.version.clone();

    emit(&app, Some(&version), AppUpdatePhase::Installing);
    // `Update::install` is synchronous, so it goes to a blocking thread rather
    // than stalling an async runtime worker the progress IPC also rides on.
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    let outcome = match installed {
        Ok(outcome) => outcome,
        Err(e) => {
            run.release();
            return Err(fail(&app, Some(&version), format!("install task: {e}")));
        }
    };
    // BOTH outcomes have to ask the same question. The destructive case arrives
    // here as an ERROR, not as a false success. Reporting it as an ordinary
    // `failed` would tell a user whose app is gone to try again. See ADR 0073.
    if let Err(e) = outcome {
        run.release();
        let e = e.to_string();
        return Err(match installed_bundle_fault() {
            None => fail(&app, Some(&version), e),
            Some(fault) => fail_as(
                &app,
                Some(&version),
                |message| AppUpdatePhase::BundleSwapFailed { message },
                bundle_swap_message(&fault, Some(&e)),
            ),
        });
    }

    // The rarer shape: `install` returned Ok over an app that is not actually
    // there. Prove there is something runnable to restart INTO before touching
    // the launchd job, which is a KeepAlive job whose ProgramArguments point
    // into that bundle.
    //
    // On failure the job is deliberately left ALONE rather than booted out, on
    // both paths. See ADR 0073.
    if let Some(fault) = installed_bundle_fault() {
        run.release();
        return Err(fail_as(
            &app,
            Some(&version),
            |message| AppUpdatePhase::BundleSwapFailed { message },
            bundle_swap_message(&fault, None),
        ));
    }

    // Window geometry is already current on disk: the debounced flush in `run()`
    // persists it well within the download and install above. We deliberately
    // do NOT call `save_window_state` here, because this runs on a Tokio worker
    // thread where that call can deadlock the UI. The final save happens from
    // the main thread, in `exit_after_relaunch_scheduled`.
    //
    // New bytes are on disk. Restart the whole background service onto them
    // BEFORE relaunching the client, which never returns.
    emit(&app, Some(&version), AppUpdatePhase::RestartingServices);
    if let Err(e) = crate::desktop::restart_service() {
        eprintln!("[updater] background service restart failed: {e}");
    }
    // Relaunch the client onto its new bytes. Never returns. Through
    // LaunchServices when we can, so the updated client comes up in front
    // rather than behind everything. See ADR 0072.
    emit(&app, Some(&version), AppUpdatePhase::Relaunching);
    match crate::desktop::schedule_relaunch_after_exit() {
        // This command runs on the async runtime, so the exit is marshalled to
        // the main thread. It does not return, so `app.restart()` cannot also
        // run and bring up a second client.
        Ok(()) => crate::exit_after_relaunch_scheduled(&app),
        Err(e) => {
            eprintln!("[updater] no LaunchServices relaunch ({e}); respawning directly");
            app.restart()
        }
    }
}

/// Abandon an in-flight app-update download. Only the check and download can be
/// cancelled. Once the bundle swap has started the run has committed and this
/// is a no-op, which is the honest answer. The page learns the outcome from the
/// `cancelled` progress frame the aborted run emits, not from this call.
#[tauri::command]
pub fn cancel_app_update(run: State<'_, AppUpdateRun>) {
    run.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a whole download through the tracker the way the plugin's callback
    /// does, returning every frame it produced.
    fn run_download(chunk: u64, total: Option<u64>, size: u64) -> Vec<DownloadFrame> {
        let mut tracker = DownloadTracker::default();
        let mut frames = Vec::new();
        let mut sent = 0;
        while sent < size {
            let len = chunk.min(size - sent);
            sent += len;
            if let Some(frame) = tracker.chunk(len, total) {
                frames.push(frame);
            }
        }
        if let Some(frame) = tracker.finish() {
            frames.push(frame);
        }
        frames
    }

    // A ~100 MB bundle arrives as thousands of chunks. One IPC frame per chunk
    // would flood the bridge — the throttle is what keeps the count bounded.
    #[test]
    fn a_known_size_download_emits_about_one_frame_per_percent() {
        let size = 100 * 1024 * 1024;
        let frames = run_download(32 * 1024, Some(size), size);
        assert!(
            frames.len() <= 101,
            "expected at most one frame per percentage point, got {}",
            frames.len()
        );
        assert!(
            frames.len() >= 100,
            "expected ~100 frames, got {}",
            frames.len()
        );
    }

    #[test]
    fn the_first_chunk_always_produces_a_frame() {
        let mut tracker = DownloadTracker::default();
        let frame = tracker
            .chunk(1, Some(100 * 1024 * 1024))
            .expect("the first chunk must announce, however small");
        assert_eq!(frame.downloaded, 1);
    }

    // Without this the bar would stop just short of full and sit there while the
    // signature check runs.
    #[test]
    fn the_final_byte_count_is_always_announced() {
        let size = 10 * 1024 * 1024;
        for total in [Some(size), None] {
            let frames = run_download(32 * 1024, total, size);
            let last = frames.last().expect("a download announces at least once");
            assert_eq!(last.downloaded, size, "total={total:?}");
        }
    }

    // No `Content-Length` means no honest percentage, so the frames must carry
    // `total: None` and the byte step becomes the throttle instead.
    #[test]
    fn an_unknown_size_download_reports_bytes_without_inventing_a_total() {
        let size = 10 * 1024 * 1024;
        let frames = run_download(32 * 1024, None, size);
        assert!(
            frames.iter().all(|f| f.total.is_none()),
            "an unknown-size download must never report a total",
        );
        assert!(
            frames.len() <= 11,
            "expected ~one frame per MiB, got {}",
            frames.len()
        );
    }

    // A download too small to cross a single throttle step still gets the two
    // frames that matter: something as soon as bytes start moving, and the true
    // final count.
    #[test]
    fn a_download_smaller_than_one_step_announces_its_start_and_its_end() {
        let frames = run_download(1024, None, 4096);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].downloaded, 1024);
        assert_eq!(frames[1].downloaded, 4096);
    }

    #[test]
    fn a_zero_length_chunk_produces_no_frame() {
        let mut tracker = DownloadTracker::default();
        tracker.chunk(1024, Some(4096));
        assert!(tracker.chunk(0, Some(4096)).is_none());
    }

    #[test]
    fn only_one_run_can_claim_the_slot() {
        let run = AppUpdateRun::default();
        assert!(run.begin());
        assert!(
            !run.begin(),
            "a second run must be refused while one is live"
        );
        run.release();
        assert!(run.begin(), "the slot frees up once the run ends");
    }

    // The whole point of the state machine: a cancel that arrives after the bundle
    // swap started must not claim to have stopped anything.
    #[test]
    fn a_cancel_after_commit_is_refused() {
        let run = AppUpdateRun::default();
        run.begin();
        run.armed(tauri::async_runtime::spawn(async {}));
        assert!(run.commit());
        assert!(!run.cancel());
    }

    // The download-to-install boundary. The download resolves and its result is
    // buffered on the channel, but a cancel lands before the awaiting task is
    // scheduled: `cancel` accepts it and frees the slot, yet the result still
    // arrives. Committing anyway would install an update the user cancelled,
    // silently — nothing is on disk yet, so the commit must lose the race.
    #[test]
    fn a_cancel_that_lands_before_the_commit_wins() {
        let run = AppUpdateRun::default();
        run.begin();
        run.armed(tauri::async_runtime::spawn(async {}));
        assert!(
            run.cancel(),
            "the cancel is accepted while still downloading"
        );
        assert!(
            !run.commit(),
            "the buffered download result must not commit over an accepted cancel",
        );
    }

    #[test]
    fn a_commit_without_a_download_is_refused() {
        let run = AppUpdateRun::default();
        assert!(!run.commit(), "nothing to commit with no run in flight");
    }

    #[test]
    fn a_cancel_with_no_run_is_refused() {
        let run = AppUpdateRun::default();
        assert!(!run.cancel());
    }

    // The window between claiming the slot and handing over the task is tiny
    // but real. A cancel landing in it is honoured when the task arrives.
    #[test]
    fn a_cancel_during_startup_is_honoured_when_the_task_arrives() {
        let run = AppUpdateRun::default();
        run.begin();
        assert!(run.cancel());
        assert!(
            matches!(*run.lock(), Phase::CancelPending),
            "the cancel must be remembered until the task exists",
        );
    }

    // ── The post-install bundle check ────────────────────────────────────────

    /// A throwaway directory that removes itself, so the bundle cases below can
    /// be built on disk without depending on this machine's real install. Rolled
    /// by hand because the crate has no dev-dependency on `tempfile` and one
    /// helper is cheaper than pulling the tree in for it.
    #[cfg(target_os = "macos")]
    struct TempDir(std::path::PathBuf);

    #[cfg(target_os = "macos")]
    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lucidos-updater-{tag}-{unique}"));
            std::fs::create_dir_all(&path).expect("create the temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Lay down `<root>/Lucidos.app/Contents/MacOS/lucidos-app` with `mode`, and
    /// return the bundle root.
    #[cfg(target_os = "macos")]
    fn write_bundle(root: &Path, mode: u32) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bundle = root.join("Lucidos.app");
        let exe = bundle.join(BUNDLE_MAIN_EXECUTABLE);
        std::fs::create_dir_all(exe.parent().expect("the executable has a parent"))
            .expect("create Contents/MacOS");
        std::fs::write(&exe, b"#!/bin/sh\n").expect("write the main executable");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(mode))
            .expect("set the executable's mode");
        bundle
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_with_an_executable_main_binary_is_runnable() {
        let tmp = TempDir::new("runnable");
        let bundle = write_bundle(tmp.path(), 0o755);
        assert_eq!(installed_bundle_verdict(&bundle), BundleVerdict::Runnable);
    }

    // The upstream failure this whole check exists for: the swap moved the old
    // bundle into a TempDir, the final rename failed, and the TempDir took the
    // backup with it. Nothing is at the path at all.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_vanished_bundle_is_reported_as_missing() {
        let tmp = TempDir::new("vanished");
        let bundle = tmp.path().join("Lucidos.app");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::BundleMissing
        );
    }

    // A partial unpack leaves a directory tree that is not an app. Checking only
    // that the bundle directory exists would call that a success and kickstart
    // launchd onto nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_directory_without_its_main_binary_is_not_runnable() {
        let tmp = TempDir::new("hollow");
        let bundle = tmp.path().join("Lucidos.app");
        std::fs::create_dir_all(bundle.join("Contents/Resources")).expect("create a hollow bundle");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableMissing
        );
    }

    // A directory where the executable should be is the same failure as no
    // executable at all. `metadata` succeeds on it, so the file-type test is
    // what separates them.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_directory_standing_in_for_the_main_binary_is_not_runnable() {
        let tmp = TempDir::new("dir-exe");
        let bundle = tmp.path().join("Lucidos.app");
        std::fs::create_dir_all(bundle.join(BUNDLE_MAIN_EXECUTABLE)).expect("create the stand-in");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableMissing
        );
    }

    // An unpack that dropped the mode bits produces a file launchd can no more
    // start than a missing one. "It exists" is not the question.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_main_binary_with_no_execute_bit_is_not_runnable() {
        let tmp = TempDir::new("no-x");
        let bundle = write_bundle(tmp.path(), 0o644);
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableNotExecutable
        );
    }

    // Every non-runnable verdict has to be able to say what is wrong, since the
    // message is the only thing the user gets. A reason-less failure would render
    // as a blank half-sentence.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_failing_verdict_carries_a_reason_and_the_runnable_one_does_not() {
        assert_eq!(BundleVerdict::Runnable.reason(), None);
        for verdict in [
            BundleVerdict::BundleMissing,
            BundleVerdict::ExecutableMissing,
            BundleVerdict::ExecutableNotExecutable,
        ] {
            let reason = verdict.reason().unwrap_or_default();
            assert!(!reason.is_empty(), "{verdict:?} must explain itself");
        }
    }

    // The whole point of resolving the path from the running executable rather
    // than from a hardcoded /Applications: a user who runs from ~/Applications or
    // a dev location must get a verdict about THEIR bundle.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_verdict_follows_the_bundle_wherever_it_lives() {
        let tmp = TempDir::new("elsewhere");
        let nested = tmp.path().join("Users/someone/Applications");
        std::fs::create_dir_all(&nested).expect("create a non-/Applications location");
        let bundle = write_bundle(&nested, 0o755);
        assert_eq!(installed_bundle_verdict(&bundle), BundleVerdict::Runnable);
    }

    // The message is the whole user-visible product of this check, so its wording
    // is tested rather than eyeballed. Both shapes must carry the fault and the
    // recovery, and neither may claim the app was not restarted when it was.
    #[test]
    fn both_bundle_swap_messages_name_the_fault_and_the_recovery() {
        let fault =
            "no runnable app is at /Applications/Lucidos.app: the application bundle is gone";
        for message in [
            bundle_swap_message(fault, None),
            bundle_swap_message(fault, Some("No such file or directory (os error 2)")),
        ] {
            assert!(message.contains(fault), "the fault must survive: {message}");
            assert!(
                message.contains("Reinstall Lucidos from the .dmg"),
                "the recovery path must survive: {message}",
            );
            assert!(
                message.contains("NOT been restarted"),
                "the user must be told the running service was left alone: {message}",
            );
        }
    }

    // The destructive upstream case reaches us as an Err, so the message on that
    // path must not open by claiming the update succeeded. Saying "the update
    // reported success" to somebody whose app the installer just deleted is the
    // specific lie this split exists to avoid.
    #[test]
    fn an_install_error_is_reported_as_a_failure_not_as_a_false_success() {
        let with_error = bundle_swap_message("the bundle is gone", Some("cross-device link"));
        assert!(
            with_error.starts_with("The update failed and"),
            "an install error must not be narrated as success: {with_error}",
        );
        assert!(
            with_error.contains("cross-device link"),
            "the installer's own reason must reach the user: {with_error}",
        );

        let without_error = bundle_swap_message("the bundle is gone", None);
        assert!(
            without_error.starts_with("The update reported success but"),
            "an Ok install that landed nothing must say so: {without_error}",
        );
        assert!(
            !without_error.contains("The installer reported"),
            "there is no installer error to quote on the Ok path: {without_error}",
        );
    }

    // The path resolution must agree with the plugin's, and the plugin's rule is
    // "walk up out of Contents/MacOS". Asserting it against a synthetic exe path
    // keeps the two from drifting without anyone noticing.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_plugin_resolves_a_bundle_exe_to_the_bundle_root() {
        let exe = Path::new("/Users/me/Applications/Lucidos.app").join(BUNDLE_MAIN_EXECUTABLE);
        let resolved = tauri_plugin_updater::extract_path_from_executable(&exe)
            .expect("a bundled exe resolves to its bundle");
        assert_eq!(
            resolved,
            Path::new("/Users/me/Applications/Lucidos.app"),
            "the check must inspect the bundle the plugin swapped, not its Contents dir",
        );
    }
}
