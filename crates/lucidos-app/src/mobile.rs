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
//! All commands are macOS-only; on other platforms they report "unsupported"
//! so the UI can hide the section.

use serde::Serialize;
use std::process::Command;

use crate::desktop::engine_port;

/// Where the connect URLs point. `lan_url` / Tailscale fields are `None` when
/// not detectable (no LAN address, Tailscale absent, etc.).
#[derive(Serialize)]
pub struct ConnectInfo {
    /// The stable gateway port the URLs use.
    pub port: u16,
    pub localhost_url: String,
    pub lan_ip: Option<String>,
    pub lan_url: Option<String>,
    pub tailscale: TailscaleInfo,
}

#[derive(Serialize)]
pub struct TailscaleInfo {
    /// The `tailscale` CLI was found on this machine.
    pub installed: bool,
    /// `tailscale up` has completed — this machine is on a tailnet.
    pub running: bool,
    /// MagicDNS name, e.g. `mymac.tailnet-name.ts.net` (no scheme).
    pub hostname: Option<String>,
    /// `https://<hostname>` — what to open on the phone, once `serve` is on.
    pub url: Option<String>,
}

/// Surface localhost / LAN / Tailscale connect URLs (mirrors the dev
/// `show_banner` in `scripts/lib/workspace.sh`).
#[tauri::command]
pub fn get_connect_info() -> ConnectInfo {
    let port = engine_port();
    let lan_ip = detect_lan_ip();
    let lan_url = lan_ip.as_ref().map(|ip| format!("http://{ip}:{port}"));
    ConnectInfo {
        port,
        localhost_url: format!("http://localhost:{port}"),
        lan_ip,
        lan_url,
        tailscale: tailscale_status(),
    }
}

/// Detect Tailscale's install + login state and the MagicDNS hostname. Folded
/// into [`get_connect_info`] (which the Mobile Access page refetches), so it's
/// an internal helper rather than its own command.
fn tailscale_status() -> TailscaleInfo {
    let Some(cli) = tailscale_cli() else {
        return TailscaleInfo {
            installed: false,
            running: false,
            hostname: None,
            url: None,
        };
    };

    // `tailscale status --json` → Self.DNSName (trailing-dot FQDN) when up.
    let hostname = Command::new(&cli)
        .args(["status", "--json"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| {
            v.get("Self")
                .and_then(|s| s.get("DNSName"))
                .and_then(|d| d.as_str())
                .map(|s| s.trim_end_matches('.').to_string())
        })
        .filter(|s| !s.is_empty());

    let running = hostname.is_some();
    let url = hostname.as_ref().map(|h| format!("https://{h}"));
    TailscaleInfo {
        installed: true,
        running,
        hostname,
        url,
    }
}

/// Bring this machine onto a tailnet (`tailscale up`). This is interactive —
/// the CLI opens a browser for the one-time tailnet login (or accepts a
/// pre-authorized auth key). Returns once the command completes; the caller
/// re-reads [`tailscale_status`] to see the result.
#[tauri::command]
pub fn tailscale_up(auth_key: Option<String>) -> Result<(), String> {
    let cli = tailscale_cli().ok_or_else(|| "Tailscale is not installed".to_string())?;
    let mut cmd = Command::new(&cli);
    cmd.arg("up");
    if let Some(key) = auth_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        cmd.arg(format!("--auth-key={key}"));
    }
    run_checked(cmd, "tailscale up")
}

/// Expose the engine over the tailnet at `https://<machine>.<tailnet>.ts.net`
/// with an auto-renewed cert (`tailscale serve`). Tailnet-private — NOT
/// `funnel` (the engine has no inbound auth). Returns the connect URL on
/// success.
#[tauri::command]
pub fn tailscale_serve() -> Result<String, String> {
    let cli = tailscale_cli().ok_or_else(|| "Tailscale is not installed".to_string())?;
    let status = tailscale_status();
    if !status.running {
        return Err("Run `tailscale up` and sign in first.".to_string());
    }
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

    status
        .url
        .ok_or_else(|| "Tailscale is up but no MagicDNS hostname is available yet.".to_string())
}

/// Locate the `tailscale` CLI: PATH first, then the GUI app's bundled binary
/// and the common Homebrew locations.
#[cfg(target_os = "macos")]
fn tailscale_cli() -> Option<String> {
    if Command::new("tailscale")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("tailscale".to_string());
    }
    const CANDIDATES: &[&str] = &[
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
    ];
    CANDIDATES
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
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
