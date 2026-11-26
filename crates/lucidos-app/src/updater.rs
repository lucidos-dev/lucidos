use lucidos_update_channel::{
    check_bundle_url, is_upgrade, Channel, UpdateAddress, GATEWAY_STATUS_PATH,
};
use serde::Serialize;
use serde_json::Value;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Runtime, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

const PROGRESS_EVENT: &str = "app-update-progress";

const PROGRESS_BYTE_STEP: u64 = 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum AppUpdatePhase {
    Checking,
    Downloading { downloaded: u64, total: Option<u64> },
    Verifying,
    Installing,
    RestartingServices,
    Relaunching,
    Cancelled,
    Failed { message: String },
    BundleSwapFailed { message: String },
}

#[derive(Clone, Serialize)]
struct AppUpdateProgress {
    version: Option<String>,
    #[serde(flatten)]
    phase: AppUpdatePhase,
}

#[derive(Clone)]
struct Narrator {
    app: AppHandle,
    relay: Option<crate::update_relay::RelayTag>,
}

impl Narrator {
    fn emit(&self, version: Option<&str>, phase: AppUpdatePhase) {
        let frame = AppUpdateProgress {
            version: version.map(str::to_string),
            phase,
        };
        if let Some(relay) = &self.relay {
            relay.publish(&frame);
        }
        let _ = self.app.emit(PROGRESS_EVENT, frame);
    }

    fn fail_as(
        &self,
        version: Option<&str>,
        phase: fn(String) -> AppUpdatePhase,
        message: String,
    ) -> String {
        self.emit(version, phase(message.clone()));
        message
    }

    fn fail(&self, version: Option<&str>, message: String) -> String {
        self.fail_as(
            version,
            |message| AppUpdatePhase::Failed { message },
            message,
        )
    }
}

struct UpdateFailure {
    version: Option<String>,
    message: String,
}

impl UpdateFailure {
    fn new(version: Option<String>, message: String) -> Self {
        Self { version, message }
    }
}

struct DownloadFrame {
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Default)]
struct DownloadTracker {
    downloaded: u64,
    total: Option<u64>,
    last_announced: Option<u64>,
}

impl DownloadTracker {
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

fn percent(bytes: u64, total: u64) -> u64 {
    bytes.saturating_mul(100) / total
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting,
    CancelPending,
    Downloading(JoinHandle<()>),
    Committed,
}

#[derive(Default)]
pub struct AppUpdateRun(Mutex<Phase>);

impl AppUpdateRun {
    fn begin(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Idle) {
            return false;
        }
        *phase = Phase::Starting;
        true
    }

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

    fn commit(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Downloading(_)) {
            return false;
        }
        *phase = Phase::Committed;
        true
    }

    fn release(&self) {
        *self.lock() = Phase::Idle;
    }

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

    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone, Serialize)]
pub struct AppUpdateOffer {
    version: String,
    notes: Option<String>,
}

#[derive(Debug, PartialEq)]
enum UpdateSource {
    Published,
    Address(UpdateAddress),
    Local(UpdateAddress),
}

async fn update_source() -> Result<UpdateSource, String> {
    let port = crate::desktop::engine_port();
    let status = tauri::async_runtime::spawn_blocking(move || {
        crate::desktop::gateway_body(port, "GET", GATEWAY_STATUS_PATH, None)
    })
    .await
    .map_err(|e| format!("update address lookup: {e}"))?;
    let source = resolve_update_source(status.as_deref());
    match &source {
        Ok(UpdateSource::Published) => eprintln!("[updater] checking the published release"),
        Ok(UpdateSource::Address(address) | UpdateSource::Local(address)) => {
            eprintln!("[updater] checking update channel at {address}")
        }
        Err(e) => eprintln!("[updater] {e}"),
    }
    source
}

fn resolve_update_source(status: Option<&str>) -> Result<UpdateSource, String> {
    let status = status.ok_or(
        "The update check did not run: the Lucidos gateway did not answer, \
         so the update address is unknown",
    )?;
    let status: Value = serde_json::from_str(status)
        .map_err(|e| format!("The update check did not run: unreadable gateway status, {e}"))?;
    let check = &status["release_check"];
    if let Some(refused) = check["channel_error"]
        .as_str()
        .or_else(|| check["update_address_error"].as_str())
    {
        return Err(format!("The update check did not run: {refused}"));
    }
    let channel = check
        .get("channel")
        .map(|raw| {
            raw.as_str()
                .ok_or_else(|| "The gateway reported an invalid update channel".to_string())
                .and_then(Channel::parse)
        })
        .transpose()?;
    if channel == Some(Channel::Nightly) {
        return Err("The Nightly channel is not published yet".into());
    }
    let local = channel == Some(Channel::Local);
    match check.get("update_address") {
        None | Some(Value::Null) if channel.as_ref().is_some_and(|c| c != &Channel::Stable) => {
            Err("The update check did not run: the channel has no feed address".into())
        }
        None | Some(Value::Null) => Ok(UpdateSource::Published),
        Some(Value::String(raw)) => match UpdateAddress::parse(raw) {
            Ok(Some(address)) if local => {
                if !matches!(
                    address.url().host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]")
                ) || address.url().path() != "/~/updates/local/latest.json"
                {
                    return Err("The Local channel must use the gateway's loopback feed".into());
                }
                Ok(UpdateSource::Local(address))
            }
            Ok(Some(address)) => Ok(UpdateSource::Address(address)),
            Ok(None) if channel.as_ref().is_some_and(|c| c != &Channel::Stable) => {
                Err("The update check did not run: the channel has no feed address".into())
            }
            Ok(None) => Ok(UpdateSource::Published),
            Err(refused) => Err(format!("The update check did not run: {refused}")),
        },
        Some(other) => Err(format!(
            "The update check did not run: the gateway reported update_address {other}"
        )),
    }
}

struct ChannelUpdater {
    updater: Updater,
    offered: Arc<Mutex<Option<String>>>,
    local: Option<UpdateAddress>,
}

fn updater_for<R: Runtime>(
    app: &AppHandle<R>,
    source: &UpdateSource,
) -> Result<ChannelUpdater, String> {
    updater_with_token(
        app,
        source,
        lucidos_local_token::read,
        lucidos_local_token::loopback_cert::read,
    )
}

fn updater_with_token<R: Runtime>(
    app: &AppHandle<R>,
    source: &UpdateSource,
    read_token: impl FnOnce() -> Option<String>,
    read_loopback_ca: impl FnOnce() -> std::io::Result<String>,
) -> Result<ChannelUpdater, String> {
    let offered = Arc::new(Mutex::new(None));
    let observed = offered.clone();
    let builder = app
        .updater_builder()
        .version_comparator(move |_build_id, release| {
            let version = release.version.to_string();
            *observed.lock().unwrap() = Some(version.clone());
            is_upgrade(crate::LUCIDOS_RELEASE, &version)
        });
    let builder = match source {
        UpdateSource::Published => builder,
        UpdateSource::Address(address) | UpdateSource::Local(address) => builder
            .endpoints(vec![address.url().clone()])
            .map_err(|e| e.to_string())?,
    };
    let local = match source {
        UpdateSource::Local(address) => Some(address.clone()),
        _ => None,
    };
    let builder = if local.is_some() {
        let token = read_token().ok_or("The Local channel needs the machine-local token")?;
        let ca = read_loopback_ca()
            .and_then(|pem| {
                reqwest_updater::Certificate::from_pem(pem.as_bytes())
                    .map_err(std::io::Error::other)
            })
            .map_err(|e| format!("The Local channel cannot trust the gateway: {e}"))?;
        builder
            .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
            .map_err(|e| e.to_string())?
            .no_proxy()
            .configure_client(move |client| {
                client
                    .redirect(reqwest_updater::redirect::Policy::none())
                    .tls_certs_only([ca.clone()])
            })
    } else {
        builder
    };
    Ok(ChannelUpdater {
        updater: builder.build().map_err(|e| e.to_string())?,
        offered,
        local,
    })
}

async fn check_offer(updater: &ChannelUpdater) -> Result<Option<Update>, String> {
    *updater.offered.lock().unwrap() = None;
    let update = updater.updater.check().await.map_err(|e| e.to_string())?;
    if let Some(update) = &update {
        check_bundle_url(&update.download_url)?;
        if let Some(local) = &updater.local {
            if update.download_url.origin() != local.url().origin()
                || !update.download_url.path().starts_with("/~/updates/local/")
            {
                return Err("The Local bundle must stay on the gateway's Local routes".into());
            }
        }
    } else if let Some(version) = updater.offered.lock().unwrap().as_deref() {
        if is_upgrade(version, crate::LUCIDOS_RELEASE) {
            return Err(format!(
                "Lucidos {} is ahead of this channel's {version}; keeping the installed build",
                crate::LUCIDOS_RELEASE
            ));
        }
    }
    Ok(update)
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> Result<Option<AppUpdateOffer>, String> {
    if tauri::is_dev() {
        return Ok(None);
    }
    let updater = updater_for(&app, &update_source().await?)?;
    let update = check_offer(&updater).await?;
    Ok(update.map(|u| AppUpdateOffer {
        version: u.version.clone(),
        notes: u
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string),
    }))
}

async fn check_and_download(
    narrator: &Narrator,
    updater: ChannelUpdater,
) -> Result<(Update, Vec<u8>), UpdateFailure> {
    let update = check_offer(&updater)
        .await
        .map_err(|e| UpdateFailure::new(None, e))?
        .ok_or_else(|| UpdateFailure::new(None, "No update available".to_string()))?;
    let version = update.version.clone();

    narrator.emit(
        Some(&version),
        AppUpdatePhase::Downloading {
            downloaded: 0,
            total: None,
        },
    );

    let tracker = Mutex::new(DownloadTracker::default());
    let bytes = update
        .download(
            |chunk, total| {
                let frame = tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .chunk(chunk as u64, total);
                if let Some(frame) = frame {
                    narrator.emit(
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
            },
            || {
                let frame = tracker.lock().unwrap_or_else(|e| e.into_inner()).finish();
                if let Some(frame) = frame {
                    narrator.emit(
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
                narrator.emit(Some(&version), AppUpdatePhase::Verifying);
            },
        )
        .await
        .map_err(|e| UpdateFailure::new(Some(version.clone()), e.to_string()))?;

    Ok((update, bytes))
}

#[cfg(target_os = "macos")]
const BUNDLE_MAIN_EXECUTABLE: &str = concat!("Contents/MacOS/", env!("CARGO_PKG_NAME"));

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum BundleVerdict {
    Runnable,
    BundleMissing,
    ExecutableMissing,
    ExecutableNotExecutable,
}

#[cfg(target_os = "macos")]
impl BundleVerdict {
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
    if meta.permissions().mode() & 0o111 == 0 {
        return BundleVerdict::ExecutableNotExecutable;
    }
    BundleVerdict::Runnable
}

#[cfg(target_os = "macos")]
fn installed_bundle_fault() -> Option<String> {
    match crate::bundle_location::bundle_path() {
        Err(e) => Some(format!("Lucidos cannot tell where its own bundle is: {e}")),
        Ok(bundle) => installed_bundle_verdict(&bundle)
            .reason()
            .map(|reason| format!("no runnable app is at {}: {reason}", bundle.display())),
    }
}

#[cfg(not(target_os = "macos"))]
fn installed_bundle_fault() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
#[expect(
    clippy::disallowed_methods,
    reason = "names the directory the updater plugin stages into; creates nothing"
)]
fn update_blocker() -> Option<String> {
    use crate::bundle_location;

    let bundle = bundle_location::bundle_path().ok()?;
    let same_device = bundle_location::same_device(&bundle, &std::env::temp_dir()).unwrap_or(true);
    bundle_location::update_blocker(&bundle_location::current(&bundle), same_device)
}

#[cfg(not(target_os = "macos"))]
fn update_blocker() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn remote_install_blocker() -> Option<String> {
    use crate::bundle_location;

    let writable = bundle_location::bundle_path()
        .map(|bundle| bundle_location::writable_in_place(&bundle))
        .unwrap_or(false);
    bundle_location::remote_install_blocker(update_blocker(), writable)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn remote_install_blocker() -> Option<String> {
    None
}

fn run_blocker(
    relayed: bool,
    local: fn() -> Option<String>,
    remote: fn() -> Option<String>,
) -> Option<String> {
    if relayed {
        remote()
    } else {
        local()
    }
}

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

#[tauri::command]
pub async fn install_app_update_and_restart(
    app: AppHandle,
    run: State<'_, AppUpdateRun>,
) -> Result<(), String> {
    run_app_update(app, &run, None).await
}

pub(crate) async fn run_app_update(
    app: AppHandle,
    run: &AppUpdateRun,
    relay: Option<crate::update_relay::RelayTag>,
) -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Updates are only available in a packaged build".to_string());
    }
    let narrator = Narrator { app, relay };
    if !run.begin() {
        let busy = "An update is already in progress".to_string();
        if let Some(relay) = &narrator.relay {
            relay.publish(&AppUpdateProgress {
                version: None,
                phase: AppUpdatePhase::Failed {
                    message: busy.clone(),
                },
            });
        }
        return Err(busy);
    }

    narrator.emit(None, AppUpdatePhase::Checking);
    if let Some(blocker) = run_blocker(
        narrator.relay.is_some(),
        update_blocker,
        remote_install_blocker,
    ) {
        run.release();
        return Err(narrator.fail(None, blocker));
    }
    let updater = match update_source()
        .await
        .and_then(|source| updater_for(&narrator.app, &source))
    {
        Ok(updater) => updater,
        Err(e) => {
            run.release();
            return Err(narrator.fail(None, e.to_string()));
        }
    };

    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let emitter = narrator.clone();
    let task = tauri::async_runtime::spawn(async move {
        let result = check_and_download(&emitter, updater).await;
        let _ = tx.send(result).await;
    });
    run.armed(task);

    let Some(result) = rx.recv().await else {
        narrator.emit(None, AppUpdatePhase::Cancelled);
        return Ok(());
    };
    if !run.commit() {
        let version = result.ok().map(|(update, _)| update.version);
        narrator.emit(version.as_deref(), AppUpdatePhase::Cancelled);
        return Ok(());
    }

    let (update, bytes) = match result {
        Ok(downloaded) => downloaded,
        Err(e) => {
            run.release();
            return Err(narrator.fail(e.version.as_deref(), e.message));
        }
    };
    let version = update.version.clone();

    narrator.emit(Some(&version), AppUpdatePhase::Installing);
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    let outcome = match installed {
        Ok(outcome) => outcome,
        Err(e) => {
            run.release();
            return Err(narrator.fail(Some(&version), format!("install task: {e}")));
        }
    };
    if let Err(e) = outcome {
        run.release();
        let e = e.to_string();
        return Err(match installed_bundle_fault() {
            None => narrator.fail(Some(&version), e),
            Some(fault) => narrator.fail_as(
                Some(&version),
                |message| AppUpdatePhase::BundleSwapFailed { message },
                bundle_swap_message(&fault, Some(&e)),
            ),
        });
    }

    if let Some(fault) = installed_bundle_fault() {
        run.release();
        return Err(narrator.fail_as(
            Some(&version),
            |message| AppUpdatePhase::BundleSwapFailed { message },
            bundle_swap_message(&fault, None),
        ));
    }

    narrator.emit(Some(&version), AppUpdatePhase::RestartingServices);
    if let Err(e) = crate::desktop::restart_service() {
        eprintln!("[updater] background service restart failed: {e}");
    }
    narrator.emit(Some(&version), AppUpdatePhase::Relaunching);
    match crate::desktop::schedule_relaunch_after_exit() {
        Ok(()) => crate::exit_after_relaunch_scheduled(&narrator.app),
        Err(e) => {
            eprintln!("[updater] no LaunchServices relaunch ({e}); respawning directly");
            crate::client_owner_lock::release_client_owner_lock();
            narrator.app.restart()
        }
    }
}

#[tauri::command]
pub fn cancel_app_update(run: State<'_, AppUpdateRun>) {
    run.cancel();
}

#[cfg(test)]
#[path = "updater_address_tests.rs"]
mod address_tests;

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_final_byte_count_is_always_announced() {
        let size = 10 * 1024 * 1024;
        for total in [Some(size), None] {
            let frames = run_download(32 * 1024, total, size);
            let last = frames.last().expect("a download announces at least once");
            assert_eq!(last.downloaded, size, "total={total:?}");
        }
    }

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

    #[test]
    fn a_cancel_after_commit_is_refused() {
        let run = AppUpdateRun::default();
        run.begin();
        run.armed(tauri::async_runtime::spawn(async {}));
        assert!(run.commit());
        assert!(!run.cancel());
    }

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
        let tmp = tempfile::tempdir().unwrap();
        let bundle = write_bundle(tmp.path(), 0o755);
        assert_eq!(installed_bundle_verdict(&bundle), BundleVerdict::Runnable);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_vanished_bundle_is_reported_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Lucidos.app");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::BundleMissing
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_directory_without_its_main_binary_is_not_runnable() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Lucidos.app");
        std::fs::create_dir_all(bundle.join("Contents/Resources")).expect("create a hollow bundle");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableMissing
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_directory_standing_in_for_the_main_binary_is_not_runnable() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Lucidos.app");
        std::fs::create_dir_all(bundle.join(BUNDLE_MAIN_EXECUTABLE)).expect("create the stand-in");
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableMissing
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_main_binary_with_no_execute_bit_is_not_runnable() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = write_bundle(tmp.path(), 0o644);
        assert_eq!(
            installed_bundle_verdict(&bundle),
            BundleVerdict::ExecutableNotExecutable
        );
    }

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

    #[cfg(target_os = "macos")]
    #[test]
    fn the_verdict_follows_the_bundle_wherever_it_lives() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("Users/someone/Applications");
        std::fs::create_dir_all(&nested).expect("create a non-/Applications location");
        let bundle = write_bundle(&nested, 0o755);
        assert_eq!(installed_bundle_verdict(&bundle), BundleVerdict::Runnable);
    }

    #[test]
    fn a_relayed_run_takes_the_remote_blocker_and_a_click_the_local_one() {
        let local = || Some("local".to_string());
        let remote = || Some("needs an administrator password".to_string());
        assert_eq!(
            run_blocker(true, local, remote).as_deref(),
            Some("needs an administrator password")
        );
        assert_eq!(run_blocker(false, local, remote).as_deref(), Some("local"));
        assert_eq!(run_blocker(true, local, || None), None);
    }

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
