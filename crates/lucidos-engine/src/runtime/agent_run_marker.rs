//! The *agent run marker*: a random token every coding-agent spawn carries in
//! its environment as `LUCIDOS_AGENT_RUN`.
//!
//! Every process the agent starts inherits it, whatever process group it lands
//! in. Claude Code runs each Bash call in a group of its own, so tearing down
//! the agent's group never reaches those calls. The marker does: once the agent
//! is gone, [`reap_marked_groups`] tears down every process group holding a
//! process that carries it (ADR 0391).
//!
//! The environment is a kernel fact (ADR 0025, ADR 0251). A process whose
//! environment cannot be read is never ours.

use std::collections::BTreeSet;
use std::time::Duration;

pub(crate) const AGENT_RUN_ENV: &str = "LUCIDOS_AGENT_RUN";

/// sccache sets this in the environment of the daemon it forks. The agents'
/// daemon is shared by every session, so it is never reaped, even when the
/// agent that forked it passed its marker on.
const SHARED_DAEMON_ENTRY: &[u8] = b"SCCACHE_START_SERVER=1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentRunMarker(String);

impl AgentRunMarker {
    pub(crate) fn mint() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub(crate) fn value(&self) -> &str {
        &self.0
    }

    fn entry(&self) -> Vec<u8> {
        format!("{AGENT_RUN_ENV}={}", self.0).into_bytes()
    }
}

/// Tear down every process group holding a process that carries `marker`:
/// SIGTERM, `grace`, then SIGKILL for what a fresh scan still finds.
///
/// Call it once the agent child is gone. The second scan is what makes the
/// SIGKILL safe: a group id from the first scan may have been recycled.
pub(crate) async fn reap_marked_groups(marker: AgentRunMarker, grace: Duration) {
    let groups = scan_groups(&marker).await;
    if groups.is_empty() {
        return;
    }
    crate::log!(
        "[AgentRunMarker] process groups {groups:?} outlived their agent ({}), sending SIGTERM",
        marker.value()
    );
    signal_groups(&groups, SIGTERM);
    tokio::time::sleep(grace).await;
    let left = scan_groups(&marker).await;
    if !left.is_empty() {
        crate::log!(
            "[AgentRunMarker] process groups {left:?} ignored SIGTERM ({}), sending SIGKILL",
            marker.value()
        );
        signal_groups(&left, SIGKILL);
    }
}

#[cfg(unix)]
const SIGTERM: i32 = libc::SIGTERM;
#[cfg(unix)]
const SIGKILL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
const SIGTERM: i32 = 15;
#[cfg(not(unix))]
const SIGKILL: i32 = 9;

fn signal_groups(groups: &BTreeSet<i32>, signal: i32) {
    for &group in groups {
        // `groups_to_signal` admits only ids above 1, so the cast is lossless.
        super::spawn_env::signal_child_process_group(group as u32, signal);
    }
}

async fn scan_groups(marker: &AgentRunMarker) -> BTreeSet<i32> {
    let entry = marker.entry();
    // A scan that could not run finds nothing, so it kills nothing. Say so,
    // since "nothing outlived the agent" is the other way to read an empty set.
    let found = match tokio::task::spawn_blocking(move || platform::marked_processes(&entry)).await
    {
        Ok(found) => found,
        Err(e) => {
            crate::log!("[AgentRunMarker] process scan failed, reaping nothing: {e}");
            Vec::new()
        }
    };
    groups_to_signal(&found, platform::own_group())
}

/// The groups to signal for the `(pid, pgid)` pairs found carrying a marker.
///
/// Group 0 means the caller's own group to `kill(2)`, and group 1 is launchd
/// or init, so neither is ever named (ADR 0025). Nor is the engine's own group.
fn groups_to_signal(found: &[(i32, i32)], own_group: i32) -> BTreeSet<i32> {
    found
        .iter()
        .map(|&(_, group)| group)
        .filter(|&group| group > 1 && group != own_group)
        .collect()
}

/// Whether a process with these environment entries is one to reap.
fn reapable(entries: &[&[u8]], marker_entry: &[u8]) -> bool {
    entries.contains(&marker_entry) && !entries.contains(&SHARED_DAEMON_ENTRY)
}

/// The environment out of a macOS `KERN_PROCARGS2` buffer. Its layout is
/// argc, the exec path, NUL padding, argc argv strings, then the environment
/// up to an empty string. Anything that does not parse is `None`.
#[cfg(any(target_os = "macos", test))]
fn procargs2_environment(buf: &[u8]) -> Option<Vec<&[u8]>> {
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let rest = buf.get(4..)?;
    let rest = &rest[rest.iter().position(|&b| b == 0)?..];
    let rest = &rest[rest.iter().position(|&b| b != 0)?..];
    let mut strings = rest.split(|&b| b == 0);
    for _ in 0..argc {
        strings.next()?;
    }
    Some(strings.take_while(|s| !s.is_empty()).collect())
}

/// The entries of a NUL-separated environment block, as Linux's
/// `/proc/<pid>/environ` holds it.
#[cfg(any(target_os = "linux", test))]
fn environ_entries(buf: &[u8]) -> Vec<&[u8]> {
    buf.split(|&b| b == 0).filter(|s| !s.is_empty()).collect()
}

#[cfg(target_os = "macos")]
mod platform {
    pub(super) fn own_group() -> i32 {
        // SAFETY: getpgrp takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    pub(super) fn marked_processes(marker_entry: &[u8]) -> Vec<(i32, i32)> {
        let own = std::process::id() as i32;
        all_pids()
            .into_iter()
            .filter(|&pid| pid > 1 && pid != own)
            .filter(|&pid| {
                raw_procargs(pid).is_some_and(|buf| {
                    super::procargs2_environment(&buf)
                        .is_some_and(|env| super::reapable(&env, marker_entry))
                })
            })
            .filter_map(|pid| group_of(pid).map(|group| (pid, group)))
            .collect()
    }

    fn group_of(pid: i32) -> Option<i32> {
        // SAFETY: getpgid takes a plain pid and reports failure as -1.
        let group = unsafe { libc::getpgid(pid) };
        (group > 0).then_some(group)
    }

    fn all_pids() -> Vec<i32> {
        // SAFETY: a null buffer asks only for the current count.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            crate::log!("[AgentRunMarker] could not list processes, reaping nothing");
            return Vec::new();
        };
        // Room for processes started between the two calls.
        let mut pids = vec![0i32; count + 64];
        let Ok(bytes) = libc::c_int::try_from(std::mem::size_of_val(pids.as_slice())) else {
            return Vec::new();
        };
        // SAFETY: the buffer is `bytes` long and writable; the call writes at
        // most that many bytes and returns how many pids it wrote.
        let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(usize::try_from(written).unwrap_or(0));
        pids
    }

    fn raw_procargs(pid: i32) -> Option<Vec<u8>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a null old-value pointer asks only for the size into `size`.
        let sized = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if sized != 0 || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: `buf` is `size` bytes long and writable, and the kernel
        // writes at most `size` bytes, updating it to the length written.
        let read = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return None;
        }
        buf.truncate(size);
        Some(buf)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) fn own_group() -> i32 {
        // SAFETY: getpgrp takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    pub(super) fn marked_processes(marker_entry: &[u8]) -> Vec<(i32, i32)> {
        let own = std::process::id() as i32;
        let Ok(dir) = std::fs::read_dir("/proc") else {
            crate::log!("[AgentRunMarker] could not list /proc, reaping nothing");
            return Vec::new();
        };
        dir.filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&pid| pid > 1 && pid != own)
            .filter(|&pid| {
                std::fs::read(format!("/proc/{pid}/environ"))
                    .is_ok_and(|buf| super::reapable(&super::environ_entries(&buf), marker_entry))
            })
            .filter_map(|pid| {
                // SAFETY: getpgid takes a plain pid and reports failure as -1.
                let group = unsafe { libc::getpgid(pid) };
                (group > 0).then_some((pid, group))
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub(super) fn own_group() -> i32 {
        0
    }

    pub(super) fn marked_processes(_marker_entry: &[u8]) -> Vec<(i32, i32)> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "agent_run_marker_tests.rs"]
mod tests;
