//! Where the packaged client's own diagnostics go.
//!
//! LaunchServices hands a launched app a write-only sink for fd 1 and fd 2, and
//! nothing written there reaches the unified log. So every `eprintln!` in the
//! client role was discarded, the updater's account of a relaunch that never
//! happened included. The service and login agents take their sinks from their
//! plists. The client has no plist, so it opens its own.
//!
//! Development is untouched. It runs from a terminal, where stderr already goes
//! somewhere useful, and it shares the packaged app-data dir, which is not its
//! to write into.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::desktop;

/// The client's combined stdout and stderr, under `<app-data>/logs/`.
///
/// One file, rather than the `.out` / `.err` pair the plists give the two
/// agents. The interleaving is the point: the `open` a relaunch watcher could
/// not complete has to read directly after the updater line that preceded it.
const CLIENT_LOG: &str = "client.log";

/// The single kept generation, rotated to at start.
const CLIENT_LOG_PREVIOUS: &str = "client.log.1";

/// The size at which a start rotates. Generous for a file only written on a
/// fault, and small enough that two generations stay a rounding error beside
/// the install. A log with no cap at all is not theoretical: the service's own
/// `engine-service.out.log` reached 579 MB on one machine.
const CLIENT_LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Point this process's stdout and stderr at `<app-data>/logs/client.log`, and
/// mark the start of a launch in it.
///
/// Called as the first statement of [`crate::run`], so everything after it is
/// captured. Best-effort by design: a client that cannot open its log still has
/// to start.
pub(crate) fn install() {
    if tauri::is_dev() {
        return;
    }
    if is_service_role(std::env::args_os()) {
        return;
    }
    if let Err(e) = redirect_to_app_data() {
        // Nowhere to report this, by construction: the sink we failed to open
        // is the one that would have carried it. A packaged build started from
        // a terminal still shows it.
        eprintln!("[client] no log file ({e}); diagnostics are being discarded");
        return;
    }
    eprintln!(
        "[client] launch starting (pid {}, v{})",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );
}

/// Is this process the service rather than the client?
///
/// The service's plist owns its sink, and `parse_service_boots` reads that file
/// back to tell a crash loop from a slow boot. Redirecting it here would empty
/// the file the report is built from. `main` routes `--service` before
/// [`crate::run`], and this is the belt for that ordering.
fn is_service_role(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    args.into_iter()
        .skip(1)
        .any(|a| a == OsStr::new("--service"))
}

/// Open the log, rotating an oversized one first, and take over both streams.
fn redirect_to_app_data() -> io::Result<()> {
    let logs = desktop::app_data_dir_from_env()?.join("logs");
    std::fs::create_dir_all(&logs)?;
    let log = logs.join(CLIENT_LOG);
    rotate_if_large(&log, &logs.join(CLIENT_LOG_PREVIOUS), CLIENT_LOG_MAX_BYTES)?;
    redirect_std_streams(&log)
}

/// Has the log earned a rotation? Pure, so the boundary is tested rather than
/// reasoned about.
fn should_rotate(size: u64, cap: u64) -> bool {
    size > cap
}

/// Move an oversized `log` onto `previous`, replacing whatever was there.
///
/// Exactly one generation, so the pair is bounded at twice the cap. A log that
/// is not there yet is the first launch, not a failure.
fn rotate_if_large(log: &Path, previous: &Path, cap: u64) -> io::Result<()> {
    let Ok(meta) = std::fs::metadata(log) else {
        return Ok(());
    };
    if !should_rotate(meta.len(), cap) {
        return Ok(());
    }
    std::fs::rename(log, previous)
}

/// Point fd 1 and fd 2 at `path`.
#[cfg(unix)]
fn redirect_std_streams(path: &Path) -> io::Result<()> {
    redirect_fds(path, &[libc::STDOUT_FILENO, libc::STDERR_FILENO])
}

/// Owner-only, for a file nobody else has any business reading.
///
/// The app-data dir itself is world-readable, and the client's diagnostics
/// carry what it was doing: workspace URLs, and on one preview error path an
/// external URL the user opened. All of that used to be discarded. Keeping the
/// file to its owner is what stops this widening what a local account can see.
/// It is also the file a user is asked to send for support.
#[cfg(unix)]
const CLIENT_LOG_MODE: u32 = 0o600;

/// Point every descriptor in `targets` at `path`, appending.
///
/// Appending rather than truncating, because a relaunch is exactly the moment
/// the previous launch's last lines matter. `targets` is a parameter so a test
/// can prove the mechanism against scratch descriptors, rather than seizing the
/// harness's own streams mid-run.
#[cfg(unix)]
fn redirect_fds(path: &Path, targets: &[i32]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(CLIENT_LOG_MODE)
        .open(path)?;
    let fd = file.as_raw_fd();
    for target in targets {
        // SAFETY: `dup2` takes two descriptors and no pointers. `fd` is open for
        // the whole call, and the caller owns every target. The duplicates
        // outlive `file`, which is what makes dropping it below safe.
        if unsafe { libc::dup2(fd, *target) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// macOS is the only packaged client Lucidos ships, and its sink is the reason
/// this module exists. Anywhere else the streams are left alone.
#[cfg(not(unix))]
fn redirect_std_streams(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::ffi::OsString;

    fn args(words: &[&str]) -> Vec<OsString> {
        std::iter::once(OsString::from(
            "/Applications/Lucidos.app/Contents/MacOS/lucidos-app",
        ))
        .chain(words.iter().map(OsString::from))
        .collect()
    }

    // The service reads its own error log back to report a crash loop. A
    // redirect here would empty the file that report is built from.
    #[test]
    fn the_service_role_keeps_the_sink_its_plist_gave_it() {
        assert!(is_service_role(args(&["--service"])));
        assert!(is_service_role(args(&["--other", "--service"])));
    }

    #[test]
    fn every_other_launch_is_a_client_and_gets_the_log() {
        assert!(!is_service_role(args(&[])));
        assert!(!is_service_role(args(&["--login"])));
    }

    // argv[0] is the executable, and a bundle could live under a directory
    // called `--service`. Skipping it is what keeps that from silencing a real
    // client.
    #[test]
    fn the_executable_path_is_never_read_as_a_flag() {
        assert!(!is_service_role(vec![OsString::from("--service")]));
    }

    #[test]
    fn rotation_starts_above_the_cap_and_not_at_it() {
        assert!(!should_rotate(0, 10));
        assert!(!should_rotate(10, 10));
        assert!(should_rotate(11, 10));
    }

    #[test]
    fn an_oversized_log_is_rotated_to_one_generation() {
        let tmp = TempDir::new("client-log-rotate");
        let log = tmp.path().join(CLIENT_LOG);
        let previous = tmp.path().join(CLIENT_LOG_PREVIOUS);

        std::fs::write(&log, b"first").expect("write the log");
        rotate_if_large(&log, &previous, 1).expect("rotate");
        assert!(!log.exists(), "the oversized log moves aside");
        assert_eq!(std::fs::read(&previous).expect("read"), b"first");
    }

    // Two generations, never three. A client that faults on every start would
    // otherwise leave a directory of them.
    #[test]
    fn a_second_rotation_replaces_the_kept_generation() {
        let tmp = TempDir::new("client-log-second");
        let log = tmp.path().join(CLIENT_LOG);
        let previous = tmp.path().join(CLIENT_LOG_PREVIOUS);

        std::fs::write(&log, b"first").expect("write the log");
        rotate_if_large(&log, &previous, 1).expect("rotate once");
        std::fs::write(&log, b"second").expect("write the log again");
        rotate_if_large(&log, &previous, 1).expect("rotate twice");

        assert_eq!(std::fs::read(&previous).expect("read"), b"second");
        let kept: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("list")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(kept.len(), 1, "one generation, plus nothing else");
    }

    #[test]
    fn a_log_under_the_cap_is_left_alone() {
        let tmp = TempDir::new("client-log-small");
        let log = tmp.path().join(CLIENT_LOG);
        std::fs::write(&log, b"small").expect("write the log");
        rotate_if_large(&log, &tmp.path().join(CLIENT_LOG_PREVIOUS), 1024).expect("no rotation");
        assert_eq!(std::fs::read(&log).expect("read"), b"small");
    }

    // The first launch has no log yet, which is not a failure to report.
    #[test]
    fn a_missing_log_is_the_first_launch() {
        let tmp = TempDir::new("client-log-absent");
        rotate_if_large(
            &tmp.path().join(CLIENT_LOG),
            &tmp.path().join(CLIENT_LOG_PREVIOUS),
            1,
        )
        .expect("a missing log rotates to nothing");
    }

    // Both streams, one file, in the order they were written. Proven against
    // scratch descriptors: seizing the real 1 and 2 would swallow whatever the
    // harness printed during the window.
    #[cfg(unix)]
    #[test]
    fn the_redirect_sends_every_stream_to_one_file_in_order() {
        let tmp = TempDir::new("client-log-redirect");
        let path = tmp.path().join(CLIENT_LOG);
        // SAFETY: `dup` takes one descriptor and no pointers. fd 1 is open for
        // the life of the process, and the copies are closed below.
        let (first, second) = unsafe { (libc::dup(1), libc::dup(1)) };
        assert!(first > 0 && second > 0, "the scratch descriptors opened");

        redirect_fds(&path, &[first, second]).expect("redirect");
        for (fd, line) in [(first, "from the first\n"), (second, "from the second\n")] {
            // SAFETY: `fd` is a descriptor this test owns and `line` is a live
            // slice of exactly `len` bytes.
            unsafe { libc::write(fd, line.as_ptr().cast(), line.len()) };
        }
        // SAFETY: both descriptors are this test's own and still open.
        unsafe {
            libc::close(first);
            libc::close(second);
        }

        let written = std::fs::read_to_string(&path).expect("read the log");
        assert_eq!(written, "from the first\nfrom the second\n");
    }

    // The app-data dir is world-readable, and this file now keeps what the
    // client was doing. A support request hands it to somebody; a local account
    // must not be able to read it without being asked.
    #[cfg(unix)]
    #[test]
    fn the_log_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new("client-log-mode");
        let path = tmp.path().join(CLIENT_LOG);
        // SAFETY: `dup` takes one descriptor and no pointers, and fd 1 is open.
        let scratch = unsafe { libc::dup(1) };
        redirect_fds(&path, &[scratch]).expect("redirect");
        // SAFETY: the descriptor is this test's own and still open.
        unsafe { libc::close(scratch) };

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, CLIENT_LOG_MODE, "got {:o}", mode & 0o777);
    }
}
