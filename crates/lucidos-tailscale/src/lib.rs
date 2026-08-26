//! What the tree knows about Tailscale: where this machine sits on a tailnet,
//! what that node is called, and where the CLI lives.
//!
//! # Why this crate exists
//!
//! Three crates need some of this. `lucidos-gateway` and `lucidos-engine` want
//! the tailnet address to offer as a network-bind hint; `lucidos-app` wants the
//! full picture for **Settings -> Access**. Each grew its own copy, so a
//! `PATH` fix on 2026-07-31 landed in two of them and missed the third, which
//! is the bug this crate was extracted to end.
//!
//! ADR 0014 §1 requires the gateway to have no dependency on the engine, and
//! offers the shared surface as "extracted to a tiny shared util **or**
//! duplicated". This is the first option. It stays viable only while the crate
//! stays trivial, so **`libc` is the only dependency** and must remain so.
//!
//! # Reading state costs no subprocess
//!
//! [`tailnet_ipv4`] reads the interface list and [`magic_dns_name`] does a
//! reverse lookup. Neither runs `tailscale`, which matters because the packaged
//! app and the packaged gateway are started by Finder/launchd with no `PATH` at
//! all, and because a user whose Tailscale works should never be asked to
//! install a CLI just so we can describe their own machine back to them.
//!
//! The CLI half ([`tailscale_binary`]) survives for the one thing that has no
//! other interface: `tailscale serve`.

use std::net::Ipv4Addr;

mod cli;

pub use cli::{resolve_tailscale_binary, tailscale_binary, TAILSCALE_CANDIDATES};

#[cfg(unix)]
mod unix;

/// Is this IPv4 literal inside Tailscale's `100.64.0.0/10` CGNAT range?
///
/// Range membership alone does **not** make an address a tailnet address: see
/// [`select_tailnet_addr`]. Kept public because both `net_config` modules
/// validate a user-typed bind address against it.
pub fn is_tailnet_ipv4(ip: &str) -> bool {
    matches!(ip.trim().parse::<Ipv4Addr>(), Ok(v4) if is_tailnet_addr(v4))
}

/// [`is_tailnet_ipv4`] for an already-parsed address.
pub fn is_tailnet_addr(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Does this interface name belong to Tailscale?
///
/// macOS (both the App Store and standalone builds) uses a `utun` device; Linux
/// uses `tailscale0`. Tailscale's userspace-networking mode creates no
/// interface at all, and on such a host detection correctly finds nothing.
pub fn is_tailscale_interface(name: &str) -> bool {
    name.starts_with("utun") || name.starts_with("tailscale")
}

/// Pick this machine's tailnet address out of its interface list.
///
/// **Both halves are required.** `100.64.0.0/10` is real CGNAT space that an ISP
/// can legitimately hand to a physical interface over DHCP, so a range-only
/// match would report a WAN lease on `en0` as the tailnet address. The CLI this
/// replaced could never make that mistake, so guarding it is not paranoia, it is
/// keeping a promise the old mechanism made for free.
pub fn select_tailnet_addr<S: AsRef<str>>(addrs: &[(S, Ipv4Addr)]) -> Option<Ipv4Addr> {
    addrs
        .iter()
        .find(|(name, addr)| is_tailscale_interface(name.as_ref()) && is_tailnet_addr(*addr))
        .map(|(_, addr)| *addr)
}

/// This machine's tailnet IPv4, read straight from the interface list.
///
/// `None` when Tailscale is absent, logged out, or running in userspace mode.
/// Non-unix targets always yield `None` (the tree ships macOS and Linux only).
pub fn tailnet_ipv4() -> Option<Ipv4Addr> {
    #[cfg(unix)]
    {
        unix::tailnet_ipv4()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The MagicDNS name for a tailnet address (`<machine>.<tailnet>.ts.net`, no
/// trailing dot, no scheme), via a reverse lookup bounded by `timeout`.
///
/// **A `None` here does not mean "not on a tailnet."** MagicDNS is a per-tailnet
/// toggle, so a node with it disabled has an address and no name. Callers must
/// treat this as a name lookup, never as the tailnet-membership test: gating
/// membership on it would report such a tailnet as offline, which the CLI it
/// replaced did not.
pub fn magic_dns_name(addr: Ipv4Addr, timeout: std::time::Duration) -> Option<String> {
    #[cfg(unix)]
    {
        unix::magic_dns_name(addr, timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = (addr, timeout);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn tailnet_range_covers_100_64_through_100_127() {
        assert!(is_tailnet_ipv4("100.64.0.1"));
        assert!(is_tailnet_ipv4("100.100.0.1"));
        assert!(is_tailnet_ipv4("100.127.255.255"));
        // Just outside the /10 on either side.
        assert!(!is_tailnet_ipv4("100.63.255.255"));
        assert!(!is_tailnet_ipv4("100.128.0.1"));
        // Ordinary private space, and garbage.
        assert!(!is_tailnet_ipv4("192.168.1.10"));
        assert!(!is_tailnet_ipv4("not-an-ip"));
        assert!(!is_tailnet_ipv4(""));
    }

    #[test]
    fn tailscale_interfaces_are_utun_and_tailscale() {
        assert!(is_tailscale_interface("utun4"));
        assert!(is_tailscale_interface("tailscale0"));
        assert!(!is_tailscale_interface("en0"));
        assert!(!is_tailscale_interface("lo0"));
    }

    #[test]
    fn select_requires_both_a_tailscale_interface_and_the_range() {
        // The regression this guards: an ISP CGNAT lease on a physical
        // interface must never be reported as the tailnet address.
        assert_eq!(select_tailnet_addr(&[("en0", v4("100.64.0.1"))]), None);
        // A tailnet interface carrying something outside the range is not it
        // either (another VPN also uses utun on macOS).
        assert_eq!(select_tailnet_addr(&[("utun0", v4("10.1.2.3"))]), None);
        // Both halves, in the presence of the usual noise.
        assert_eq!(
            select_tailnet_addr(&[
                ("lo0", v4("127.0.0.1")),
                ("en0", v4("192.168.1.10")),
                ("utun4", v4("100.64.0.2")),
            ]),
            Some(v4("100.64.0.2"))
        );
        assert_eq!(
            select_tailnet_addr(&[("tailscale0", v4("100.90.1.2"))]),
            Some(v4("100.90.1.2"))
        );
        assert_eq!(select_tailnet_addr::<&str>(&[]), None);
    }
}
