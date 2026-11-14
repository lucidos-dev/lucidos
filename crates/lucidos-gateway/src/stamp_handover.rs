//! The stamp keys across a dev reload's re-exec.
//!
//! `reload_gateway` replaces this image and keeps every engine running. Each
//! engine still holds the key it got at spawn, so the new image needs the same
//! keys or it signs nothing those engines accept. They cross the exec in a
//! pipe, the way the owner lock's descriptor does. Only the descriptor number
//! travels in the environment, never a key.
//!
//! A restart that is not a re-exec loses the table. The supervisor then
//! respawns each keyless engine it keeps once that engine is idle.

use std::collections::HashMap;
use std::sync::Arc;

use lucidos_gateway_stamp::StampKey;

/// Where a re-exec names the descriptor holding the key table.
pub const INHERITED_STAMP_KEYS_FD_ENV: &str = "LUCIDOS_GATEWAY_STAMP_KEYS_FD";

/// Workspace id to the stamp key of its running engine.
pub type StampKeys = HashMap<String, Arc<StampKey>>;

/// The table as pipe text: one `<workspace id> <key line>` per engine.
fn encode(keys: &StampKeys) -> String {
    keys.iter()
        .map(|(id, key)| format!("{id} {}", key.handover_line()))
        .collect()
}

/// Read [`encode`]'s text back. A malformed line is skipped, which leaves that
/// engine keyless: its devices read as unattested until it is re-keyed.
fn decode(text: &str) -> StampKeys {
    text.lines()
        .filter_map(|line| {
            let (id, hex) = line.rsplit_once(' ')?;
            let key = StampKey::read_handover(hex.as_bytes())?;
            (!id.is_empty()).then(|| (id.to_string(), Arc::new(key)))
        })
        .collect()
}

/// A pipe holding `keys`, returning its read end for a re-exec to inherit.
///
/// `std::io::pipe` makes both ends close-on-exec, atomically where the OS can
/// (Linux), so no engine spawned meanwhile inherits either. The caller clears
/// the flag on the read end in its `pre_exec`. The write end is non-blocking
/// and closed before return: a table larger than the pipe buffer fails here
/// rather than hanging the reload.
#[cfg(unix)]
pub fn pipe_for_exec(keys: &StampKeys) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    let (read, mut write) = std::io::pipe()?;
    // SAFETY: sets one status flag on a descriptor this function owns.
    unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    write.write_all(encode(keys).as_bytes())?;
    Ok(read.into())
}

/// Take the key table a `reload_gateway` re-exec handed this image, and drop
/// its descriptor and its variable.
///
/// Call before the first spawn, while the process is still single-threaded,
/// since it sets process env. Empty on any launch that is not a re-exec.
#[cfg(unix)]
pub fn take_inherited() -> StampKeys {
    use std::os::fd::FromRawFd as _;
    let value = std::env::var(INHERITED_STAMP_KEYS_FD_ENV).ok();
    // SAFETY: single-threaded, per the contract above.
    unsafe {
        std::env::remove_var(INHERITED_STAMP_KEYS_FD_ENV);
    }
    // Stdio never carries the table.
    let Some(fd) = value
        .and_then(|v| v.parse::<std::os::fd::RawFd>().ok())
        .filter(|&fd| fd > 2)
    else {
        return StampKeys::new();
    };
    // SAFETY: the previous image passed this descriptor to this one alone, and
    // nothing in this image has opened it yet.
    let pipe = unsafe { std::fs::File::from_raw_fd(fd) };
    let keys = decode(&read_written(pipe));
    crate::log!(
        "[Gateway] took {} engine stamp key(s) over from the previous image",
        keys.len()
    );
    keys
}

/// Read what the previous image wrote before its exec. The read end is
/// non-blocking, so a child that inherited the write end cannot hang the
/// boot: the table is already in the pipe buffer, and the read stops there.
#[cfg(unix)]
fn read_written(mut pipe: std::fs::File) -> String {
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;
    // SAFETY: sets one status flag on a descriptor this function owns.
    unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => {
                crate::log!("[Gateway] could not read the stamp keys the reload handed over: {e}");
                break;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// No re-exec hands anything over off Unix.
#[cfg(not(unix))]
pub fn take_inherited() -> StampKeys {
    StampKeys::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_gateway_stamp::Claim;

    fn table(ids: &[&str]) -> StampKeys {
        ids.iter()
            .map(|id| (id.to_string(), Arc::new(StampKey::generate().unwrap())))
            .collect()
    }

    fn same_keys(a: &StampKeys, b: &StampKeys) -> bool {
        a.len() == b.len()
            && a.iter().all(|(id, key)| {
                let claim = Claim::Device {
                    workspace_id: id,
                    device_id: "d1",
                };
                b.get(id)
                    .is_some_and(|other| other.verify(&key.sign(claim, 1_000), claim, 1_000))
            })
    }

    #[test]
    fn the_table_round_trips_through_its_text() {
        let keys = table(&["dev", "work-2"]);
        assert!(same_keys(&keys, &decode(&encode(&keys))));
    }

    #[test]
    fn a_malformed_line_leaves_only_that_engine_keyless() {
        let keys = table(&["dev"]);
        let text = format!("broken-line\nnohex zz\n{}", encode(&keys));
        assert!(same_keys(&keys, &decode(&text)));
    }

    #[cfg(unix)]
    #[test]
    fn the_pipe_carries_the_table_to_its_read_end() {
        use std::io::Read as _;
        let keys = table(&["dev", "work-2"]);
        let read = pipe_for_exec(&keys).unwrap();
        let mut text = String::new();
        std::fs::File::from(read).read_to_string(&mut text).unwrap();
        assert!(same_keys(&keys, &decode(&text)));
    }

    /// A child that inherited the write end keeps it open. The boot still
    /// reads the table and goes on, rather than waiting for an EOF.
    #[cfg(unix)]
    #[test]
    fn a_held_write_end_does_not_hang_the_read() {
        use std::io::Write as _;
        let keys = table(&["dev"]);
        let (read, mut write) = std::io::pipe().unwrap();
        write.write_all(encode(&keys).as_bytes()).unwrap();
        let text = read_written(std::fs::File::from(std::os::fd::OwnedFd::from(read)));
        assert!(same_keys(&keys, &decode(&text)));
        drop(write);
    }
}
