use crate::gateway;
use crate::workspace::BoxError;
use lucidos_gateway_constants::{PAIR_PARAM, SIGIL};

const PAIRING_CODE_PATH: &str = "/~/api/v1/auth/pairing-code";

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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

    let token = local_token()?;

    let origin = if args.qr {
        let probe = Probe::new(&client, &base)?;
        match args.host.map(str::trim).filter(|h| !h.is_empty()) {
            Some(host) => Some(
                reachable_origin(&probe, host)
                    .unwrap_or_else(|| origin_at_host(&probe.off_loopback_base, host)),
            ),
            None => pairing_origin(
                crate::external_address::stored(&client, &base, &token),
                || gateway_https_origin(&probe),
                || tailnet_origin(&probe),
            ),
        }
    } else {
        crate::external_address::stored(&client, &base, &token)
    };

    let waits = crate::devices::any_paired(&client, &base, &token) == Some(true);
    let mut request = client
        .post(format!("{base}{PAIRING_CODE_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token);
    if let Some(label) = args.label {
        request = request.query(&[("label", label)]);
    }
    if let Some(origin) = &origin {
        request = request.query(&[("origin", origin.as_str())]);
    }
    let Minted {
        code,
        minutes,
        body,
    } = mint(request)?;
    let pair_url = body.get("pair_url").and_then(|u| u.as_str());
    let secret = body.get("secret").and_then(|s| s.as_str());

    println!();
    if args.qr {
        match pair_url {
            Some(url) => match terminal_qr(url) {
                Some(qr) => {
                    print!("{qr}");
                    println!("  Scan this from anywhere, or enter the code below.");
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
    if let Some(secret) = secret {
        println!("  Pairing secret: {secret}");
    }
    println!();
    let link = pair_url
        .map(str::to_owned)
        .or_else(|| secret.map(|s| format!("{base}/{SIGIL}/#{PAIR_PARAM}={s}")));
    match &link {
        Some(link) => println!("  Open {link} on the device you want to pair."),
        None => println!("  Open {base}/ on the device you want to pair, and enter the code."),
    }
    println!("  Or type the code on its pairing screen, from another device on your");
    println!("  local network or tailnet.");
    if link.is_some() {
        println!("  Through a tunnel or proxy, paste the secret or link into the");
        println!("  Pairing secret or link field.");
    }
    if let Some(label) = args.label {
        println!("  Unless that device names itself, it will be listed as \"{label}\".");
    }
    println!("  It works once and expires in {minutes} minutes.");
    if waits {
        print!("{}", awaiting_owner_note("device"));
    }
    println!();
    Ok(())
}

fn awaiting_owner_note(kind: &str) -> String {
    format!(
        "\n  A device is already paired here, so the new {kind} waits until you tap\n  \
         Allow under Settings -> Access on a paired device. If no paired device\n  \
         works any more, run `lucidos devices revoke --all` and pair again.\n"
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Grants {
    pub manage: bool,
    pub control: bool,
}

pub fn cmd_pair_gateway(
    port: Option<u16>,
    label: Option<&str>,
    grants: Grants,
) -> Result<(), BoxError> {
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "A gateway pairing secret only works on the gateway that minted it, so pass --port to \
         name the one the other gateway will reach",
    )?;
    let token = local_token()?;
    let probe = Probe::new(&client, &base)?;
    let address = gateway_pairing_origin(
        crate::external_address::stored(&client, &base, &token),
        || gateway_https_origin(&probe),
        || tailnet_origin(&probe),
    );

    let waits = crate::devices::any_paired(&client, &base, &token) == Some(true);
    let mut request = client
        .post(format!("{base}{GATEWAY_PAIRING_CODE_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token);
    if let Some(label) = label {
        request = request.query(&[("label", label)]);
    }
    if grants.manage {
        request = request.query(&[("manage", "true")]);
    }
    if grants.control {
        request = request.query(&[("control", "true")]);
    }
    let Minted { code, minutes, .. } = mint(request)?;

    println!();
    println!("  Gateway pairing secret: {code}");
    if grants.manage {
        println!("  Devices on the other gateway may manage this gateway's users and devices.");
    }
    if grants.control {
        println!(
            "  Devices on the other gateway may create, start, stop and restore this \
             gateway's workspaces."
        );
    }
    println!();
    println!("  On the other gateway, open Settings -> Access -> Add a remote gateway,");
    match &address {
        Some(address) => {
            println!("  and paste this address and secret:");
            println!();
            println!("    {address}");
        }
        None => {
            println!("  and paste this secret with an https address for this gateway.");
            println!();
            println!("  This gateway has no https address another gateway can reach yet.");
            println!("  Gateways pair over https only. Set up `tailscale serve`, or set one");
            println!("  with `lucidos external-address set`.");
        }
    }
    println!();
    println!("  It works once and expires in {minutes} minutes.");
    if waits {
        print!("{}", awaiting_owner_note("gateway"));
    }
    println!();
    Ok(())
}

const GATEWAY_PAIRING_CODE_PATH: &str = "/~/api/v1/auth/gateway-pairing-code";

fn local_token() -> Result<String, BoxError> {
    lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         mint a pairing code"
            .into()
    })
}

struct Minted {
    code: String,
    minutes: u64,
    body: serde_json::Value,
}

fn mint(request: reqwest::blocking::RequestBuilder) -> Result<Minted, BoxError> {
    let response = request.send()?;
    if !response.status().is_success() {
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
        .ok_or_else(|| -> BoxError { "the gateway returned no code".into() })?
        .to_string();
    let minutes = body
        .get("expires_in_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(300)
        / 60;
    Ok(Minted {
        code,
        minutes,
        body,
    })
}

fn pairing_origin(
    external: Option<String>,
    gateway_https: impl FnOnce() -> Option<String>,
    tailnet: impl FnOnce() -> Option<String>,
) -> Option<String> {
    external.or_else(gateway_https).or_else(tailnet)
}

fn gateway_pairing_origin(
    external: Option<String>,
    gateway_https: impl FnOnce() -> Option<String>,
    tailnet: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let https = |origin: &String| origin.starts_with("https://");
    pairing_origin(external.filter(https), gateway_https, || {
        tailnet().filter(https)
    })
}

struct Probe {
    ours: Option<serde_json::Value>,
    remote: reqwest::blocking::Client,
    off_loopback_base: String,
}

impl Probe {
    fn new(loopback: &reqwest::blocking::Client, base: &str) -> Result<Self, BoxError> {
        let remote = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let ours = health(loopback, base);
        let off_loopback_base = off_loopback_base(base, ours.as_ref());
        Ok(Self {
            ours,
            remote,
            off_loopback_base,
        })
    }
}

fn off_loopback_base(base: &str, health: Option<&serde_json::Value>) -> String {
    let operator = health
        .and_then(|h| h.pointer("/tls/source"))
        .and_then(serde_json::Value::as_str)
        == Some("operator");
    let scheme = if operator { "https" } else { "http" };
    let authority = base.split_once("://").map_or(base, |(_, a)| a);
    format!("{scheme}://{authority}")
}

fn gateway_https_origin(probe: &Probe) -> Option<String> {
    let ours = probe.ours.as_ref()?;
    let origin = ours.pointer("/tls/https_origin")?.as_str()?;
    let id = instance_id(ours)?;
    reaches_this_gateway(&probe.remote, origin, &id).then(|| origin.to_string())
}

fn tailnet_origin(probe: &Probe) -> Option<String> {
    let state = lucidos_tailscale::tailnet_state();
    let Some(node) = state.node() else {
        if let Some(reason) = state.reason() {
            eprintln!("  No tailnet address: {reason}");
        }
        return None;
    };
    let ours = our_instance(probe, "No tailnet address")?;
    tailnet_candidates(&probe.off_loopback_base, node)
        .into_iter()
        .find(|origin| reaches_this_gateway(&probe.remote, origin, &ours))
}

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

fn reachable_origin(probe: &Probe, host: &str) -> Option<String> {
    let ours = our_instance(probe, "Using the gateway's own port at --host")?;
    qr_origin_candidates(&probe.off_loopback_base, host)
        .into_iter()
        .find(|origin| reaches_this_gateway(&probe.remote, origin, &ours))
}

fn qr_origin_candidates(base: &str, host: &str) -> Vec<String> {
    let ported = origin_at_host(base, host);
    let served = format!("https://{host}");
    if ported == served {
        vec![ported]
    } else {
        vec![ported, served]
    }
}

fn reaches_this_gateway(client: &reqwest::blocking::Client, origin: &str, ours: &str) -> bool {
    health(client, origin)
        .as_ref()
        .and_then(instance_id)
        .as_deref()
        == Some(ours)
}

fn our_instance(probe: &Probe, outcome: &str) -> Option<String> {
    let ours = probe.ours.as_ref().and_then(instance_id);
    if ours.is_none() {
        eprintln!(
            "  {outcome}: the gateway is older than this CLI and cannot prove which \
             gateway answers there. Restart Lucidos to update it."
        );
    }
    ours
}

fn instance_id(health: &serde_json::Value) -> Option<String> {
    health
        .get("instance_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

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

fn origin_at_host(base: &str, host: &str) -> String {
    let (scheme, authority) = base.split_once("://").unwrap_or(("https", base));
    match authority.rsplit_once(':') {
        Some((_, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            format!("{scheme}://{host}:{port}")
        }
        _ => format!("{scheme}://{host}"),
    }
}

fn terminal_qr(data: &str) -> Option<String> {
    let plain = std::env::var_os("NO_COLOR").is_some();
    Some(colorize(&qr_block(data)?, plain))
}

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
        let probe = probe_at("http://127.0.0.1:1");
        assert!(!reaches_this_gateway(&probe.remote, &elsewhere, "ours"));
    }

    fn probe_at(base: &str) -> Probe {
        Probe::new(&gateway::client().unwrap(), base).unwrap()
    }

    #[test]
    fn off_loopback_is_https_only_behind_an_operator_certificate() {
        let operator = serde_json::json!({"tls": {"source": "operator"}});
        let tailnet = serde_json::json!({"tls": {"source": "tailnet"}});
        let base = "https://127.0.0.1:5252";
        assert_eq!(
            off_loopback_base(base, Some(&operator)),
            "https://127.0.0.1:5252"
        );
        assert_eq!(
            off_loopback_base(base, Some(&tailnet)),
            "http://127.0.0.1:5252"
        );
        assert_eq!(off_loopback_base(base, None), "http://127.0.0.1:5252");
    }

    const UNASKED: fn() -> Option<String> = || panic!("a later rung must not be probed");

    #[test]
    fn the_external_address_leads_and_spares_every_probe() {
        let external = Some("https://lucidos.example.com".to_string());
        assert_eq!(pairing_origin(external.clone(), UNASKED, UNASKED), external);
    }

    #[test]
    fn the_gateways_own_https_comes_next_and_spares_the_tailnet_probe() {
        let served = || Some("https://mymac.tailnet-name.ts.net:5252".to_string());
        assert_eq!(
            pairing_origin(None, served, UNASKED).as_deref(),
            Some("https://mymac.tailnet-name.ts.net:5252")
        );
        assert_eq!(
            gateway_pairing_origin(Some("http://10.8.0.2:5252".to_string()), served, UNASKED)
                .as_deref(),
            Some("https://mymac.tailnet-name.ts.net:5252")
        );
    }

    #[test]
    fn without_either_the_tailnet_answers() {
        let tailnet = || Some("http://mymac.tailnet-name.ts.net:5252".to_string());
        assert_eq!(
            pairing_origin(None, || None, tailnet).as_deref(),
            Some("http://mymac.tailnet-name.ts.net:5252")
        );
        assert_eq!(pairing_origin(None, || None, || None), None);
    }

    #[test]
    fn an_http_external_address_never_hides_a_tailnet_https_one() {
        let tailnet = || Some("https://mymac.tailnet-name.ts.net".to_string());
        assert_eq!(
            gateway_pairing_origin(Some("http://10.8.0.2:5252".to_string()), || None, tailnet)
                .as_deref(),
            Some("https://mymac.tailnet-name.ts.net")
        );
        let plain = || Some("http://mymac.tailnet-name.ts.net:5252".to_string());
        assert_eq!(gateway_pairing_origin(None, || None, plain), None);
        let external = Some("https://lucidos.example.com".to_string());
        assert_eq!(
            gateway_pairing_origin(external.clone(), UNASKED, UNASKED),
            external
        );
    }

    fn health_answer(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn the_served_origin_counts_once_it_answers_as_this_gateway() {
        let served = answer_once(health_answer(r#"{"instance_id":"ours"}"#));
        let base = answer_once(health_answer(&format!(
            r#"{{"instance_id":"ours","tls":{{"https_origin":"{served}"}}}}"#
        )));
        let probe = probe_at(&base);
        assert_eq!(gateway_https_origin(&probe), Some(served));
    }

    #[test]
    fn a_served_origin_that_another_gateway_answers_is_skipped() {
        let served = answer_once(health_answer(r#"{"instance_id":"theirs"}"#));
        let base = answer_once(health_answer(&format!(
            r#"{{"instance_id":"ours","tls":{{"https_origin":"{served}"}}}}"#
        )));
        let probe = probe_at(&base);
        assert_eq!(gateway_https_origin(&probe), None);
    }

    #[test]
    fn a_gateway_with_no_served_origin_has_no_rung() {
        let base = answer_once(health_answer(
            r#"{"instance_id":"ours","tls":{"source":"none","https_origin":null}}"#,
        ));
        let probe = probe_at(&base);
        assert_eq!(gateway_https_origin(&probe), None);
    }

    #[test]
    fn swapping_the_host_keeps_the_scheme_and_the_port() {
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
            addresses: vec![std::net::Ipv4Addr::new(100, 70, 0, 1).into()],
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
        assert!(qr.contains("\u{1b}[30;47m") || std::env::var_os("NO_COLOR").is_some());

        let plain = colorize("ab\ncd", true);
        assert_eq!(plain, "ab\ncd\n");
        assert!(!plain.contains('\u{1b}'));
    }

    #[test]
    fn a_pairing_qr_fits_an_ordinary_terminal() {
        let block = qr_block("https://mac.tail1234.ts.net/~/?pair=01234567").unwrap();
        let widest = block.lines().map(|l| l.chars().count()).max().unwrap();
        assert!(
            widest <= 80,
            "a QR {widest} columns wide wraps on most terminals"
        );
    }

    #[test]
    fn every_line_is_wrapped_and_reset() {
        let painted = colorize("ab\ncd", false);
        assert_eq!(
            painted,
            "\u{1b}[30;47mab\u{1b}[0m\n\u{1b}[30;47mcd\u{1b}[0m\n"
        );
    }

    #[test]
    fn the_waiting_note_names_the_redeemer_and_the_way_back_in() {
        let note = awaiting_owner_note("gateway");
        assert!(note.contains("the new gateway waits"), "{note}");
        assert!(note.contains("lucidos devices revoke --all"), "{note}");
        assert!(note.lines().all(|l| l.chars().count() <= 80), "{note}");
    }
}
