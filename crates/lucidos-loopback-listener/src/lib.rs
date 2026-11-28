//! Bind a server's TCP listeners so it holds both halves of loopback.
//!
//! IPv4 and IPv6 loopback are separate addresses. While a server holds only
//! `127.0.0.1:p`, another program can bind `[::1]:p` without an error. A
//! client that resolves `localhost` to `::1` first then reaches that program.
//! So every server that holds a loopback port also binds `[::1]:p`.
//!
//! A host with IPv6 turned off has no `::1`. There the IPv6 half is skipped
//! and reported once, because no program can shadow an address that does not
//! exist. A `[::1]:p` already held by someone else is fatal, exactly like a
//! held `127.0.0.1:p`.

use std::io;
use std::net::{Ipv6Addr, SocketAddr, TcpListener};

/// How often an ephemeral pick (port 0) is retried when its `[::1]` twin is
/// taken. That is chance, not a conflict, so another pick fixes it.
const EPHEMERAL_ATTEMPTS: usize = 16;

/// The listeners a server serves, plus why the IPv6 half is missing, if it is.
#[derive(Debug)]
pub struct Bound {
    /// The required listeners in the order given, then `[::1]` when held.
    pub listeners: Vec<TcpListener>,
    /// Set when this host has no IPv6 loopback. The caller logs it once.
    pub ipv6_loopback_unavailable: Option<io::Error>,
}

impl Bound {
    /// The port every listener shares.
    pub fn port(&self) -> io::Result<u16> {
        Ok(self.listeners[0].local_addr()?.port())
    }
}

/// Bind every `required` address, then `[::1]` on the same port unless a
/// required address already covers IPv6 loopback (`[::]` or `[::1]`).
///
/// Every required address must share one port, and the list must not be
/// empty. Port 0 picks an ephemeral port that is free on both halves.
pub fn bind(required: &[SocketAddr]) -> io::Result<Bound> {
    let first = required
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to bind"))?;
    if first.port() != 0 {
        return bind_once(required, &mut TcpListener::bind);
    }
    let mut last = None;
    for _ in 0..EPHEMERAL_ATTEMPTS {
        match bind_once(required, &mut TcpListener::bind) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => last = Some(e),
            other => return other,
        }
    }
    Err(last.expect("the loop ran at least once"))
}

/// The line a server logs once when [`Bound::ipv6_loopback_unavailable`] is set.
pub fn describe_unavailable(port: u16, error: &io::Error) -> String {
    format!(
        "this host has no IPv6 loopback ({error}), so only 127.0.0.1:{port} is held; \
         nothing can take [::1]:{port} here either"
    )
}

fn bind_once(
    required: &[SocketAddr],
    bind_one: &mut dyn FnMut(SocketAddr) -> io::Result<TcpListener>,
) -> io::Result<Bound> {
    let mut listeners = Vec::with_capacity(required.len() + 1);
    let mut port = required[0].port();
    for addr in required {
        let listener = bind_one(SocketAddr::new(addr.ip(), port))?;
        port = listener.local_addr()?.port();
        listeners.push(listener);
    }
    if required.iter().any(covers_ipv6_loopback) {
        return Ok(Bound {
            listeners,
            ipv6_loopback_unavailable: None,
        });
    }
    let ipv6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
    match bind_one(ipv6) {
        Ok(listener) => {
            listeners.push(listener);
            Ok(Bound {
                listeners,
                ipv6_loopback_unavailable: None,
            })
        }
        Err(e) if host_lacks_ipv6_loopback(&e) => Ok(Bound {
            listeners,
            ipv6_loopback_unavailable: Some(e),
        }),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another program holds {ipv6}, so `localhost:{port}` would reach it \
                 instead of Lucidos ({e})"
            ),
        )),
        Err(e) => Err(e),
    }
}

fn covers_ipv6_loopback(addr: &SocketAddr) -> bool {
    match addr {
        SocketAddr::V6(v6) => v6.ip().is_unspecified() || v6.ip().is_loopback(),
        SocketAddr::V4(_) => false,
    }
}

/// `EADDRNOTAVAIL` when IPv6 is on but `::1` is gone (a sysctl
/// `disable_ipv6=1`), `EAFNOSUPPORT` when the kernel has no IPv6 at all.
fn host_lacks_ipv6_loopback(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AddrNotAvailable
        || error.raw_os_error() == Some(libc::EAFNOSUPPORT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    fn host_has_ipv6_loopback() -> bool {
        TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_ok()
    }

    #[test]
    fn a_loopback_server_holds_both_halves() {
        if !host_has_ipv6_loopback() {
            eprintln!("skipped: this host has no IPv6 loopback");
            return;
        }
        let bound = bind(&[v4(0)]).expect("binds");
        let port = bound.port().unwrap();
        assert_eq!(bound.listeners.len(), 2);
        assert!(bound.ipv6_loopback_unavailable.is_none());
        let squatter = TcpListener::bind((Ipv6Addr::LOCALHOST, port));
        assert_eq!(
            squatter.map(|_| ()).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn a_program_already_on_the_ipv6_half_refuses_the_bind() {
        if !host_has_ipv6_loopback() {
            eprintln!("skipped: this host has no IPv6 loopback");
            return;
        }
        let squatter = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();
        let port = squatter.local_addr().unwrap().port();
        let error = bind(&[v4(port)]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        assert!(error.to_string().contains(&format!("[::1]:{port}")));
    }

    /// Binds IPv4 for real on an ephemeral port, records every address asked
    /// for, and answers the IPv6 attempt with `ipv6`.
    fn scripted(
        ipv6: io::Error,
        asked: &mut Vec<SocketAddr>,
    ) -> impl FnMut(SocketAddr) -> io::Result<TcpListener> + '_ {
        let mut ipv6 = Some(ipv6);
        move |addr| {
            asked.push(addr);
            match addr {
                SocketAddr::V6(_) => Err(ipv6.take().expect("one IPv6 attempt")),
                SocketAddr::V4(_) => TcpListener::bind(SocketAddr::new(addr.ip(), 0)),
            }
        }
    }

    #[test]
    fn a_host_without_ipv6_loopback_serves_ipv4_only() {
        for error in [
            io::Error::from(io::ErrorKind::AddrNotAvailable),
            io::Error::from_raw_os_error(libc::EADDRNOTAVAIL),
            io::Error::from_raw_os_error(libc::EAFNOSUPPORT),
        ] {
            let mut asked = Vec::new();
            let bound = bind_once(&[v4(0)], &mut scripted(error, &mut asked)).expect("starts");
            assert_eq!(bound.listeners.len(), 1);
            assert!(bound.ipv6_loopback_unavailable.is_some());
        }
    }

    #[test]
    fn a_held_or_failing_ipv6_half_is_fatal() {
        for error in [
            io::Error::from_raw_os_error(libc::EADDRINUSE),
            io::Error::from_raw_os_error(libc::EACCES),
        ] {
            let mut asked = Vec::new();
            assert!(bind_once(&[v4(0)], &mut scripted(error, &mut asked)).is_err());
        }
    }

    #[test]
    fn the_ipv6_half_takes_the_port_the_ipv4_half_bound() {
        let mut asked = Vec::new();
        let error = io::Error::from(io::ErrorKind::AddrNotAvailable);
        let bound = bind_once(&[v4(0)], &mut scripted(error, &mut asked)).unwrap();
        let port = bound.port().unwrap();
        assert_eq!(
            asked,
            vec![v4(0), SocketAddr::from((Ipv6Addr::LOCALHOST, port))]
        );
    }

    #[test]
    fn an_address_that_covers_ipv6_loopback_binds_no_second_socket() {
        for addr in [Ipv6Addr::LOCALHOST, Ipv6Addr::UNSPECIFIED] {
            let mut asked = 0;
            let mut record = |_: SocketAddr| {
                asked += 1;
                TcpListener::bind(v4(0))
            };
            bind_once(&[SocketAddr::from((addr, 0))], &mut record).unwrap();
            assert_eq!(asked, 1, "{addr}");
        }
    }

    #[test]
    fn an_empty_list_is_refused() {
        assert_eq!(bind(&[]).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
