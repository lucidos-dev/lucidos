//! What the tree knows about Tailscale: where this machine sits on a tailnet,
//! what that node is called, and where the CLI lives.
//!
//! Three crates need this. `lucidos-gateway` and `lucidos-engine` offer the
//! tailnet address as a network-bind hint; `lucidos-app` shows the full picture
//! in **Settings -> Access**. One copy here keeps a fix from reaching only some.
//!
//! ADR 0014 §1 forbids the gateway any dependency on the engine. This crate is
//! the shared surface it allows, so it stays small: `libc` and `serde_json`,
//! never an async runtime, a database crate or an engine crate (ADR 0588).

//! # Tailscale is asked, never guessed
//!
//! [`tailnet_state`] asks Tailscale's LocalAPI for its status, the way the
//! Tailscale CLI does, and falls back to `tailscale status --json`. The address
//! and the MagicDNS name come only from that answer. No interface name or
//! address range ever produces one: other VPNs use `100.64.0.0/10` too.
//! Every subprocess and socket read here has a hard bound.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod cli;
mod discovery;
mod localapi;
mod process;
mod status;

pub use cli::{resolve_tailscale_binary, tailscale_binary, TAILSCALE_CANDIDATES};
pub use status::{TailnetNode, TailnetState};

use discovery::{discover, Discovery, Endpoint, Platform, Probes};

/// Is this address inside the `100.64.0.0/10` CGNAT range?
///
/// A range check only. Tailscale draws from it, and so do other VPNs and ISPs,
/// so membership never makes an address a tailnet address.
pub fn is_tailnet_addr(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// What one way of asking Tailscale got back.
#[derive(Debug)]
pub(crate) enum Answer {
    /// A status body, still to be parsed.
    Status(String),
    /// Nothing answered: refused, absent, or silent until the deadline.
    Silent,
    /// Something answered, and it was not a status.
    Garbled(String),
}

/// Ask Tailscale where this machine stands. Blocking, and bounded to a few
/// seconds; call it off any async worker.
pub fn tailnet_state() -> TailnetState {
    let found = discover(Platform::current(), &SystemProbes);
    settle(&found, localapi::query, cli::status_via_cli)
}

/// This machine's tailnet IPv4, when Tailscale reports one and this host can
/// bind it. Userspace-networking mode reports an address it does not hold.
pub fn bindable_tailnet_ipv4() -> Option<Ipv4Addr> {
    let ip = tailnet_state().node()?.ipv4;
    holds_address(ip.into()).then_some(ip)
}

/// Does this host hold exactly `ip`? A UDP bind succeeds only on a local address.
pub fn holds_address(ip: IpAddr) -> bool {
    UdpSocket::bind((ip, 0)).is_ok()
}

/// Try each endpoint, then the CLI. The first status that parses decides.
/// Otherwise the first thing that went wrong does: a port that refused comes
/// before the CLI shim's prose, so a stopped app reads as not running.
fn settle(
    found: &Discovery,
    query: impl Fn(&Endpoint) -> Answer,
    cli: impl Fn(&str) -> Option<Answer>,
) -> TailnetState {
    let answers = found
        .endpoints
        .iter()
        .map(|endpoint| Some(query(endpoint)))
        .chain(std::iter::once_with(|| cli(&found.cli)))
        .flatten();
    let mut first_failure = None;
    for answer in answers {
        let failure = match answer {
            Answer::Status(body) => match status::parse_status(&body) {
                Ok(state) => return state,
                Err(detail) => TailnetState::Unreadable { detail },
            },
            Answer::Silent => TailnetState::NotRunning,
            Answer::Garbled(detail) => TailnetState::Unreadable { detail },
        };
        first_failure.get_or_insert(failure);
    }
    match first_failure {
        Some(state) => state,
        None if found.installed => TailnetState::NotRunning,
        None => TailnetState::NotInstalled,
    }
}

const LSOF: &str = "/usr/sbin/lsof";
const LSOF_TIMEOUT: Duration = Duration::from_secs(2);

/// The real filesystem, environment and `lsof`.
struct SystemProbes;

impl Probes for SystemProbes {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_link(&self, path: &Path) -> Option<PathBuf> {
        std::fs::read_link(path).ok()
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn ipn_extension_files(&self) -> Option<String> {
        #[cfg(unix)]
        {
            // SAFETY: getuid has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            let mut cmd = Command::new(LSOF);
            // -b and -w keep lsof from blocking on a stuck filesystem.
            cmd.args(["-n", "-b", "-w", "-a", &format!("-u{uid}")])
                .args(["-c", "IPNExtension", "-F", "n"]);
            process::run_bounded(cmd, LSOF_TIMEOUT)
                .ok()
                .map(|ran| ran.stdout)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn cli_override(&self) -> Option<String> {
        std::env::var("LUCIDOS_TAILSCALE_BIN").ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discovery::tests::FakeProbes;
    use status::tests::status_json;
    use std::cell::Cell;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn tailnet_range_covers_100_64_through_100_127() {
        assert!(is_tailnet_addr(v4("100.64.0.1")));
        assert!(is_tailnet_addr(v4("100.100.0.1")));
        assert!(is_tailnet_addr(v4("100.127.255.255")));
        assert!(!is_tailnet_addr(v4("100.63.255.255")));
        assert!(!is_tailnet_addr(v4("100.128.0.1")));
        assert!(!is_tailnet_addr(v4("192.168.1.10")));
    }

    fn found(endpoints: Vec<Endpoint>, installed: bool) -> Discovery {
        Discovery {
            endpoints,
            cli: "tailscale".into(),
            installed,
        }
    }

    fn socket() -> Endpoint {
        Endpoint::Unix("/var/run/tailscale/tailscaled.sock".into())
    }

    fn running() -> String {
        status_json(
            "Running",
            &["100.70.0.1"],
            "mymac.tailnet-name.ts.net.",
            true,
            None,
        )
    }

    #[test]
    fn the_localapi_answer_wins_and_the_cli_is_never_run() {
        let cli_ran = Cell::new(false);
        let state = settle(
            &found(vec![socket()], true),
            |_| Answer::Status(running()),
            |_| {
                cli_ran.set(true);
                None
            },
        );
        assert_eq!(state.node().map(|n| n.ipv4), Some(v4("100.70.0.1")));
        assert!(!cli_ran.get());
    }

    #[test]
    fn the_cli_answers_when_no_endpoint_does() {
        let state = settle(
            &found(vec![socket()], true),
            |_| Answer::Silent,
            |_| Some(Answer::Status(running())),
        );
        assert!(state.node().is_some());
    }

    #[test]
    fn a_cli_with_its_daemon_stopped_is_not_running() {
        let state = settle(
            &found(vec![], true),
            |_| unreachable!(),
            |_| Some(Answer::Silent),
        );
        assert_eq!(state, TailnetState::NotRunning);
    }

    #[test]
    fn a_refused_port_outranks_the_gui_shims_prose() {
        let state = settle(
            &found(
                vec![Endpoint::Tcp {
                    port: 1,
                    token: "t".into(),
                }],
                true,
            ),
            |_| Answer::Silent,
            |_| Some(Answer::Garbled("the Tailscale CLI printed \"GUI\"".into())),
        );
        assert_eq!(state, TailnetState::NotRunning);
    }

    #[test]
    fn the_gui_shims_prose_alone_is_unreadable() {
        let state = settle(
            &found(vec![], true),
            |_| unreachable!(),
            |_| Some(Answer::Garbled("the Tailscale CLI printed \"GUI\"".into())),
        );
        assert!(matches!(state, TailnetState::Unreadable { .. }));
    }

    #[test]
    fn an_installed_app_with_nothing_to_ask_is_not_running() {
        let state = settle(&found(vec![], true), |_| unreachable!(), |_| None);
        assert_eq!(state, TailnetState::NotRunning);
    }

    #[test]
    fn tailscale_missing_beside_another_cgnat_vpn_has_no_address() {
        // Appgate holds 100.64.10.175 on a utun, but leaves no Tailscale
        // endpoint or CLI, so nothing is reported at all.
        let found = discover(Platform::MacOs, &FakeProbes::default());
        let state = settle(&found, |_| unreachable!(), |_| None);
        assert_eq!(state, TailnetState::NotInstalled);
        assert!(state.reason().is_some());
    }

    #[test]
    fn holds_address_is_true_only_for_an_address_this_host_has() {
        assert!(holds_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        // TEST-NET-1 is never assigned to a host.
        assert!(!holds_address(IpAddr::V4(v4("192.0.2.1"))));
    }

    /// Reads this machine's real Tailscale, read-only. Run by hand:
    /// `cargo test -p lucidos-tailscale -- --ignored live`.
    #[test]
    #[ignore]
    fn live_tailscale_answers() {
        let state = tailnet_state();
        let kind = match &state {
            TailnetState::Joined(n) => format!("joined, https={}", n.can_serve_https),
            other => format!("{other:?}"),
        };
        eprintln!("live state: {kind}");
        assert!(!matches!(state, TailnetState::NotInstalled));
    }
}
