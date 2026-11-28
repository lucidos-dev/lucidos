use qrcode::render::svg;
use qrcode::QrCode;

const MAX_ORIGIN_LEN: usize = 255;

pub use lucidos_gateway_constants::PAIR_PARAM;

pub fn pairing_token_in_query(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == PAIR_PARAM)
        .and_then(|(_, value)| crate::pairing::valid_device_pairing_token(value))
}

pub fn valid_origin(raw: &str) -> Option<&str> {
    let origin = raw.trim();
    if origin.is_empty() || origin.len() > MAX_ORIGIN_LEN {
        return None;
    }
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

pub fn pair_url(origin: &str, secret: &str) -> String {
    format!("{origin}/~/#{PAIR_PARAM}={secret}")
}

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

enum Host<'a> {
    Name(&'a str),
    V6(&'a str),
}

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

fn host_is_addressable(host: Host<'_>) -> bool {
    match host {
        Host::V6(literal) => literal.parse::<std::net::Ipv6Addr>().is_ok(),
        Host::Name(name) => {
            !name.is_empty() && name.len() <= 253 && name.split('.').all(valid_label)
        }
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

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
            "https://mac.ts.net:5252/~/#pair=01234567"
        );
    }

    const SECRET: &str = "ldp_abcdefghijklmnopqrstuvwxyz";

    #[test]
    fn the_query_reader_finds_the_secret_the_pair_url_wrote() {
        let url = pair_url("https://mac.ts.net", SECRET);
        let query = url.split_once('#').map(|(_, q)| q);
        assert_eq!(pairing_token_in_query(query), Some(SECRET));
    }

    #[test]
    fn the_query_reader_still_takes_an_older_gateways_short_code() {
        let url = pair_url("https://mac.ts.net", "01234567");
        let query = url.split_once('#').map(|(_, q)| q);
        assert_eq!(pairing_token_in_query(query), Some("01234567"));

        assert_eq!(
            pairing_token_in_query(Some("x=1&pair=01234567&y=2")),
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
            Some("xpair=01234567"),
            Some("other=01234567"),
            Some("pair=%30%31%32%33%34%35%36%37"),
            Some("pair=lgp_abcdefghijklmnopqrstuvwxyz"),
            Some("pair=ldp_ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ] {
            assert_eq!(pairing_token_in_query(query), None, "accepted {query:?}");
        }
    }

    #[test]
    fn the_svg_renders_and_leaks_neither_the_origin_nor_the_code() {
        let url = pair_url("https://mac.tail1234.ts.net", SECRET);
        let svg = qr_svg(&url).expect("a pairing URL fits in a QR");
        assert!(svg.starts_with("<?xml"), "{svg}");
        assert!(!svg.contains("mac.tail1234.ts.net"));
        assert!(!svg.contains(SECRET));
        assert!(svg.contains("#000000") && svg.contains("#ffffff"));
    }

    #[test]
    fn data_too_long_for_any_qr_yields_none_rather_than_panicking() {
        assert!(qr_svg(&"a".repeat(8000)).is_none());
    }
}
