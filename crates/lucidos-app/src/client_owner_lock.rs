//! One packaged GUI client per app-data dir, held by the client's owner lock
//! (`lucidos_owner_lock::client`, ADR 0438). Why: ADR 0471.
//!
//! LaunchServices keeps a Finder, Dock or `open` launch of one bundle to one
//! client. A direct exec of `Contents/MacOS/lucidos-app`, or a second copy of
//! the bundle, bypasses it, and that second client adopts every workspace.
//!
//! The lock file is close-on-exec, which two relaunch paths depend on:
//! - `restart_process` execs in place, and the new image takes the lock again;
//! - a spawned relaunch watcher outlives the client, and must not keep the lock
//!   it would otherwise block the relaunch with.
//!
//! A relaunch that spawns the new client before exiting (Tauri's `restart`)
//! must call [`release_client_owner_lock`] first.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use lucidos_owner_lock::{client, OwnerLock};

/// Held until exit once [`claim_client_owner_lock`] takes it, unless
/// [`release_client_owner_lock`] gives it up first.
static CLIENT_OWNER_LOCK: Mutex<Option<OwnerLock>> = Mutex::new(None);

/// Take the client's owner lock, before any Tauri init.
///
/// `Err` is the refusal for stderr: another client holds the lock, and the
/// caller must exit. Dev builds take no lock. A lock that fails for any other
/// reason is reported and skipped, since a client must start even then.
pub fn claim_client_owner_lock() -> Result<(), String> {
    if tauri::is_dev() {
        return Ok(());
    }
    let app_data = match crate::desktop::app_data_dir_from_env() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[client] no owner lock ({e}); starting unguarded");
            return Ok(());
        }
    };
    match take(&app_data) {
        Ok(Some(lock)) => {
            *CLIENT_OWNER_LOCK
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(lock);
            Ok(())
        }
        Ok(None) => Err(refusal(client::published_pid(&app_data))),
        Err(e) => {
            eprintln!("[client] no owner lock ({e}); starting unguarded");
            Ok(())
        }
    }
}

/// Give up the lock ahead of a relaunch that starts the new client before
/// this one exits, as `AppHandle::restart` does on macOS. Without it the new
/// client finds the lock held and refuses to start.
pub(crate) fn release_client_owner_lock() {
    drop(
        CLIENT_OWNER_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(),
    );
}

/// Take the lock in `app_data` and publish this pid. `None` means another
/// client holds it.
fn take(app_data: &Path) -> std::io::Result<Option<OwnerLock>> {
    lucidos_home::ensure_storage_root(app_data)?;
    let Some(lock) = OwnerLock::try_acquire(&client::lock_path(app_data))? else {
        return Ok(None);
    };
    // The lock alone keeps the client single. The pid only names the holder
    // in a refusal, so failing to publish it must not cost the lock.
    if let Err(e) = lock.publish_pid(&client::pid_path(app_data)) {
        eprintln!("[client] owner lock held, but its pid was not published ({e})");
    }
    Ok(Some(lock))
}

fn refusal(holder: Option<u32>) -> String {
    let pid = holder.map_or_else(String::new, |p| format!(" (pid {p})"));
    format!("Lucidos is already running{pid}. Not starting a second client.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_client_is_refused_while_the_first_holds_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let first = take(tmp.path())
            .unwrap()
            .expect("first client takes the lock");
        assert!(take(tmp.path()).unwrap().is_none());
        assert_eq!(client::published_pid(tmp.path()), Some(std::process::id()));
        drop(first);
        // Other tests spawn children in parallel, and a child holds a copy of
        // the lock file between its fork and its exec. So the lock frees soon,
        // not at once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while take(tmp.path()).unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "the lock never freed");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // A relaunch watcher outlives the client, and the in-place restart execs.
    // Either would hold the lock past the client's exit if the fd survived.
    #[cfg(unix)]
    #[test]
    fn the_lock_never_crosses_an_exec() {
        use std::os::fd::AsRawFd;
        let tmp = tempfile::tempdir().unwrap();
        let lock = take(tmp.path()).unwrap().unwrap();
        // SAFETY: the descriptor is valid for as long as `lock` lives.
        let flags = unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0, "{}", std::io::Error::last_os_error());
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "lock fd is inherited by children"
        );
    }

    #[test]
    fn the_refusal_names_the_holder_when_it_is_known() {
        assert_eq!(
            refusal(Some(42)),
            "Lucidos is already running (pid 42). Not starting a second client."
        );
        assert_eq!(
            refusal(None),
            "Lucidos is already running. Not starting a second client."
        );
    }
}
