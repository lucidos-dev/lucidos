//! The two POSIX syscalls behind the crate's CLI-free reads: `getifaddrs` for
//! this machine's tailnet address, `getnameinfo` for its MagicDNS name.

use std::ffi::CStr;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::select_tailnet_addr;

/// POSIX `NI_MAXHOST`. Written out rather than taken from `libc` because the
/// constant is not exposed uniformly across its platform modules; the value is
/// 1025 on both macOS and Linux.
const NI_MAXHOST: usize = 1025;

/// Every IPv4 address currently assigned, paired with its interface name.
fn ipv4_interfaces() -> Vec<(String, Ipv4Addr)> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes a list head we own and free below; a non-zero
    // return means it wrote nothing.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is non-null and points into the list getifaddrs built,
        // which stays alive until the freeifaddrs below.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: non-null, and every sockaddr starts with its family.
        if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: AF_INET means the sockaddr is really a sockaddr_in.
        let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
        // s_addr is network byte order; from_be puts the first octet high.
        let addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
        // SAFETY: getifaddrs always supplies a NUL-terminated name.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) };
        if let Ok(name) = name.to_str() {
            out.push((name.to_string(), addr));
        }
    }
    // SAFETY: `head` came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    out
}

pub(crate) fn tailnet_ipv4() -> Option<Ipv4Addr> {
    select_tailnet_addr(&ipv4_interfaces())
}

/// Blocking reverse lookup. Only ever called from [`magic_dns_name`], which
/// bounds it.
fn reverse_lookup(addr: Ipv4Addr) -> Option<String> {
    // Zero-initialised and then filled, because macOS's sockaddr_in carries an
    // `sin_len` that Linux's does not; a struct literal could not cover both.
    // SAFETY: sockaddr_in is a plain C struct, valid all-zero.
    let mut sin: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    sin.sin_family = libc::AF_INET as libc::sa_family_t;
    sin.sin_addr.s_addr = u32::from(addr).to_be();
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        sin.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
    }

    let mut host = [0 as libc::c_char; NI_MAXHOST];
    // SAFETY: `sin` is a fully initialised sockaddr_in of the length passed,
    // and `host` is a writable buffer of the capacity passed. NI_NAMEREQD makes
    // a missing PTR record an error rather than the numeric form echoed back,
    // so we never mistake "no name" for a name.
    let rc = unsafe {
        libc::getnameinfo(
            std::ptr::addr_of!(sin) as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if rc != 0 {
        return None;
    }
    // SAFETY: on success getnameinfo NUL-terminates within the buffer.
    let name = unsafe { CStr::from_ptr(host.as_ptr()) }.to_str().ok()?;
    let name = name.trim_end_matches('.').trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub(crate) fn magic_dns_name(addr: Ipv4Addr, timeout: Duration) -> Option<String> {
    // `getnameinfo` has no timeout of its own and answers to whatever the system
    // resolver decides, which on a half-configured tailnet is seconds. This is
    // called while a settings pane is loading, so it gets a hard ceiling: the
    // worker is left to finish into a dropped channel rather than joined, since
    // the only cost of abandoning it is the lookup we already gave up on.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(reverse_lookup(addr));
    });
    rx.recv_timeout(timeout).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_has_no_magic_dns_name_and_returns_promptly() {
        // 127.0.0.1 reverse-resolves to "localhost" on a normal host, which is
        // not a tailnet name; the point of the assertion is that the bounded
        // call SETTLES rather than what it settles on.
        let start = std::time::Instant::now();
        let _ = magic_dns_name(Ipv4Addr::LOCALHOST, Duration::from_millis(1500));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn interface_list_is_readable_and_includes_loopback() {
        // Guards the FFI itself: a mis-cast sockaddr or a bad byte order would
        // show up here as a missing or garbled 127.0.0.1.
        let ifaces = ipv4_interfaces();
        assert!(
            ifaces.iter().any(|(_, a)| *a == Ipv4Addr::LOCALHOST),
            "expected loopback among {ifaces:?}"
        );
    }
}
