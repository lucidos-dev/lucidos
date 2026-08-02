//! Mobile access for the packaged desktop app: surface the connect URLs and
//! drive the Tailscale setup the user consents to in the UI.
//!
//! The packaged gateway binds the [stable port](crate::desktop), so Lucidos is
//! reachable on `localhost`, the LAN, and — once Tailscale is set up — at a
//! tailnet-private `https://<machine>.<tailnet>.ts.net` URL with an auto-renewed
//! cert (full PWA + web push, works off-LAN). Workspace engines stay behind the
//! gateway on loopback-only ports.
//!
//! We use `tailscale serve` (tailnet-private), NOT `tailscale funnel` (public):
//! Lucidos has **no inbound API auth**, so it must never be exposed to the open
//! internet. Mobile devices reach it by joining the same tailnet.
//!
//! The Mac side is scriptable only *after the user consents* — Tailscale is a
//! system VPN whose install/login can't be silent. The phone side is guided
//! (install Tailscale, join the tailnet), never silent — OS sandboxing prevents
//! remote install/login.
//!
//! # Reading state never runs the CLI
//!
//! Tailnet membership and the MagicDNS name come from `lucidos-tailscale`, which
//! reads the interface list and does a reverse lookup. Two reasons, one of which
//! is a shipped bug:
//!
//! - This page is reached from a phone as often as from the Mac, and a user
//!   whose Tailscale already works should never be told to install a CLI just so
//!   we can describe their own machine back to them.
//! - The CLI probe used to pick `/Applications/Tailscale.app/Contents/MacOS/
//!   Tailscale`, which is the GUI executable. It **exits 0** while printing "The
//!   Tailscale GUI failed to start ... (Tailscale.CLIError error 3)", so every
//!   check read as a success with unparseable output: the page showed **Sign
//!   in** to a machine already on its tailnet, and pressing it reported success
//!   and changed nothing.
//!
//! A CLI is still required for [`tailscale_serve`] (and [`tailscale_up`] where
//! one exists), because `serve` has no GUI, config-file or admin-console
//! equivalent. That is the only thing it gates: a Mac without a CLI still gets
//! an accurate description of itself.
//!
//! Because exit 0 turned out to be a lie, nothing here trusts it alone. The CLI
//! probe demands a parseable version, and every action re-reads the world
//! afterwards and fails when it did not move.

use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

use crate::desktop::engine_port;

/// Bound on the MagicDNS reverse lookup. The resolver is local (`100.100.100.100`)
/// whenever we get this far, so this is a stall guard, not a budget.
const REVERSE_DNS_TIMEOUT: Duration = Duration::from_millis(1500);

/// Bound on the "is anything serving HTTPS" probe. Loopback-speed in practice.
const SERVE_PROBE_TIMEOUT: Duration = Duration::from_millis(700);

/// Bound on `tailscale version`, the one CLI call the status path makes. Answers
/// in milliseconds when healthy; the ceiling exists for when it does not.
const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The port `tailscale serve` fronts, and the one [`tailscale_serve`] configures.
const TAILNET_HTTPS_PORT: u16 = 443;

/// Where the connect URLs point. `lan_ip` / Tailscale fields are `None` when
/// not detectable (no LAN address, Tailscale absent, etc.). The LAN *URL* is
/// deliberately not pre-built here: whether a LAN address is reachable at all
/// depends on the gateway's network bind (loopback-only by default in
/// packaged), which the frontend reads from `GET /api/v1/network-config` and
/// combines with `lan_ip` + `port` (see `MobileAccessPage.tsx::lanRowState`).
#[derive(Serialize)]
pub struct ConnectInfo {
    /// The stable gateway port the URLs use.
    pub port: u16,
    pub localhost_url: String,
    pub lan_ip: Option<String>,
    pub tailscale: TailscaleInfo,
}

/// What this Mac's Tailscale setup looks like, as two independent facts.
///
/// **Tailnet state** (`on_tailnet`, `tailnet_ip`, `magic_dns_name`, `serve_url`)
/// is read without a CLI. **CLI availability** (`cli_available`) gates the
/// buttons and nothing else. Keeping them apart is what lets the page stay
/// accurate on a Mac that has Tailscale working but no CLI installed, which
/// before this split rendered as a Sign in button that could not work.
#[derive(Serialize)]
pub struct TailscaleInfo {
    /// Tailscale is present at all: the app bundle, or a CLI. Drives the
    /// "Get Tailscale" offer, so it deliberately does NOT mean "usable".
    pub installed: bool,
    /// This Mac holds a tailnet address, so it is signed in and connected.
    pub on_tailnet: bool,
    /// The tailnet IPv4, when on a tailnet. Reachable over plain HTTP from any
    /// device on the same tailnet, which is Route A in `remote-access.md`.
    pub tailnet_ip: Option<String>,
    /// MagicDNS name, e.g. `mymac.tailnet-name.ts.net` (no scheme). `None` on a
    /// tailnet with MagicDNS disabled, which is not the same as being offline.
    pub magic_dns_name: Option<String>,
    /// `https://<magic_dns_name>`, set **only** once something is proven to be
    /// serving it. Before `tailscale serve` runs there is no listener on 443, so
    /// publishing the URL earlier would advertise a dead address as the one that
    /// carries the PWA and push.
    pub serve_url: Option<String>,
    /// A working `tailscale` CLI was found. Gates the actions only: never the
    /// reporting above.
    pub cli_available: bool,
}

/// Surface localhost / LAN / Tailscale connect URLs (mirrors the dev
/// `show_banner` in `scripts/lib/workspace.sh`).
#[tauri::command]
pub fn get_connect_info() -> ConnectInfo {
    let port = engine_port();
    ConnectInfo {
        port,
        localhost_url: format!("http://localhost:{port}"),
        lan_ip: detect_lan_ip(),
        tailscale: tailscale_status(),
    }
}

/// Detect this Mac's tailnet state and whether a CLI is available. Folded into
/// [`get_connect_info`] (which the Mobile Access page refetches), so it's an
/// internal helper rather than its own command.
fn tailscale_status() -> TailscaleInfo {
    let cli = tailscale_cli();
    let tailnet_addr = lucidos_tailscale::tailnet_ipv4();
    let magic_dns_name =
        tailnet_addr.and_then(|ip| lucidos_tailscale::magic_dns_name(ip, REVERSE_DNS_TIMEOUT));

    // Only a name we can show is answering becomes a URL. See `serve_is_live`.
    let serve_url = magic_dns_name
        .as_ref()
        .filter(|_| tailnet_addr.is_some_and(serve_is_live))
        .map(|h| format!("https://{h}"));

    TailscaleInfo {
        // The app bundle counts even without a CLI: offering "Get Tailscale" to
        // someone who already has it would be the wrong instruction.
        installed: cli.is_some() || std::path::Path::new(TAILSCALE_APP_BUNDLE).exists(),
        on_tailnet: tailnet_addr.is_some(),
        tailnet_ip: tailnet_addr.map(|ip| ip.to_string()),
        magic_dns_name,
        serve_url,
        cli_available: cli.is_some(),
    }
}

/// The macOS GUI app bundle. Its presence means Tailscale is installed; its
/// executable is NOT a CLI and is never run (see the module docs and
/// `lucidos_tailscale::TAILSCALE_CANDIDATES`).
const TAILSCALE_APP_BUNDLE: &str = "/Applications/Tailscale.app";

/// Is the address we are about to publish actually answering?
///
/// Deliberately a bounded TCP connect to **443 on the tailnet address**, and
/// deliberately NOT `tailscale serve status`. Asking the CLI whether *a* serve
/// mapping exists answers the wrong question: `remote-access.md` documents a
/// second gateway served on 8443, and a config containing only that mapping is
/// non-empty while `https://<name>` on 443 stays dead. The UI publishes exactly
/// one URL, so the only honest test is whether exactly that endpoint responds.
///
/// It proves a listener, not a working certificate. Tailscale can be listening
/// while a first-run cert is still provisioning, which is why
/// [`tailscale_serve`] says so in its own failure message rather than implying
/// the address is broken.
fn serve_is_live(addr: Ipv4Addr) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((addr, TAILNET_HTTPS_PORT)),
        SERVE_PROBE_TIMEOUT,
    )
    .is_ok()
}

/// Run a command with a hard deadline, killing it if it overruns.
///
/// Every CLI probe on the status path goes through this. `get_connect_info` is
/// awaited by a settings pane, and `std::process::Command::output()` waits
/// forever: a wedged `tailscaled` would leave the pane loading with nothing to
/// show and no way to know why. A timeout is `None`, which every caller already
/// treats as "no usable CLI".
///
/// Only safe for commands with SMALL output, which is why it is private and used
/// solely for `version`. It waits for exit before reading the pipes, so a child
/// that filled a pipe buffer would block instead of exiting.
fn output_with_timeout(mut cmd: Command, timeout: Duration) -> Option<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

/// Bring this machine onto a tailnet (`tailscale up`). This is interactive —
/// the CLI opens a browser for the one-time tailnet login (or accepts a
/// pre-authorized auth key). Returns once the command completes; the caller
/// re-reads [`tailscale_status`] to see the result.
#[tauri::command]
pub fn tailscale_up(auth_key: Option<String>) -> Result<(), String> {
    let cli = tailscale_cli().ok_or_else(|| NO_CLI.to_string())?;
    let mut cmd = Command::new(&cli);
    cmd.arg("up");
    if let Some(key) = auth_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        cmd.arg(format!("--auth-key={key}"));
    }
    run_checked(cmd, "tailscale up")?;
    // Post-condition, not ceremony: the reported bug was `up` "succeeding" and
    // changing nothing, which the page could only render as a reload.
    if lucidos_tailscale::tailnet_ipv4().is_none() {
        return Err(
            "tailscale up reported success but this Mac still has no tailnet address. \
             Open the Tailscale app and check it is signed in."
                .to_string(),
        );
    }
    Ok(())
}

/// What to say when an action needs a CLI and there is none. Names the two ways
/// to get one, since the user plainly has Tailscale working without it.
const NO_CLI: &str = "The Tailscale command-line tool isn't available. \
     Install it from the Tailscale app (Install CLI), or with `brew install tailscale`.";

/// Expose the engine over the tailnet at `https://<machine>.<tailnet>.ts.net`
/// with an auto-renewed cert (`tailscale serve`). Tailnet-private — NOT
/// `funnel` (the engine has no inbound auth). Returns the connect URL on
/// success.
#[tauri::command]
pub fn tailscale_serve() -> Result<String, String> {
    let cli = tailscale_cli().ok_or_else(|| NO_CLI.to_string())?;
    let addr = lucidos_tailscale::tailnet_ipv4().ok_or_else(|| {
        "This Mac is not on a tailnet yet. Sign in to Tailscale first.".to_string()
    })?;
    let name = lucidos_tailscale::magic_dns_name(addr, REVERSE_DNS_TIMEOUT).ok_or_else(|| {
        "This Mac is on a tailnet but has no MagicDNS name, so there is no HTTPS \
         address to serve. Enable MagicDNS for your tailnet and try again."
            .to_string()
    })?;
    let port = engine_port();

    // `tailscale serve --bg https / http://127.0.0.1:<port>` proxies the tailnet
    // HTTPS endpoint at the root path to the loopback engine, in the background
    // (survives this process). The `https /` form pins it to the HTTPS handler.
    let mut cmd = Command::new(&cli);
    cmd.args([
        "serve",
        "--bg",
        "https",
        "/",
        &format!("http://127.0.0.1:{port}"),
    ]);
    run_checked(cmd, "tailscale serve")?;

    // Same post-condition rule as `up`: only report the URL we can show is live.
    // A cert can still be provisioning on the very first run, so say that rather
    // than hand back an address that will not load yet.
    if !serve_is_live(addr) {
        return Err(format!(
            "tailscale serve reported success but nothing is answering on {name}. \
             If this is the first time, the certificate may still be provisioning; \
             `tailscale serve status` shows what was configured."
        ));
    }
    Ok(format!("https://{name}"))
}

/// Locate a **working** `tailscale` CLI, or `None`.
///
/// Resolution is `lucidos-tailscale`'s: an env override, then absolute paths,
/// then the bare name for a shell that has a `PATH`. The old list here led with
/// `/Applications/Tailscale.app/Contents/MacOS/Tailscale`, the GUI executable,
/// which existed on every Mac with Tailscale and shadowed the real CLI beside
/// it.
///
/// The probe then demands a **parseable version**, not merely a zero exit,
/// because that GUI binary exits 0 while printing an error. Anything that
/// cannot say what version it is does not get to be our CLI.
#[cfg(target_os = "macos")]
fn tailscale_cli() -> Option<String> {
    let bin = lucidos_tailscale::tailscale_binary();
    // Bounded: this runs on every Mobile Access load, and a wedged CLI or
    // daemon would otherwise hang the pane on its loading state forever.
    let mut probe = Command::new(&bin);
    probe.arg("version");
    let out = output_with_timeout(probe, CLI_PROBE_TIMEOUT)?;
    if !out.status.success() {
        return None;
    }
    reports_a_version(&String::from_utf8_lossy(&out.stdout)).then_some(bin)
}

/// Does this `tailscale version` output come from something that actually is
/// the CLI?
///
/// Pure so the exit-0 liar can be pinned by a test rather than by a Mac. Real
/// output opens with the version itself (`1.96.4-t41cb72f27`); the GUI
/// executable opens with an apology and exits 0 all the same.
fn reports_a_version(stdout: &str) -> bool {
    let first = stdout.lines().next().unwrap_or_default().trim();
    let major = first.split('.').next().unwrap_or_default();
    !major.is_empty() && major.chars().all(|c| c.is_ascii_digit())
}

#[cfg(not(target_os = "macos"))]
fn tailscale_cli() -> Option<String> {
    None
}

/// Best LAN IPv4 for this machine (`ipconfig getifaddr en0`, then `en1`).
/// macOS-only; `None` elsewhere or when offline.
#[cfg(target_os = "macos")]
fn detect_lan_ip() -> Option<String> {
    for iface in ["en0", "en1"] {
        if let Ok(out) = Command::new("ipconfig").args(["getifaddr", iface]).output() {
            if out.status.success() {
                let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !ip.is_empty() {
                    return Some(ip);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn detect_lan_ip() -> Option<String> {
    None
}

/// Run a command, mapping a non-zero exit (or spawn failure) to a readable
/// error carrying the captured stderr.
fn run_checked(mut cmd: Command, label: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("{label} failed ({})", out.status)
    } else {
        format!("{label} failed: {stderr}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macos_gui_executable_is_not_accepted_as_a_cli() {
        // Verbatim from /Applications/Tailscale.app/Contents/MacOS/Tailscale run
        // under the packaged environment. It goes to STDOUT and the process
        // EXITS 0, so an exit-code check alone reads it as a working CLI. That
        // is exactly how Mobile Access came to show a Sign in button that
        // silently did nothing on a Mac already on its tailnet.
        assert!(!reports_a_version(
            "The Tailscale GUI failed to start: The operation couldn't be completed. \
             (Tailscale.CLIError error 3.)"
        ));
    }

    #[test]
    fn real_tailscale_version_output_is_accepted() {
        // `tailscale version` leads with the version and then adds build lines.
        assert!(reports_a_version(
            "1.96.4-t41cb72f27\n  go version: go1.24.0\n"
        ));
        assert!(reports_a_version("1.98.9\n"));
    }

    #[test]
    fn a_hung_probe_is_killed_at_the_deadline() {
        // `get_connect_info` is awaited by a settings pane, so an unbounded
        // `.output()` on a wedged CLI leaves it loading forever with nothing to
        // show. `sleep` stands in for that: the call must give up, not block.
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        let start = std::time::Instant::now();
        assert!(output_with_timeout(cmd, Duration::from_millis(200)).is_none());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "took {:?}, so the deadline did not fire",
            start.elapsed()
        );
    }

    #[test]
    fn a_prompt_command_still_returns_its_output() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("1.96.4");
        let out = output_with_timeout(cmd, Duration::from_secs(5)).expect("echo should answer");
        assert!(out.status.success());
        assert!(reports_a_version(&String::from_utf8_lossy(&out.stdout)));
    }

    #[test]
    fn empty_or_wordy_output_is_not_a_version() {
        assert!(!reports_a_version(""));
        assert!(!reports_a_version("\n"));
        assert!(!reports_a_version("command not found"));
        // A leading blank line is not a version either: the version must be the
        // first thing said, as it is in every real build.
        assert!(!reports_a_version("\n1.96.4"));
    }
}
