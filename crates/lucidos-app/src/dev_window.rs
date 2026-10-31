//! The development window, and how it follows a switch to a new version
//! (ADR 0461).
//!
//! `scripts/tauri-dev.sh` runs this binary from its published launch binary in
//! `.launch/`, and names the page to show in [`URL_ENV`]. The build that Apply
//! starts may publish a newer one at the same path. The window moves to it only
//! once the engine it shows has moved too, so it is never newer than the stack.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

/// The page the launcher wants the window on.
pub const URL_ENV: &str = "LUCIDOS_DEV_WINDOW_URL";

/// How long the window waits for its page's server before navigating anyway.
/// The launcher starts the server first, so this covers only a slow start.
const SERVER_WAIT: Duration = Duration::from_secs(30);
const SERVER_POLL: Duration = Duration::from_millis(300);

/// Which file a path named, told apart from any file published there later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

/// What is published at a path now: which file, and when it was published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Published {
    id: FileId,
    at: SystemTime,
}

/// Where the launcher wants the page to reopen on, when the window exits to
/// follow a switch. The launcher then starts the binary published at its path.
pub const RELAUNCH_FILE_ENV: &str = "LUCIDOS_DEV_WINDOW_RELAUNCH_FILE";

/// A window the launcher started: the binary it runs, the origin of the page it
/// was pointed at, and where to hand the launcher a relaunch.
struct LaunchedWindow {
    path: PathBuf,
    id: FileId,
    origin: String,
    relaunch_file: PathBuf,
}

/// Set once, by [`launch`], and only for a window the launcher started.
static LAUNCHED: OnceLock<LaunchedWindow> = OnceLock::new();

/// Point the window at the page the launcher named, once its server answers.
///
/// Without [`URL_ENV`] the window keeps `build.devUrl` and never follows a
/// switch, which is what a bare `cargo run` gets. The page's origin gets the
/// packaged gateway's IPC grant, because Tauri treats any origin but `devUrl`
/// as remote (ADR 0028).
pub fn launch(app: &AppHandle) {
    let Ok(url) = std::env::var(URL_ENV) else {
        return;
    };
    let parsed = match url.parse::<tauri::Url>() {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("[dev-window] {URL_ENV} is not a URL ({url}): {e}");
            return;
        }
    };
    let origin = parsed.origin().ascii_serialization();
    if let (Some((path, id)), Some(relaunch_file)) =
        (read_running_image(), std::env::var_os(RELAUNCH_FILE_ENV))
    {
        let _ = LAUNCHED.set(LaunchedWindow {
            path,
            id,
            origin: origin.clone(),
            relaunch_file: relaunch_file.into(),
        });
    }
    if let Err(e) = app.add_capability(crate::desktop::gateway_capability(&origin)) {
        eprintln!(
            "[dev-window] FAILED to grant {origin} its IPC permissions; every Tauri IPC call \
             from the window will be rejected: {e}"
        );
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        if !wait_until_listening(&parsed, SERVER_WAIT) {
            eprintln!("[dev-window] nothing answers at {url} yet; showing it anyway");
        }
        crate::desktop::navigate_main_window(&handle, &url);
    });
}

/// Move onto the window binary now published at this one's path, when the
/// engine the page shows started after it was published. Returns whether it
/// did; on success it never returns, since the window exits.
///
/// The page calls this with its engine's `started_at` on its first health
/// answer and on every restart, so a switch from any device reaches it. Only a
/// page on the launcher's origin counts. The launcher starts the new binary on
/// the page the window showed, so the user stays where they were.
///
/// It never execs in place. macOS registers a self-exec'd app again with pid
/// -1, and window managers such as Rectangle then cannot find the window.
#[tauri::command]
pub fn follow_engine_switch(
    webview: tauri::Webview,
    engine_started_at_ms: u64,
) -> Result<bool, String> {
    let Some(window) = LAUNCHED.get() else {
        return Ok(false);
    };
    let page = webview
        .url()
        .map_err(|e| format!("cannot read the page URL: {e}"))?;
    if !is_on_origin(&page, &window.origin) {
        return Ok(false);
    }
    let engine_started_at = UNIX_EPOCH + Duration::from_millis(engine_started_at_ms);
    if !should_follow(window.id, read_published(&window.path), engine_started_at) {
        return Ok(false);
    }
    eprintln!(
        "[dev-window] the engine moved to a new version; relaunching {} on {page}",
        window.path.display()
    );
    std::fs::write(&window.relaunch_file, page.as_str()).map_err(|e| {
        format!(
            "cannot hand the launcher the relaunch at {}: {e}",
            window.relaunch_file.display()
        )
    })?;
    // The same exit as restart_app, so the new window keeps this one's frame.
    let app = webview.app_handle();
    crate::window_persist::persist_windows(app);
    crate::window_persist::begin_teardown();
    app.cleanup_before_exit();
    std::process::exit(0);
}

/// Whether the window should move onto what is published at its path.
///
/// Two things must hold. The file there is not the one this process runs, and
/// the engine started after it was published. A switch publishes nothing, so
/// an engine that started later already runs that build.
fn should_follow(
    running: FileId,
    published: Option<Published>,
    engine_started_at: SystemTime,
) -> bool {
    match published {
        Some(published) => published.id != running && engine_started_at > published.at,
        None => false,
    }
}

/// Whether `page` is on the launcher's origin. The window opens on `devUrl`
/// before it navigates, and that page may be another workspace's engine.
fn is_on_origin(page: &tauri::Url, origin: &str) -> bool {
    page.origin().ascii_serialization() == origin
}

fn read_running_image() -> Option<(PathBuf, FileId)> {
    let path = std::env::current_exe().ok()?;
    let id = read_published(&path)?.id;
    Some((path, id))
}

#[cfg(unix)]
fn read_published(path: &Path) -> Option<Published> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(Published {
        id: FileId {
            dev: meta.dev(),
            ino: meta.ino(),
        },
        at: meta.modified().ok()?,
    })
}

#[cfg(not(unix))]
fn read_published(_path: &Path) -> Option<Published> {
    None
}

/// Wait until something accepts a TCP connection at the URL's host and port.
fn wait_until_listening(url: &tauri::Url, timeout: Duration) -> bool {
    let addrs: Vec<SocketAddr> = match (url.host_str(), url.port_or_known_default()) {
        (Some(host), Some(port)) => (host, port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if addrs
            .iter()
            .any(|addr| TcpStream::connect_timeout(addr, SERVER_POLL).is_ok())
        {
            return true;
        }
        std::thread::sleep(SERVER_POLL);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: FileId = FileId { dev: 1, ino: 10 };
    const NEWER: FileId = FileId { dev: 1, ino: 11 };

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn published(id: FileId, secs: u64) -> Option<Published> {
        Some(Published { id, at: at(secs) })
    }

    #[test]
    fn follows_a_new_binary_once_the_engine_started_after_it() {
        assert!(should_follow(RUNNING, published(NEWER, 100), at(101)));
    }

    #[test]
    fn waits_while_the_engine_predates_the_new_binary() {
        // The build published a window, and nobody has switched yet.
        assert!(!should_follow(RUNNING, published(NEWER, 100), at(99)));
        assert!(!should_follow(RUNNING, published(NEWER, 100), at(100)));
    }

    #[test]
    fn stays_when_the_published_binary_is_the_running_one() {
        assert!(!should_follow(RUNNING, published(RUNNING, 100), at(500)));
    }

    #[test]
    fn stays_when_nothing_can_be_read_at_the_path() {
        assert!(!should_follow(RUNNING, None, at(500)));
    }

    #[test]
    fn only_a_page_on_the_launchers_origin_counts() {
        let origin = "https://localhost:5251";
        let workspace: tauri::Url = "https://localhost:5251/dev/".parse().unwrap();
        let other_workspace: tauri::Url = "https://localhost:5251/myws/".parse().unwrap();
        let dev_url: tauri::Url = "http://localhost:5173/".parse().unwrap();
        assert!(is_on_origin(&workspace, origin));
        assert!(is_on_origin(&other_workspace, origin));
        assert!(!is_on_origin(&dev_url, origin));
    }

    /// The launcher sets the variables this module reads. Shell cannot import
    /// the names, so the scripts are pinned to them here.
    #[test]
    fn the_scripts_name_the_variables_the_window_reads() {
        let launcher = include_str!("../../../scripts/lib/workspace.sh");
        for (script, source, name) in [
            ("workspace.sh", launcher, URL_ENV),
            ("workspace.sh", launcher, RELAUNCH_FILE_ENV),
            (
                "status.sh",
                include_str!("../../../scripts/status.sh"),
                URL_ENV,
            ),
        ] {
            assert!(source.contains(name), "{script} no longer names {name}");
        }
    }

    #[test]
    fn reads_a_republished_file_as_a_different_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lucidos-app");
        std::fs::write(&path, b"old").unwrap();
        let before = read_published(&path).unwrap();
        // The publish writes a temp file and renames it over the old one.
        let tmp = dir.path().join("lucidos-app.tmp");
        std::fs::write(&tmp, b"new").unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        let after = read_published(&path).unwrap();
        assert_ne!(before.id, after.id);
    }

    #[test]
    fn waits_no_longer_than_its_timeout_for_a_silent_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url: tauri::Url = format!("http://127.0.0.1:{port}/dev/").parse().unwrap();
        assert!(wait_until_listening(&url, Duration::from_secs(1)));
        drop(listener);
        let started = Instant::now();
        assert!(!wait_until_listening(&url, Duration::from_millis(400)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
