//! The size of a volume on ZFS, where `statvfs` understates it.
//!
//! On a ZFS dataset the OS reports total space as own usage plus pool free
//! space. So the total shrinks as the pool fills. Only ZFS's own CLI knows the
//! pool size. Plan: `docs/plans/2026-11-05-disk-thresholds-scale-with-volume.md`.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Where OpenZFS installs its CLI on Linux, NixOS, FreeBSD and macOS. The
/// engine runs under a minimal service PATH, so it never looks up the name.
const ZFS_CLI_PATHS: &[&str] = &[
    "/usr/sbin/zfs",
    "/sbin/zfs",
    "/usr/local/sbin/zfs",
    "/run/current-system/sw/bin/zfs",
    "/usr/local/zfs/bin/zfs",
];

const ZFS_CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an answer, or a failure to get one, stands before the next query.
/// A quota change or a grown cloud volume shows up within this.
const SIZE_REFRESH: Duration = Duration::from_secs(600);

/// True when `path` lives on a ZFS dataset.
#[cfg(target_os = "linux")]
pub(crate) fn is_zfs(path: &Path) -> bool {
    const ZFS_SUPER_MAGIC: u64 = 0x2fc1_2fc1;
    statfs(path).is_some_and(|s| u64::try_from(s.f_type).is_ok_and(|t| t == ZFS_SUPER_MAGIC))
}

/// True when `path` lives on a ZFS dataset.
#[cfg(target_os = "macos")]
pub(crate) fn is_zfs(path: &Path) -> bool {
    statfs(path).is_some_and(|s| {
        // SAFETY: statfs filled `f_fstypename` with a NUL-terminated name.
        let name = unsafe { std::ffi::CStr::from_ptr(s.f_fstypename.as_ptr()) };
        name.to_bytes() == b"zfs"
    })
}

/// True when `path` lives on a ZFS dataset.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn is_zfs(_path: &Path) -> bool {
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn statfs(path: &Path) -> Option<libc::statfs> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `buf` is a valid out pointer.
    let rc = unsafe { libc::statfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    // SAFETY: a zero return means statfs filled `buf`.
    (rc == 0).then(|| unsafe { buf.assume_init() })
}

#[derive(Default)]
struct SizeEntry {
    cli_size: Option<u64>,
    largest_os_total: u64,
    fetched: Option<Instant>,
    in_flight: bool,
}

/// The size the dataset holding `path` can grow to. The last CLI answer
/// wins. Before it, or without one, the largest OS total seen stands: it is
/// exact under a quota, and it does not shrink as the pool fills.
///
/// Never blocks: a due query runs on its own thread, at most one per path,
/// so a hung pool stalls only that thread.
pub(crate) fn volume_size_bytes(path: &Path, os_total_bytes: u64) -> u64 {
    static SIZES: OnceLock<Mutex<HashMap<PathBuf, SizeEntry>>> = OnceLock::new();
    let sizes = SIZES.get_or_init(Default::default);
    let Ok(mut map) = sizes.lock() else {
        return os_total_bytes;
    };
    let entry = map.entry(path.to_path_buf()).or_default();
    entry.largest_os_total = entry.largest_os_total.max(os_total_bytes);
    let due = entry.fetched.is_none_or(|at| at.elapsed() >= SIZE_REFRESH);
    if due && !entry.in_flight {
        let key = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("zfs-size".into())
            .spawn(move || {
                let cli_size = query_dataset_size(&key);
                let Ok(mut map) = sizes.lock() else { return };
                let entry = map.entry(key.clone()).or_default();
                match cli_size {
                    Some(size) => entry.cli_size = Some(size),
                    None if entry.fetched.is_none() => log!(
                        "[WorktreeCleanup] ZFS size unknown for {}: the largest OS total applies",
                        key.display()
                    ),
                    // A failed refresh keeps the last answer.
                    None => {}
                }
                entry.fetched = Some(Instant::now());
                entry.in_flight = false;
            });
        entry.in_flight = spawned.is_ok();
    }
    entry.cli_size.unwrap_or(entry.largest_os_total)
}

fn query_dataset_size(path: &Path) -> Option<u64> {
    let zfs = ZFS_CLI_PATHS.iter().map(Path::new).find(|p| p.is_file())?;
    // OpenZFS resolves an absolute path argument to the dataset that holds it.
    let path = std::path::absolute(path).ok()?;
    let dataset = run_zfs(zfs, &["list", "-H", "-o", "name", path.to_str()?])?;
    let dataset = dataset.trim();
    let mut args = vec!["list", "-Hp", "-o", "name,quota,refquota,used,available"];
    args.extend(dataset_and_ancestors(dataset));
    size_from_listing(dataset, &run_zfs(zfs, &args)?)
}

/// Runs `zfs <args>` and returns its stdout, or `None` on a spawn failure, a
/// non-zero exit or the timeout. Blocks, so only the query thread calls it.
fn run_zfs(zfs: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(zfs)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + ZFS_CLI_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            // Reaps the timed-out child. Its answer is lost either way.
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    status.success().then_some(out)
}

/// `tank/a/b` gives `tank/a/b`, `tank/a` and `tank`.
pub(crate) fn dataset_and_ancestors(dataset: &str) -> impl Iterator<Item = &str> {
    dataset
        .match_indices('/')
        .map(|(i, _)| &dataset[..i])
        .chain(std::iter::once(dataset))
}

/// The size `dataset` can grow to, from `name quota refquota used available`
/// lines for it and its ancestors. The pool root's used plus available is the
/// pool's usable size. The dataset's own quota and refquota bound it, and so
/// does each ancestor's quota. ZFS prints 0 for an unset quota.
pub(crate) fn size_from_listing(dataset: &str, listing: &str) -> Option<u64> {
    let pool = dataset.split('/').next()?;
    let mut pool_size = None;
    let mut limit = u64::MAX;
    for line in listing.lines() {
        let mut fields = line.split('\t');
        let name = fields.next()?;
        let [quota, refquota, used, available] = [(); 4].map(|_| fields.next());
        let num = |f: Option<&str>| f?.parse::<u64>().ok();
        let quota = num(quota)?;
        if name == pool {
            pool_size = Some(num(used)?.checked_add(num(available)?)?);
        }
        let refquota = if name == dataset { num(refquota)? } else { 0 };
        limit = [quota, refquota]
            .into_iter()
            .filter(|&b| b > 0)
            .fold(limit, u64::min);
    }
    Some(pool_size?.min(limit))
}
