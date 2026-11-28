use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod cert;
mod cli;
mod discovery;
mod localapi;
mod process;
mod status;

pub use cert::{cert_pair, CertPair};
pub use cli::{resolve_tailscale_binary, tailscale_binary, TAILSCALE_CANDIDATES};
pub use status::{TailnetNode, TailnetState};

use discovery::{discover, Discovery, Endpoint, Platform, Probes};

pub fn is_tailnet_addr(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

#[derive(Debug)]
pub(crate) enum Answer {
    Body(String),
    Refused { status_line: String, body: String },
    Silent,
    Garbled(String),
}

pub fn tailnet_state() -> TailnetState {
    let found = discover(Platform::current(), &SystemProbes);
    settle(&found, localapi::query, cli::status_via_cli)
}

pub fn bindable_tailnet_ipv4() -> Option<Ipv4Addr> {
    let ip = tailnet_state().node()?.ipv4;
    holds_address(ip.into()).then_some(ip)
}

pub fn holds_address(ip: IpAddr) -> bool {
    UdpSocket::bind((ip, 0)).is_ok()
}

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
            Answer::Body(body) => match status::parse_status(&body) {
                Ok(state) => return state,
                Err(detail) => TailnetState::Unreadable { detail },
            },
            Answer::Refused { status_line, .. } => TailnetState::Unreadable {
                detail: format!("LocalAPI said {status_line}"),
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
            |_| Answer::Body(running()),
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
            |_| Some(Answer::Body(running())),
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
        let found = discover(Platform::MacOs, &FakeProbes::default());
        let state = settle(&found, |_| unreachable!(), |_| None);
        assert_eq!(state, TailnetState::NotInstalled);
        assert!(state.reason().is_some());
    }

    #[test]
    fn holds_address_is_true_only_for_an_address_this_host_has() {
        assert!(holds_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!holds_address(IpAddr::V4(v4("192.0.2.1"))));
    }

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
