//! The link a new device opens to pair, and its QR.
//!
//! A pairing code is eight digits somebody reads off one screen and types into
//! another. A QR carries the whole pairing URL instead, so the phone's camera
//! does the typing.
//!
//! # The origin is the hard part, and it comes from the caller
//!
//! A QR pointing at `127.0.0.1` is useless to a phone, and loopback is exactly
//! what the machine minting the code is usually reading. Only the client knows
//! which of its addresses another device can reach. So the client sends one,
//! and this module refuses anything that is not a bare `http(s)` origin.
//!
//! Nothing here authorizes. [`crate::auth`] decides who may mint, and the
//! caller-supplied origin never reaches it.

use qrcode::render::svg;
use qrcode::QrCode;

/// The longest origin we will encode.
///
/// A MagicDNS name with a port is far under this. The cap stops a caller
/// growing the payload until the encoder refuses it. It also bounds the string
/// we echo back in `pair_url`.
const MAX_ORIGIN_LEN: usize = 255;

/// The URL parameter carrying the code to the pairing screen.
///
/// Its counterpart is `PAIR_CODE_PARAM` in
/// `crates/lucidos-app/src/utils/pairingCodeSeed.ts`, which reads it and then
/// strips it from the address bar.
pub const PAIR_PARAM: &str = "pair";

/// The pairing code a query string carries, or `None`.
///
/// The reader for what [`pair_url`] writes. Two callers: the manifest, which
/// stamps the code into `start_url` so a freshly installed home-screen app
/// pairs itself, and the shell, which points its manifest link here.
///
/// Nothing is percent-decoded. A code is eight digits, and those encode to
/// themselves, so a value needing decoding is not a value we wrote.
pub fn pairing_code_in_query(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == PAIR_PARAM)
        .and_then(|(_, value)| crate::auth::valid_pairing_code(value))
}

/// Accept `raw` only if it is a bare `http(s)` origin, and hand it back
/// trimmed.
///
/// Bare means scheme, host and an optional port, with no credentials, path,
/// query or fragment. The value is echoed back inside `pair_url`, which the
/// page renders as a link. A permissive parse would therefore be a way to put
/// an arbitrary URL on somebody's screen.
pub fn valid_origin(raw: &str) -> Option<&str> {
    let origin = raw.trim();
    if origin.is_empty() || origin.len() > MAX_ORIGIN_LEN {
        return None;
    }
    // Lowercase only. Every client builds this from a `location.protocol` or a
    // literal, and both are already lowercase. Accepting `HTTPS://` would only
    // widen what has to be reasoned about.
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let (host, port) = split_authority(authority)?;
    if !host_is_addressable(host) {
        return None;
    }
    match port {
        Some(p) if !valid_port(p) => None,
        _ => Some(origin),
    }
}

/// The URL that pairs a device, for an origin [`valid_origin`] accepted.
///
/// `code` is our own eight digits, so it needs no escaping. `/~/` is the
/// reserved sigil namespace the picker is served from, and the picker is the
/// one surface an unpaired device may reach.
pub fn pair_url(origin: &str, code: &str) -> String {
    format!("{origin}/~/?{PAIR_PARAM}={code}")
}

/// `data` as an SVG QR code, or `None` when it does not fit in a QR at all.
///
/// Black on white whatever the app's theme is: a scanner wants dark modules on
/// a light field, and an inverted code is a coin flip. One module per unit, so
/// the payload stays small and the page scales it as a vector.
///
/// The output carries no caller text. This crate's SVG renderer emits one
/// `rect` and one `path`, and the only strings in it are the two colours here.
pub fn qr_svg(data: &str) -> Option<String> {
    let code = QrCode::new(data).ok()?;
    Some(
        code.render()
            .module_dimensions(1, 1)
            .quiet_zone(true)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

/// The host part of an authority, and whether it arrived in brackets.
enum Host<'a> {
    /// A hostname or an IPv4 literal.
    Name(&'a str),
    /// The inside of `[...]`, which has to be an IPv6 literal.
    V6(&'a str),
}

/// Split `host[:port]`, tolerating a bracketed IPv6 literal.
fn split_authority(authority: &str) -> Option<(Host<'_>, Option<&str>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        return Some((Host::V6(host), port));
    }
    Some(match authority.split_once(':') {
        Some((host, port)) => (Host::Name(host), Some(port)),
        None => (Host::Name(authority), None),
    })
}

/// Can a browser actually address this host?
///
/// Not merely a character allow-list. A permissive one accepts `-`, `a..b` and
/// `[foo]`, and none of those resolve. The gateway would then mint a QR nobody
/// can scan, instead of the 400 this promises.
fn host_is_addressable(host: Host<'_>) -> bool {
    match host {
        // Parsed rather than pattern-matched: `std` already knows the grammar,
        // including the IPv4-mapped forms.
        Host::V6(literal) => literal.parse::<std::net::Ipv6Addr>().is_ok(),
        Host::Name(name) => {
            !name.is_empty() && name.len() <= 253 && name.split('.').all(valid_label)
        }
    }
}

/// One dot-separated label of a hostname, per RFC 1123.
///
/// Empty rejects a leading, trailing or doubled dot. The hyphen rule rejects a
/// bare `-`. Both are names a caller can type and no browser can resolve.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A port the URL may carry: decimal, non-zero, and inside `u16`.
fn valid_port(port: &str) -> bool {
    port.parse::<u16>().is_ok_and(|p| p > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_origin_is_accepted_and_trimmed() {
        assert_eq!(
            valid_origin("https://mac.tail1234.ts.net"),
            Some("https://mac.tail1234.ts.net")
        );
        assert_eq!(
            valid_origin("  http://192.168.1.5:5252  "),
            Some("http://192.168.1.5:5252")
        );
        assert_eq!(
            valid_origin("https://localhost:5251"),
            Some("https://localhost:5251")
        );
        assert_eq!(
            valid_origin("http://[fd7a::1]:5252"),
            Some("http://[fd7a::1]:5252")
        );
    }

    #[test]
    fn anything_that_is_not_a_bare_origin_is_refused() {
        // Each of these would put something other than an origin into the
        // `pair_url` the page renders as a link.
        for raw in [
            "",
            "   ",
            "ftp://mac.ts.net",
            "javascript:alert(1)",
            "//mac.ts.net",
            "mac.ts.net",
            "https://mac.ts.net/steal",
            "https://mac.ts.net?x=1",
            "https://mac.ts.net#frag",
            "https://user:pw@mac.ts.net",
            "https://mac.ts.net evil",
            "https://mac.ts.net\nSet-Cookie: x=1",
            "https://",
            "HTTPS://mac.ts.net",
        ] {
            assert_eq!(valid_origin(raw), None, "accepted {raw:?}");
        }
    }

    #[test]
    fn a_host_no_browser_can_resolve_is_refused_too() {
        // Not an injection, a dud. Each of these passes a character
        // allow-list and resolves nowhere. The QR would scan to a dead link
        // instead of the caller getting a 400.
        for raw in [
            "https://-",
            "https://a..b",
            "https://.mac.ts.net",
            "https://mac.ts.net.",
            "https://-mac.ts.net",
            "https://mac-.ts.net",
            "https://[foo]",
            "https://[]",
            "http://[192.168.1.5]",
        ] {
            assert_eq!(valid_origin(raw), None, "accepted {raw:?}");
        }
    }

    #[test]
    fn a_port_has_to_be_a_real_port() {
        assert!(valid_origin("https://mac.ts.net:0").is_none());
        assert!(valid_origin("https://mac.ts.net:70000").is_none());
        assert!(valid_origin("https://mac.ts.net:abc").is_none());
        assert!(valid_origin("https://mac.ts.net:").is_none());
        assert!(valid_origin("https://mac.ts.net:5252").is_some());
    }

    #[test]
    fn an_over_long_origin_is_refused_before_the_encoder_sees_it() {
        let long = format!("https://{}.ts.net", "a".repeat(MAX_ORIGIN_LEN));
        assert_eq!(valid_origin(&long), None);
    }

    #[test]
    fn the_pair_url_carries_the_code_into_the_picker() {
        assert_eq!(
            pair_url("https://mac.ts.net:5252", "01234567"),
            "https://mac.ts.net:5252/~/?pair=01234567"
        );
    }

    #[test]
    fn the_query_reader_finds_the_code_the_pair_url_wrote() {
        let url = pair_url("https://mac.ts.net", "01234567");
        let query = url.split_once('?').map(|(_, q)| q);
        assert_eq!(pairing_code_in_query(query), Some("01234567"));

        // Among siblings, and in either order.
        assert_eq!(
            pairing_code_in_query(Some("x=1&pair=01234567&y=2")),
            Some("01234567")
        );
    }

    #[test]
    fn the_query_reader_answers_none_for_anything_else() {
        for query in [
            None,
            Some(""),
            Some("pair="),
            Some("pair=abc"),
            Some("pair=0123456"),
            // A name that merely ENDS WITH ours must not satisfy the lookup.
            Some("xpair=01234567"),
            Some("other=01234567"),
            // Percent-encoded digits are not digits. We never write these.
            Some("pair=%30%31%32%33%34%35%36%37"),
        ] {
            assert_eq!(pairing_code_in_query(query), None, "accepted {query:?}");
        }
    }

    #[test]
    fn the_svg_renders_and_leaks_neither_the_origin_nor_the_code() {
        // The whole reason a caller-supplied string is safe to encode: it
        // becomes squares, never text.
        let url = pair_url("https://mac.tail1234.ts.net", "01234567");
        let svg = qr_svg(&url).expect("a pairing URL fits in a QR");
        assert!(svg.starts_with("<?xml"), "{svg}");
        assert!(!svg.contains("mac.tail1234.ts.net"));
        assert!(!svg.contains("01234567"));
        assert!(svg.contains("#000000") && svg.contains("#ffffff"));
    }

    #[test]
    fn data_too_long_for_any_qr_yields_none_rather_than_panicking() {
        // Version 40 tops out around 2953 bytes. `valid_origin` keeps a real
        // caller far away from this, so it is the belt to that braces.
        assert!(qr_svg(&"a".repeat(8000)).is_none());
    }
}
