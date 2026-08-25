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
//! hostname comes from Tailscale or from `--host`, and the gateway builds and
//! encodes the URL.
//!
//! It reaches the gateway rather than a workspace engine, so it resolves its
//! own target instead of going through `crate::workspace`.

use crate::workspace::BoxError;

/// Gateway ports to try, in order. The packaged gateway holds 5252 and the dev
/// one 5251, deliberately one apart so both can run at once. Probing beats
/// asking the user, since most people have exactly one.
const DEFAULT_PORTS: [u16; 2] = [5252, 5251];

/// Where the gateway's own surface lives, behind the reserved sigil namespace.
const PAIRING_CODE_PATH: &str = "/~/api/v1/auth/pairing-code";
const HEALTH_PATH: &str = "/~/api/v1/health";

/// How long to wait for the MagicDNS reverse lookup. Short: a name we cannot
/// resolve promptly is one we fall back from, and the tailnet address works.
const MAGIC_DNS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(700);

/// How long to wait for one reachability probe. Two run back to back, and a
/// dead address is the ordinary answer. So this bounds what `--qr` costs on a
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
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let base = match args.port {
        Some(p) => find_gateways(&client, &[p])
            .into_iter()
            .next()
            .map(|g| g.base)
            .ok_or_else(|| -> BoxError { format!("no gateway answered on port {p}").into() })?,
        None => {
            let found = find_gateways(&client, &candidate_ports());
            choose_gateway(&found, dev_port_override())?
        }
    };

    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         mint a pairing code"
            .into()
    })?;

    // The origin the NEW device should open, which is this gateway at a
    // hostname that is not loopback. `None` means we found nothing reachable,
    // and then there is no QR to draw.
    let origin = if args.qr {
        match args.host.map(str::trim).filter(|h| !h.is_empty()) {
            // An explicit host is an assertion about a network we cannot see,
            // so it is never refused. Probing only picks which of its two
            // origins to use, and falls back to the ported one.
            Some(host) => Some(
                reachable_origin(&client, &base, host)
                    .unwrap_or_else(|| origin_at_host(&base, host)),
            ),
            None => reachable_host().and_then(|host| reachable_origin(&client, &base, &host)),
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
                println!("  access, set up `tailscale serve`, or pass --host.");
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

/// A hostname another device on the tailnet can open, or `None`.
///
/// Tailscale is used to FIND A NAME TO PRINT, never in the auth decision, which
/// is what keeps ADR 0094 intact. Both lookups read the interface list with no
/// subprocess. The MagicDNS name comes first, because it is the one a person
/// can retype.
fn reachable_host() -> Option<String> {
    let addr = lucidos_tailscale::tailnet_ipv4()?;
    Some(
        lucidos_tailscale::magic_dns_name(addr, MAGIC_DNS_TIMEOUT)
            .unwrap_or_else(|| addr.to_string()),
    )
}

/// The origin at `host` that actually answers, or `None`.
///
/// Holding a tailnet address says nothing about being reachable at it, and
/// both ways of getting that wrong ship by default. The packaged gateway binds
/// loopback, where `<name>:<port>` is dead. `tailscale serve` is the mirror
/// case: it fronts 443 on that same name and proxies to `127.0.0.1`, so it
/// answers where the gateway's own port does not.
///
/// So this asks instead of inferring, and health is the question it asks. Same
/// standard the Access page holds itself to, reached a different way: the page
/// reads the configured bind, and a terminal can just knock on the door.
fn reachable_origin(client: &reqwest::blocking::Client, base: &str, host: &str) -> Option<String> {
    qr_origin_candidates(base, host)
        .into_iter()
        .find(|origin| answers_health(client, origin))
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

/// Does a gateway answer its health endpoint at `origin`?
///
/// Its own short-lived client: two of these run back to back, and a dead
/// address is the ordinary answer rather than the exceptional one.
fn answers_health(client: &reqwest::blocking::Client, origin: &str) -> bool {
    client
        .get(format!("{origin}{HEALTH_PATH}"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .is_ok_and(|r| r.status().is_success())
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

/// Ports to probe: the dev override from the environment, then the defaults.
fn candidate_ports() -> Vec<u16> {
    candidate_ports_from(dev_port_override())
}

/// The port `LUCIDOS_DEV_GATEWAY_PORT` names, when it names a valid one.
///
/// Setting it is a deliberate pick, so [`choose_gateway`] takes that gateway
/// even with another one answering beside it.
fn dev_port_override() -> Option<u16> {
    std::env::var("LUCIDOS_DEV_GATEWAY_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
}

/// The pure half, so the ordering is table-tested without touching the
/// environment. `set_var` is process-global and Rust runs tests in parallel
/// threads, so two tests sharing one variable race each other.
fn candidate_ports_from(override_port: Option<u16>) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Some(p) = override_port {
        ports.push(p);
    }
    for p in DEFAULT_PORTS {
        if !ports.contains(&p) {
            ports.push(p);
        }
    }
    ports
}

/// One gateway that answered its health endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FoundGateway {
    port: u16,
    base: String,
}

/// Every gateway that answers, in probe order.
///
/// Both schemes are tried per port. The dev gateway serves TLS and the packaged
/// one serves plain HTTP, and this command cannot know which it faces. All of
/// them rather than the first, because [`choose_gateway`] has to see a second
/// one to refuse it.
fn find_gateways(client: &reqwest::blocking::Client, ports: &[u16]) -> Vec<FoundGateway> {
    let mut found = Vec::new();
    for &port in ports {
        for scheme in ["https", "http"] {
            let base = format!("{scheme}://127.0.0.1:{port}");
            if client
                .get(format!("{base}{HEALTH_PATH}"))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                found.push(FoundGateway { port, base });
                // One gateway per port. The other scheme is the same process
                // refusing a handshake, never a second gateway.
                break;
            }
        }
    }
    found
}

/// Pure: which gateway to mint on, or why the caller has to say.
///
/// Ambiguity is refused rather than guessed. A code and the device it enrols
/// both live on the gateway that minted them. Minting on the wrong one hands
/// the user a code their phone is then told has expired. That is invisible:
/// both gateways answer, and the refusal names no port.
///
/// `preferred` is a port the caller chose, through `--port` or the dev
/// override. It wins whenever it answered.
fn choose_gateway(found: &[FoundGateway], preferred: Option<u16>) -> Result<String, BoxError> {
    if let Some(hit) = preferred.and_then(|p| found.iter().find(|g| g.port == p)) {
        return Ok(hit.base.clone());
    }
    match found {
        [] => Err("no gateway found. Start one, or pass --port if it is on an unusual port".into()),
        [only] => Ok(only.base.clone()),
        many => {
            let ports: Vec<String> = many.iter().map(|g| g.port.to_string()).collect();
            Err(format!(
                "{} gateways are running (ports {}). A pairing code only works on the gateway \
                 that minted it, so pass --port to name the one the new device will reach",
                many.len(),
                ports.join(", ")
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_dev_override_is_probed_before_the_defaults() {
        // Set on a dev machine that also runs a packaged install, where probing
        // 5252 first would pair the wrong gateway.
        assert_eq!(candidate_ports_from(Some(5999)), vec![5999, 5252, 5251]);
    }

    #[test]
    fn an_override_equal_to_a_default_is_not_listed_twice() {
        assert_eq!(candidate_ports_from(Some(5251)), vec![5251, 5252]);
        assert_eq!(candidate_ports_from(Some(5252)), vec![5252, 5251]);
    }

    #[test]
    fn the_defaults_are_probed_when_nothing_is_set() {
        // Packaged first: it is the one an ordinary install has.
        assert_eq!(candidate_ports_from(None), vec![5252, 5251]);
    }

    fn found(ports: &[u16]) -> Vec<FoundGateway> {
        ports
            .iter()
            .map(|&port| FoundGateway {
                port,
                base: format!("https://127.0.0.1:{port}"),
            })
            .collect()
    }

    #[test]
    fn one_running_gateway_is_used_without_asking() {
        let only = found(&[5252]);
        assert_eq!(
            choose_gateway(&only, None).unwrap(),
            "https://127.0.0.1:5252"
        );
    }

    #[test]
    fn two_running_gateways_are_refused_rather_than_guessed() {
        // The reported bug. Both answer, the first wins, and the code is minted
        // on a gateway the new device may never reach. Its own store holds the
        // device, so the phone is told the code expired.
        let both = found(&[5252, 5251]);
        let err = choose_gateway(&both, None).unwrap_err().to_string();
        assert!(err.contains("5252") && err.contains("5251"), "{err}");
        assert!(err.contains("--port"), "the refusal must say what to do");
    }

    #[test]
    fn a_preferred_port_wins_over_a_second_gateway() {
        // `--port` and the dev override are both the caller saying which one.
        let both = found(&[5252, 5251]);
        assert_eq!(
            choose_gateway(&both, Some(5251)).unwrap(),
            "https://127.0.0.1:5251"
        );
    }

    #[test]
    fn a_preferred_port_that_did_not_answer_falls_back_to_the_rules() {
        // A stale override must not mint on a gateway nobody chose either.
        let both = found(&[5252, 5251]);
        assert!(choose_gateway(&both, Some(5999)).is_err());
        let one = found(&[5252]);
        assert_eq!(
            choose_gateway(&one, Some(5999)).unwrap(),
            "https://127.0.0.1:5252"
        );
    }

    #[test]
    fn no_gateway_at_all_says_how_to_start_one() {
        let err = choose_gateway(&[], None).unwrap_err().to_string();
        assert!(err.contains("no gateway found"), "{err}");
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
