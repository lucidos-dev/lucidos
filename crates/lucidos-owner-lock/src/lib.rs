//! The *owner lock*: an exclusive flock a process holds for its whole life.
//!
//! A pidfile says who owned something once. The kernel drops a flock the
//! moment its holder dies, so a held owner lock says who owns it now. Checking
//! ownership and taking it are one call, which is what a pidfile check followed
//! by a spawn never was. Why: ADR 0438.
//!
//! Three rules keep it sound:
//!
//! - **Never delete a lock file.** A second opener would create a new inode and
//!   lock that one, so two processes would each hold "the" lock.
//! - **Only the holder publishes its pid** ([`OwnerLock::publish_pid`]). A
//!   reader trusts the pid only while the lock is held ([`live_pid`]).
//! - **A child keeps the lock only until it execs.** Rust opens every file
//!   close-on-exec, so a subprocess that outlives its parent cannot keep the
//!   parent "alive". Between fork and exec, though, the child holds a copy. A
//!   freshly copied binary can take macOS a while to exec, so a release is
//!   observed promptly, not instantly. A lock is never observed free early.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::Path;

/// A held owner lock. Dropping it releases the lock.
#[derive(Debug)]
pub struct OwnerLock {
    _file: File,
}

impl OwnerLock {
    /// Take the lock, waiting for the current holder to exit.
    pub fn acquire(path: &Path) -> std::io::Result<OwnerLock> {
        let file = open_lock_file(path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(OwnerLock { _file: file })
    }

    /// Take the lock if nobody holds it. `None` means another process does.
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<OwnerLock>> {
        let file = open_lock_file(path)?;
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(OwnerLock { _file: file })),
            Err(e) if is_contended(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Take the lock through a file this process already has open, such as one
    /// inherited across `exec`. An open file that already holds the lock takes
    /// it again at once, whatever other copies of the descriptor exist.
    /// `None` means another open file holds it.
    pub fn adopt(file: File) -> std::io::Result<Option<OwnerLock>> {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(OwnerLock { _file: file })),
            Err(e) if is_contended(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Write this process's pid to `pid_path`, whole or not at all.
    ///
    /// A temp file renamed into place, so a reader never sees a partial pid.
    /// Taking `&self` is the point: only the holder can name itself.
    pub fn publish_pid(&self, pid_path: &Path) -> std::io::Result<()> {
        let name = pid_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "pid path has no name"))?;
        let tmp = pid_path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        let mut out = File::create(&tmp)?;
        out.write_all(std::process::id().to_string().as_bytes())?;
        out.sync_all()?;
        std::fs::rename(&tmp, pid_path)
    }
}

#[cfg(unix)]
impl std::os::fd::AsRawFd for OwnerLock {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self._file.as_raw_fd()
    }
}

/// Whether some process holds the lock at `path` right now.
///
/// A missing lock file is "not held": nobody ever took it. The probe briefly
/// takes a SHARED lock, so it can make a concurrent [`OwnerLock::try_acquire`]
/// fail. Probe only locks whose owner claims with [`OwnerLock::acquire`].
pub fn is_held(path: &Path) -> std::io::Result<bool> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    match fs2::FileExt::try_lock_shared(&file) {
        Ok(()) => Ok(false),
        Err(e) if is_contended(&e) => Ok(true),
        Err(e) => Err(e),
    }
}

/// The pid at `pid_path`, but only while the lock at `lock_path` is held.
///
/// `None` covers every reason not to signal: no holder, no pidfile, or a file
/// that does not parse. A probe error also answers `None`, since a pid nobody
/// can vouch for must never be signalled.
pub fn live_pid(lock_path: &Path, pid_path: &Path) -> Option<u32> {
    if !is_held(lock_path).ok()? {
        return None;
    }
    read_pid(pid_path)
}

/// The pid a pidfile holds, with no claim that it is alive.
fn read_pid(pid_path: &Path) -> Option<u32> {
    std::fs::read_to_string(pid_path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|&pid| pid > 1)
}

/// A *consumer lock*: a shared flock a consumer of something holds for its
/// whole life.
///
/// Many consumers hold it at once. Whoever would take the shared thing away
/// asks with one exclusive, non-blocking try. Granted means no consumer is
/// left, and none can register until it lets go. So a consumer arriving cannot
/// split the decision from the act.
#[derive(Debug)]
pub struct ConsumerLock {
    _file: File,
}

impl ConsumerLock {
    /// Register as a consumer, waiting out a teardown that holds the lock.
    pub fn acquire(path: &Path) -> std::io::Result<ConsumerLock> {
        let file = open_lock_file(path)?;
        fs2::FileExt::lock_shared(&file)?;
        Ok(ConsumerLock { _file: file })
    }
}

/// A workspace engine's lock and pidfile, inside its workspace directory.
pub mod engine {
    use std::path::{Path, PathBuf};

    pub fn lock_path(workspace: &Path) -> PathBuf {
        workspace.join(".lucidos/engine.lock")
    }

    pub fn pid_path(workspace: &Path) -> PathBuf {
        workspace.join(".lucidos/engine.pid")
    }

    /// Held by an engine that is waiting for [`lock_path`], so a stop can find
    /// a successor before it owns the workspace.
    pub fn successor_lock_path(workspace: &Path) -> PathBuf {
        workspace.join(".lucidos/engine.successor.lock")
    }

    pub fn successor_pid_path(workspace: &Path) -> PathBuf {
        workspace.join(".lucidos/engine.successor.pid")
    }

    /// The pid of an engine waiting to own this workspace, while it waits.
    pub fn waiting_pid(workspace: &Path) -> Option<u32> {
        super::live_pid(
            &successor_lock_path(workspace),
            &successor_pid_path(workspace),
        )
    }

    /// The engine's pid, while an engine owns this workspace.
    ///
    /// No lock file means no lock-aware engine has ever run here, so the
    /// pidfile still follows the old contract: a launcher wrote it. Its pid is
    /// returned unvouched, and the caller's own liveness and executable checks
    /// guard it. Temporary measure: `docs/temporary-measures.md` § "Pre-lock
    /// engine pidfile fallback".
    pub fn live_pid(workspace: &Path) -> Option<u32> {
        let lock = lock_path(workspace);
        if !lock.exists() {
            return super::read_pid(&pid_path(workspace));
        }
        super::live_pid(&lock, &pid_path(workspace))
    }
}

/// A gateway's lock and pidfile, inside its data directory.
pub mod gateway {
    use std::path::{Path, PathBuf};

    pub fn lock_path(data_dir: &Path) -> PathBuf {
        data_dir.join("gateway.lock")
    }

    pub fn pid_path(data_dir: &Path) -> PathBuf {
        data_dir.join("gateway.pid")
    }
}

/// The packaged desktop client's lock and pidfile, inside its app-data dir.
///
/// The client claims with [`super::OwnerLock::try_acquire`], so never probe
/// this lock with [`super::is_held`]. The probe can make a starting client
/// lose its try and refuse to start.
pub mod client {
    use std::path::{Path, PathBuf};

    pub fn lock_path(app_data: &Path) -> PathBuf {
        app_data.join("client.lock")
    }

    pub fn pid_path(app_data: &Path) -> PathBuf {
        app_data.join("client.pid")
    }

    /// The pid the holder published, for a process that just lost the try.
    ///
    /// Unvouched: a holder that has not published yet leaves the previous
    /// client's pid. Use it to name the holder, never to signal it.
    pub fn published_pid(app_data: &Path) -> Option<u32> {
        super::read_pid(&pid_path(app_data))
    }
}

/// A checkout's shared build-watch, under `<app>/.build-watch/`, where `<app>`
/// is the directory holding the served `dist/`. The shell side is
/// `start_frontend_built` in `scripts/lib/workspace.sh`.
pub mod build_watch {
    use std::path::{Path, PathBuf};

    /// The watch script. Its presence beside `dist/` marks a dev checkout.
    pub const SCRIPT: &str = "dev-build-watch.mjs";

    pub fn dir(app: &Path) -> PathBuf {
        app.join(".build-watch")
    }

    /// The watcher's owner lock. The watcher takes it blocking, so probing it
    /// with [`super::is_held`] is sound.
    pub fn watch_lock_path(app: &Path) -> PathBuf {
        dir(app).join("watch.lock")
    }

    pub fn pid_path(app: &Path) -> PathBuf {
        dir(app).join("pid")
    }

    /// The [`super::ConsumerLock`] every engine serving `<app>/dist` holds.
    pub fn consumers_lock_path(app: &Path) -> PathBuf {
        dir(app).join("consumers.lock")
    }

    /// The watcher's pid, while a watcher runs.
    pub fn live_pid(app: &Path) -> Option<u32> {
        super::live_pid(&watch_lock_path(app), &pid_path(app))
    }
}

fn open_lock_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn is_contended(e: &std::io::Error) -> bool {
    e.kind() == fs2::lock_contended_error().kind()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_held_lock_refuses_a_second_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("engine.lock");
        let first = OwnerLock::try_acquire(&path).unwrap().expect("free lock");
        assert!(OwnerLock::try_acquire(&path).unwrap().is_none());
        assert!(is_held(&path).unwrap());
        drop(first);
        assert!(!is_held(&path).unwrap());
        assert!(OwnerLock::try_acquire(&path).unwrap().is_some());
    }

    /// The successor's whole contract: it gets the lock exactly when the
    /// predecessor lets go, and not before. Synchronised by channels and the
    /// lock itself, never by a sleep.
    #[test]
    fn acquire_waits_for_the_holder_to_let_go() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("engine.lock");
        let pid_path = dir.join("engine.pid");
        let predecessor = OwnerLock::acquire(&path).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let successor = {
            let path = path.clone();
            let pid_path = pid_path.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let lock = OwnerLock::acquire(&path).unwrap();
                lock.publish_pid(&pid_path).unwrap();
                lock
            })
        };
        started_rx.recv().unwrap();
        assert!(
            OwnerLock::try_acquire(&path).unwrap().is_none(),
            "the predecessor still owns the workspace"
        );
        assert!(
            !pid_path.exists(),
            "the successor published before it owned"
        );
        drop(predecessor);
        let lock = successor.join().unwrap();
        assert_eq!(live_pid(&path, &pid_path), Some(std::process::id()));
        drop(lock);
    }

    #[test]
    fn a_pid_with_no_holder_is_not_live() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("engine.lock");
        let pid_path = dir.join("engine.pid");
        let lock = OwnerLock::acquire(&path).unwrap();
        lock.publish_pid(&pid_path).unwrap();
        assert_eq!(live_pid(&path, &pid_path), Some(std::process::id()));
        drop(lock);
        assert!(pid_path.exists(), "the pidfile outlives its holder");
        assert_eq!(live_pid(&path, &pid_path), None);
    }

    /// An engine from before the lock wrote no lock file, and is still
    /// stoppable by its pid. Once any lock-aware engine has run, the lock
    /// decides.
    #[test]
    fn a_workspace_no_lock_aware_engine_ran_in_keeps_its_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
        std::fs::write(engine::pid_path(dir), "4242").unwrap();
        assert_eq!(engine::live_pid(dir), Some(4242));
        drop(OwnerLock::acquire(&engine::lock_path(dir)).unwrap());
        assert_eq!(engine::live_pid(dir), None);
    }

    #[test]
    fn a_missing_lock_file_is_not_held() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(!is_held(&dir.join("never.lock")).unwrap());
        assert_eq!(
            live_pid(&dir.join("never.lock"), &dir.join("never.pid")),
            None
        );
    }

    /// A teardown's exclusive try is refused while any consumer is registered,
    /// and a consumer arriving during a teardown waits for it to finish.
    #[test]
    fn consumers_hold_off_an_exclusive_try_and_wait_out_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("consumers.lock");
        let first = ConsumerLock::acquire(&path).unwrap();
        let second = ConsumerLock::acquire(&path).unwrap();
        assert!(OwnerLock::try_acquire(&path).unwrap().is_none());
        drop(first);
        assert!(OwnerLock::try_acquire(&path).unwrap().is_none());
        drop(second);

        let teardown = OwnerLock::try_acquire(&path)
            .unwrap()
            .expect("no consumer left");
        let (started_tx, started_rx) = mpsc::channel();
        let arriving = {
            let path = path.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                ConsumerLock::acquire(&path).unwrap()
            })
        };
        started_rx.recv().unwrap();
        assert!(
            !arriving.is_finished(),
            "a consumer registered under a held teardown"
        );
        drop(teardown);
        let consumer = arriving.join().unwrap();
        assert!(OwnerLock::try_acquire(&path).unwrap().is_none());
        drop(consumer);
    }

    #[test]
    fn the_build_watch_pid_is_live_only_while_the_watch_lock_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();
        let lock = OwnerLock::acquire(&build_watch::watch_lock_path(app)).unwrap();
        lock.publish_pid(&build_watch::pid_path(app)).unwrap();
        assert_eq!(build_watch::live_pid(app), Some(std::process::id()));
        drop(lock);
        assert_eq!(build_watch::live_pid(app), None);
    }

    #[test]
    fn a_second_client_loses_the_try_and_can_name_the_first() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path();
        let first = OwnerLock::try_acquire(&client::lock_path(app_data))
            .unwrap()
            .expect("free lock");
        first.publish_pid(&client::pid_path(app_data)).unwrap();
        assert!(OwnerLock::try_acquire(&client::lock_path(app_data))
            .unwrap()
            .is_none());
        assert_eq!(client::published_pid(app_data), Some(std::process::id()));
        drop(first);
        assert!(OwnerLock::try_acquire(&client::lock_path(app_data))
            .unwrap()
            .is_some());
    }

    #[test]
    fn publishing_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let lock = OwnerLock::acquire(&dir.join("x.lock")).unwrap();
        lock.publish_pid(&dir.join("x.pid")).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["x.lock", "x.pid"]);
        drop(lock);
    }
}
