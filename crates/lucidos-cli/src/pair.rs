//! `lucidos pair`: mint a one-time code that enrols a device.
//!
//! The gateway authenticates every network caller. A browser cannot read the
//! machine-local token file, so even a browser on this machine has to pair.
//! This command is the bootstrap: run it in a terminal, then type the code into
//! whichever device you want to let in.
//!
//! `--qr` draws the code instead, so a phone scans it. That needs an address
//! the phone can reach, which is the hard part of the whole feature: this
//! command talks to `127.0.0.1`, and a QR aimed there helps nobody. The
//! address comes from `--host`, else the gateway's external address, else
//! Tailscale's own status. The gateway builds and encodes the URL.
//!
//! It reaches the gateway rather than a workspace engine, so it finds it
//! through `crate::gateway`.

use crate::gateway;
use crate::workspace::BoxError;

/// Where the gateway's own surface lives, behind the reserved sigil namespace.
const PAIRING_CODE_PATH: &str = "/~/api/v1/auth/pairing-code";

/// How long to wait for one reachability probe. Up to four run back to back,
/// and a dead address is the ordinary answer. So this bounds what `--qr` costs on a
/// machine nothing can reach.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Everything the command was asked for.
pub struct PairArgs<'a> {
    pub port: Option<u16>,
    pub label: Option<&'a str>,
    pub qr: bool,
    pub host: Option<&'a str>,
}

pub fn cmd_pair(args: PairArgs<'_>) -> Result<(), BoxError> {
    let client = gateway::client()?;

    let base = gateway::the_gateway(
        &client,
        args.port,
        "A pairing code only works on the gateway that minted it, so pass --port to name \
         the one the new device will reach",
    )?;

    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         mint a pairing code"
            .into()
    })?;

    // The origin the NEW device should open, which is this gateway at a
    // hostname that is not loopback. `None` means we found nothing reachable,
    // and then there is no QR to draw.
    let origin = if args.qr {
        let probe = gateway::probe_client()?;
        match args.host.map(str::trim).filter(|h| !h.is_empty()) {
            // An explicit host is an assertion about a network we cannot see,
            // so it is never refused. Probing only picks which of its two
            // origins to use, and falls back to the ported one.
            Some(host) => Some(
                reachable_origin(&probe, &base, host)
                    .unwrap_or_else(|| origin_at_host(&base, host)),
            ),
            None => pairing_origin(
                crate::external_address::stored(&client, &base, &token),
                || tailnet_origin(&probe, &base),
            ),
        }
    } else {
        None
    };

    let mut request = client
        .post(format!("{base}{PAIRING_CODE_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token);
    // The label rides the code, so the device that redeems it is stored under
    // the name printed below. Without this the flag names nothing.
    if let Some(label) = args.label {
        request = request.query(&[("label", label)]);
    }
    // The gateway owns the URL shape, and validates the origin. Asking it keeps
    // one definition of what a pairing link looks like.
    if let Some(origin) = &origin {
        request = request.query(&[("origin", origin.as_str())]);
    }
    let response = request.send()?;
    if !response.status().is_success() {
        // The gateway's own sentence, not just the status. A rejected
        // `--host` returns a 400 explaining what shape an origin has to be,
        // and "400 Bad Request" alone says nothing anyone can act on.
        let status = response.status();
        let reason = response
            .json::<serde_json::Value>()
            .ok()
            .and_then(|b| b.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        return Err(format!("the gateway refused to mint a code: {reason}").into());
    }
    let body: serde_json::Value = response.json()?;
    let code = body
        .get("code")
        .and_then(|c| c.as_str())
        .ok_or_else(|| -> BoxError { "the gateway returned no code".into() })?;
    let minutes = body
        .get("expires_in_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(300)
        / 60;
    let pair_url = body.get("pair_url").and_then(|u| u.as_str());

    // User-facing CLI output that a person reads off the screen and types into
    // a phone, so bare `println!` rather than the timestamped `log!`.
    println!();
    if args.qr {
        match pair_url {
            Some(url) => match terminal_qr(url) {
                Some(qr) => {
                    print!("{qr}");
                    println!("  Scan this, or enter the code below.");
                    println!();
                }
                None => println!("  That address does not fit in a QR code."),
            },
            None => {
                println!("  Nothing here answers from outside this machine, so there is no QR.");
                println!("  Allow tailnet or LAN access under Settings -> Access -> Network");
                println!("  access, set up `tailscale serve`, set an address with");
                println!("  `lucidos external-address set`, or pass --host.");
                println!();
            }
        }
    }
    println!("  Pairing code: {code}");
    println!();
    match pair_url {
        Some(url) => println!("  Open {url} on the device you want to pair,"),
        None => println!("  Open {base}/ on the device you want to pair,"),
    }
    if let Some(label) = args.label {
        // The pairing screen offers the device a name of its own, and what the
        // person there types wins. So this is what it falls back to.
        println!("  and enter this code. Unless that device names itself,");
        println!("  it will be listed as \"{label}\".");
    } else {
        println!("  and enter this code.");
    }
    println!("  It works once and expires in {minutes} minutes.");
    println!();
    Ok(())
}

/// Pure: the pairing address, in the one order ADR 0550 sets. The external
/// address comes first, since the user typed it. Then the tailnet origin, asked
/// for only when there is no external address. The Access page's
/// `pairingOrigin` follows the same order, and adds two steps this command
/// cannot see: the reader's own origin and the LAN address.
fn pairing_origin(
    external: Option<String>,
    tailnet: impl FnOnce() -> Option<String>,
) -> Option<String> {
    external.or_else(tailnet)
}

/// The tailnet origin another device can open, or `None` with the reason
/// printed.
///
/// Tailscale is asked, through `lucidos-tailscale`, and only to FIND AN ADDRESS
/// TO PRINT, never in the auth decision, which keeps ADR 0094 intact. The
/// address and name are the ones Tailscale reports (ADR 0588).
fn tailnet_origin(client: &reqwest::blocking::Client, base: &str) -> Option<String> {
    let state = lucidos_tailscale::tailnet_state();
    let Some(node) = state.node() else {
        if let Some(reason) = state.reason() {
            eprintln!("  No tailnet address: {reason}");
        }
        return None;
    };
    let ours = our_instance(client, base, "No tailnet address")?;
    tailnet_candidates(base, node)
        .into_iter()
        .find(|origin| reaches_this_gateway(client, origin, &ours))
}

/// The tailnet origins worth trying, most useful first.
///
/// `https://<name>` leads, but only when Tailscale can certify the name: that is
/// the address with the installable app and push. Then the gateway's own port
/// at the name a person can retype, then at the bare address.
fn tailnet_candidates(base: &str, node: &lucidos_tailscale::TailnetNode) -> Vec<String> {
    let mut origins = Vec::new();
    if let Some(name) = node.https_name() {
        origins.push(format!("https://{name}"));
    }
    if let Some(name) = &node.magic_dns_name {
        origins.push(origin_at_host(base, name));
    }
    origins.push(origin_at_host(base, &node.ipv4.to_string()));
    origins.dedup();
    origins
}

/// The origin at `host` that actually answers, or `None`.
///
/// Holding a tailnet address says nothing about being reachable at it, and
/// both ways of getting that wrong ship by default. The packaged gateway binds
/// loopback, where `<name>:<port>` is dead. `tailscale serve` is the mirror
/// case: it fronts 443 on that same name and proxies to `127.0.0.1`, so it
/// answers where the gateway's own port does not.
///
/// So this asks instead of inferring, and health is the question it asks.
fn reachable_origin(client: &reqwest::blocking::Client, base: &str, host: &str) -> Option<String> {
    let ours = our_instance(client, base, "Using the gateway's own port at --host")?;
    qr_origin_candidates(base, host)
        .into_iter()
        .find(|origin| reaches_this_gateway(client, origin, &ours))
}

/// The origins worth trying for `host`, most specific first.
///
/// Always `https` for the bare one: `serve` terminates TLS itself, even when
/// the gateway behind it speaks plain HTTP.
fn qr_origin_candidates(base: &str, host: &str) -> Vec<String> {
    let ported = origin_at_host(base, host);
    let served = format!("https://{host}");
    if ported == served {
        vec![ported]
    } else {
        vec![ported, served]
    }
}

/// Does `origin` reach the gateway whose health carries `ours`?
///
/// Two gateways can run on one machine, and `serve` on 443 fronts only one of
/// them. A pairing code works only on the gateway that minted it, so an answer
/// from the other one is a dead QR. So the answer must carry our id, and a
/// gateway older than `instance_id` proves nothing.
fn reaches_this_gateway(client: &reqwest::blocking::Client, origin: &str, ours: &str) -> bool {
    health_instance(client, origin).as_deref() == Some(ours)
}

/// This gateway's `instance_id` over loopback, or `None` with `outcome` and
/// the reason printed.
fn our_instance(client: &reqwest::blocking::Client, base: &str, outcome: &str) -> Option<String> {
    let ours = health_instance(client, base);
    if ours.is_none() {
        eprintln!(
            "  {outcome}: the gateway is older than this CLI and cannot prove which \
             gateway answers there. Restart Lucidos to update it."
        );
    }
    ours
}

/// The `instance_id` the gateway's health answers with at `origin`.
fn health_instance(client: &reqwest::blocking::Client, origin: &str) -> Option<String> {
    health(client, origin)?
        .get("instance_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The health body at `origin`, or `None` when nothing answered in time.
///
/// Its own short timeout: several of these run back to back, and a dead
/// address is the ordinary answer rather than the exceptional one.
fn health(client: &reqwest::blocking::Client, origin: &str) -> Option<serde_json::Value> {
    client
        .get(format!("{origin}{}", gateway::HEALTH_PATH))
        .timeout(PROBE_TIMEOUT)
        .send()
        .ok()
        .filter(|r| r.status().is_success())?
        .json()
        .ok()
}

/// `base` with its hostname swapped for `host`, keeping the scheme and port.
///
/// The same move `MobileAccessPage`'s `originAtHost` makes, and for the same
/// reason: whatever answered at `127.0.0.1:<port>` answers at `<host>:<port>`.
fn origin_at_host(base: &str, host: &str) -> String {
    let (scheme, authority) = base.split_once("://").unwrap_or(("https", base));
    match authority.rsplit_once(':') {
        Some((_, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            format!("{scheme}://{host}:{port}")
        }
        _ => format!("{scheme}://{host}"),
    }
}

/// `data` as a QR made of half-block characters, or `None` if it does not fit.
///
/// Black on white, set explicitly. Half the terminals in use are dark, and the
/// glyphs would then be light modules on a dark field. That is an inverted QR,
/// which many scanners refuse. `NO_COLOR` drops the escapes for anyone who has
/// asked for no colour, and their terminal's own theme then decides.
fn terminal_qr(data: &str) -> Option<String> {
    let plain = std::env::var_os("NO_COLOR").is_some();
    Some(colorize(&qr_block(data)?, plain))
}

/// The bare half-block QR, before any colour is put around it.
fn qr_block(data: &str) -> Option<String> {
    use qrcode::render::unicode;
    let code = qrcode::QrCode::new(data).ok()?;
    Some(
        code.render::<unicode::Dense1x2>()
            .quiet_zone(true)
            .dark_color(unicode::Dense1x2::Dark)
            .light_color(unicode::Dense1x2::Light)
            .build(),
    )
}

/// Wrap every line in black-on-white, unless the caller asked for no colour.
/// Split out so the escaping is testable without a terminal.
fn colorize(block: &str, plain: bool) -> String {
    let mut out = String::with_capacity(block.len() * 2);
    for line in block.lines() {
        if plain {
            out.push_str(line);
        } else {
            out.push_str("\u{1b}[30;47m");
            out.push_str(line);
            out.push_str("\u{1b}[0m");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loopback origin that answers one request with `response`.
    fn answer_once(response: String) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.read(&mut [0u8; 4096]);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        origin
    }

    #[test]
    fn an_identity_probe_does_not_follow_a_redirect() {
        let body = r#"{"instance_id":"ours"}"#;
        let ours = answer_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        let elsewhere = answer_once(format!(
            "HTTP/1.1 302 Found\r\nLocation: {ours}{}\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n",
            gateway::HEALTH_PATH
        ));
        let probe = gateway::probe_client().unwrap();
        assert!(!reaches_this_gateway(&probe, &elsewhere, "ours"));
    }

    #[test]
    fn the_external_address_leads_and_spares_the_tailnet_probe() {
        let external = Some("https://lucidos.example.com".to_string());
        let origin = pairing_origin(external.clone(), || {
            panic!("the tailnet must not be probed when an external address is set")
        });
        assert_eq!(origin, external);
    }

    #[test]
    fn without_an_external_address_the_tailnet_answers() {
        let tailnet = || Some("http://mymac.tailnet-name.ts.net:5252".to_string());
        assert_eq!(
            pairing_origin(None, tailnet).as_deref(),
            Some("http://mymac.tailnet-name.ts.net:5252")
        );
        assert_eq!(pairing_origin(None, || None), None);
    }

    #[test]
    fn swapping_the_host_keeps_the_scheme_and_the_port() {
        // The gateway is found at loopback and reached from a phone by name.
        // Everything but the hostname has to survive that swap.
        assert_eq!(
            origin_at_host("https://127.0.0.1:5251", "mac.tail1234.ts.net"),
            "https://mac.tail1234.ts.net:5251"
        );
        assert_eq!(
            origin_at_host("http://127.0.0.1:5252", "100.64.0.7"),
            "http://100.64.0.7:5252"
        );
    }

    #[test]
    fn a_served_tailnet_name_is_probed_as_well_as_the_gateway_port() {
        // Both default setups get this wrong in opposite directions, so both
        // origins have to be on the list. Under the packaged loopback bind
        // `<name>:<port>` is dead; under `tailscale serve` the bare name is
        // the live one, over TLS the gateway itself may not speak.
        assert_eq!(
            qr_origin_candidates("http://127.0.0.1:5252", "mac.tail1234.ts.net"),
            vec![
                "http://mac.tail1234.ts.net:5252".to_string(),
                "https://mac.tail1234.ts.net".to_string(),
            ]
        );
    }

    #[test]
    fn a_portless_https_base_yields_one_candidate_rather_than_two_identical_ones() {
        assert_eq!(
            qr_origin_candidates("https://127.0.0.1", "mac.ts.net"),
            vec!["https://mac.ts.net".to_string()]
        );
    }

    fn node(name: Option<&str>, https: bool) -> lucidos_tailscale::TailnetNode {
        lucidos_tailscale::TailnetNode {
            ipv4: std::net::Ipv4Addr::new(100, 70, 0, 1),
            magic_dns_name: name.map(str::to_string),
            can_serve_https: https,
        }
    }

    #[test]
    fn a_certifiable_name_is_tried_over_https_first() {
        assert_eq!(
            tailnet_candidates(
                "http://127.0.0.1:5252",
                &node(Some("mymac.tailnet-name.ts.net"), true)
            ),
            vec![
                "https://mymac.tailnet-name.ts.net".to_string(),
                "http://mymac.tailnet-name.ts.net:5252".to_string(),
                "http://100.70.0.1:5252".to_string(),
            ]
        );
    }

    #[test]
    fn without_a_certificate_there_is_no_https_candidate() {
        // Headscale issues no certificates, so no serve URL can exist.
        assert_eq!(
            tailnet_candidates(
                "http://127.0.0.1:5252",
                &node(Some("mymac.example-tailnet.test"), false)
            ),
            vec![
                "http://mymac.example-tailnet.test:5252".to_string(),
                "http://100.70.0.1:5252".to_string(),
            ]
        );
        // MagicDNS off: the address Tailscale reported is all there is.
        assert_eq!(
            tailnet_candidates("http://127.0.0.1:5252", &node(None, false)),
            vec!["http://100.70.0.1:5252".to_string()]
        );
    }

    #[test]
    fn a_base_with_no_port_keeps_none() {
        assert_eq!(
            origin_at_host("https://127.0.0.1", "mac.ts.net"),
            "https://mac.ts.net"
        );
    }

    #[test]
    fn the_terminal_qr_is_black_on_white_unless_colour_is_declined() {
        let qr = terminal_qr("https://mac.tail1234.ts.net/~/?pair=01234567").unwrap();
        // An inverted QR is one many scanners refuse, and a dark terminal makes
        // every unstyled one inverted.
        assert!(qr.contains("\u{1b}[30;47m") || std::env::var_os("NO_COLOR").is_some());

        let plain = colorize("ab\ncd", true);
        assert_eq!(plain, "ab\ncd\n");
        assert!(!plain.contains('\u{1b}'));
    }

    #[test]
    fn a_pairing_qr_fits_an_ordinary_terminal() {
        // A QR wider than the window wraps, and a wrapped QR scans as nothing.
        // A MagicDNS pairing URL lands at version 3, so 29 modules plus the
        // 8-module quiet zone. The unicode renderer packs two rows per line.
        let block = qr_block("https://mac.tail1234.ts.net/~/?pair=01234567").unwrap();
        let widest = block.lines().map(|l| l.chars().count()).max().unwrap();
        assert!(
            widest <= 80,
            "a QR {widest} columns wide wraps on most terminals"
        );
    }

    #[test]
    fn every_line_is_wrapped_and_reset() {
        // A line left unreset paints the rest of the terminal white.
        let painted = colorize("ab\ncd", false);
        assert_eq!(
            painted,
            "\u{1b}[30;47mab\u{1b}[0m\n\u{1b}[30;47mcd\u{1b}[0m\n"
        );
    }
}
