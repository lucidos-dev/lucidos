//! Where to ask Tailscale, per platform.
//!
//! These are the lookups the Tailscale CLI itself makes (its `safesocket`
//! package). Pure over a [`Platform`] and injected [`Probes`], so every
//! platform's rules run as unit tests on any machine.

use std::path::{Path, PathBuf};

use crate::cli::{resolve_tailscale_binary, BARE_CLI, TAILSCALE_CANDIDATES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// One LocalAPI endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// `tailscaled`'s unix socket.
    Unix(PathBuf),
    /// The macOS app's loopback port, with the token that proves same user.
    Tcp { port: u16, token: String },
}

/// What discovery may look at. The real one reads the filesystem and runs a
/// bounded `lsof`; tests fake it.
pub(crate) trait Probes {
    fn exists(&self, path: &Path) -> bool;
    fn read_link(&self, path: &Path) -> Option<PathBuf>;
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// `lsof -F n` output for this user's `IPNExtension` processes.
    fn ipn_extension_files(&self) -> Option<String>;
    /// `LUCIDOS_TAILSCALE_BIN`.
    fn cli_override(&self) -> Option<String>;
}

pub(crate) struct Discovery {
    /// LocalAPI endpoints, in the order to try them.
    pub endpoints: Vec<Endpoint>,
    /// The CLI to fall back on: a found path, or the bare name for `PATH`.
    pub cli: String,
    /// Something on disk says Tailscale is installed, even if nothing answers.
    pub installed: bool,
}

const LINUX_SOCKETS: &[&str] = &[
    "/var/run/tailscale/tailscaled.sock",
    "/run/tailscale/tailscaled.sock",
];
/// The open-source `tailscaled` on macOS, as Homebrew installs it.
const MACOS_SOCKET: &str = "/var/run/tailscaled.socket";
/// The standalone app's port, as the target of this symlink.
const MACOS_IPNPORT: &str = "/Library/Tailscale/ipnport";
const MACOS_SAMEUSERPROOF_DIR: &str = "/Library/Tailscale";
/// The App Store app's token file, as `lsof` lists it:
/// `.../<group>.tailscale.ipn.macos/sameuserproof-<port>-<token>`.
const APP_STORE_PROOF_MARKER: &str = ".tailscale.ipn.macos/sameuserproof-";
const MACOS_APP_BUNDLE: &str = "/Applications/Tailscale.app";

pub(crate) fn discover(platform: Platform, probes: &impl Probes) -> Discovery {
    let mut endpoints = Vec::new();
    let mut installed = false;
    match platform {
        Platform::Linux => {
            endpoints.extend(existing_sockets(LINUX_SOCKETS, probes));
        }
        Platform::MacOs => {
            endpoints.extend(existing_sockets(&[MACOS_SOCKET], probes));
            let standalone = standalone_app_endpoint(probes);
            let has_standalone = standalone.is_some();
            endpoints.extend(standalone);
            if !has_standalone {
                endpoints.extend(app_store_endpoint(probes));
            }
            installed = probes.exists(Path::new(MACOS_APP_BUNDLE));
        }
        // Windows' LocalAPI is a named pipe; the CLI is the way in there.
        Platform::Windows | Platform::Other => {}
    }
    let cli = resolve_tailscale_binary(
        probes.cli_override().as_deref(),
        TAILSCALE_CANDIDATES,
        |p| probes.exists(Path::new(p)),
    );
    let cli_on_disk = cli != BARE_CLI && probes.exists(Path::new(&cli));
    installed |= cli_on_disk || !endpoints.is_empty();
    Discovery {
        endpoints,
        cli,
        installed,
    }
}

fn existing_sockets(paths: &[&str], probes: &impl Probes) -> Vec<Endpoint> {
    paths
        .iter()
        .map(PathBuf::from)
        .filter(|p| probes.exists(p))
        .map(Endpoint::Unix)
        .collect()
}

fn standalone_app_endpoint(probes: &impl Probes) -> Option<Endpoint> {
    let target = probes.read_link(Path::new(MACOS_IPNPORT))?;
    let port: u16 = target.to_str()?.trim().parse().ok()?;
    let proof = Path::new(MACOS_SAMEUSERPROOF_DIR).join(format!("sameuserproof-{port}"));
    let token = probes.read_to_string(&proof)?.trim().to_string();
    (!token.is_empty()).then_some(Endpoint::Tcp { port, token })
}

fn app_store_endpoint(probes: &impl Probes) -> Option<Endpoint> {
    probes.ipn_extension_files()?.lines().find_map(|line| {
        let rest = &line[line.find(APP_STORE_PROOF_MARKER)? + APP_STORE_PROOF_MARKER.len()..];
        let (port, token) = rest.split_once('-')?;
        let port = port.parse().ok()?;
        (!token.is_empty()).then(|| Endpoint::Tcp {
            port,
            token: token.to_string(),
        })
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct FakeProbes {
        pub files: HashMap<PathBuf, String>,
        pub links: HashMap<PathBuf, PathBuf>,
        pub lsof: Option<String>,
        pub cli_override: Option<String>,
    }

    impl FakeProbes {
        pub(crate) fn with_file(mut self, path: &str, contents: &str) -> Self {
            self.files.insert(path.into(), contents.into());
            self
        }

        fn with_link(mut self, path: &str, target: &str) -> Self {
            self.links.insert(path.into(), target.into());
            self
        }
    }

    impl Probes for FakeProbes {
        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(path) || self.links.contains_key(path)
        }
        fn read_link(&self, path: &Path) -> Option<PathBuf> {
            self.links.get(path).cloned()
        }
        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn ipn_extension_files(&self) -> Option<String> {
            self.lsof.clone()
        }
        fn cli_override(&self) -> Option<String> {
            self.cli_override.clone()
        }
    }

    #[test]
    fn linux_asks_the_tailscaled_sockets_in_order() {
        let probes = FakeProbes::default()
            .with_file("/run/tailscale/tailscaled.sock", "")
            .with_file("/var/run/tailscale/tailscaled.sock", "")
            .with_file("/usr/bin/tailscale", "");
        let found = discover(Platform::Linux, &probes);
        assert_eq!(
            found.endpoints,
            vec![
                Endpoint::Unix("/var/run/tailscale/tailscaled.sock".into()),
                Endpoint::Unix("/run/tailscale/tailscaled.sock".into()),
            ]
        );
        assert_eq!(found.cli, "/usr/bin/tailscale");
        assert!(found.installed);
    }

    #[test]
    fn macos_standalone_app_gives_a_port_and_its_same_user_token() {
        let probes = FakeProbes::default()
            .with_link("/Library/Tailscale/ipnport", "52281")
            .with_file("/Library/Tailscale/sameuserproof-52281", "s3cret\n")
            .with_file("/Applications/Tailscale.app", "");
        let found = discover(Platform::MacOs, &probes);
        assert_eq!(
            found.endpoints,
            vec![Endpoint::Tcp {
                port: 52281,
                token: "s3cret".into()
            }]
        );
        assert!(found.installed);
    }

    #[test]
    fn macos_app_store_app_is_found_through_its_open_proof_file() {
        let probes = FakeProbes {
            lsof: Some(
                "p812\nn/usr/lib/dyld\nn/Users/me/Library/Group Containers/W5364U7YZB.io.tailscale.ipn.macos/sameuserproof-61234-abc-def\n"
                    .into(),
            ),
            ..FakeProbes::default()
        };
        let found = discover(Platform::MacOs, &probes);
        assert_eq!(
            found.endpoints,
            vec![Endpoint::Tcp {
                port: 61234,
                token: "abc-def".into()
            }]
        );
    }

    #[test]
    fn macos_homebrew_tailscaled_is_a_unix_socket() {
        let probes = FakeProbes::default().with_file("/var/run/tailscaled.socket", "");
        let found = discover(Platform::MacOs, &probes);
        assert_eq!(
            found.endpoints,
            vec![Endpoint::Unix("/var/run/tailscaled.socket".into())]
        );
    }

    #[test]
    fn a_dangling_port_link_or_empty_token_is_no_endpoint() {
        let no_proof = FakeProbes::default().with_link("/Library/Tailscale/ipnport", "52281");
        assert!(discover(Platform::MacOs, &no_proof).endpoints.is_empty());
        let empty = FakeProbes::default()
            .with_link("/Library/Tailscale/ipnport", "52281")
            .with_file("/Library/Tailscale/sameuserproof-52281", " \n");
        assert!(discover(Platform::MacOs, &empty).endpoints.is_empty());
    }

    #[test]
    fn windows_goes_through_the_installed_cli() {
        let probes =
            FakeProbes::default().with_file(r"C:\Program Files\Tailscale\tailscale.exe", "");
        let found = discover(Platform::Windows, &probes);
        assert!(found.endpoints.is_empty());
        assert_eq!(found.cli, r"C:\Program Files\Tailscale\tailscale.exe");
        assert!(found.installed);
    }

    #[test]
    fn nothing_on_disk_is_not_installed_even_beside_another_vpn() {
        // Another CGNAT VPN leaves no Tailscale socket, port or CLI behind.
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let found = discover(platform, &FakeProbes::default());
            assert!(found.endpoints.is_empty());
            assert_eq!(found.cli, "tailscale");
            assert!(!found.installed, "{platform:?}");
        }
    }

    #[test]
    fn the_cli_override_is_honoured() {
        let probes = FakeProbes {
            cli_override: Some("/opt/ts/bin/tailscale".into()),
            ..FakeProbes::default()
        };
        assert_eq!(
            discover(Platform::Linux, &probes).cli,
            "/opt/ts/bin/tailscale"
        );
    }
}
