//! The update address: the `latest.json` URL an update check reads instead of
//! the published release (plan `docs/plans/2026-10-21-update-address-setting.md`).
//!
//! One rule, applied wherever an address is written or used: `https`, or
//! `http` to the loopback names `localhost`, `127.0.0.1` and `[::1]` only.
//! Credentials in the URL are refused, since the value is shown in Settings
//! and written to logs.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

/// The gateway route whose `release_check` object reports the address in use
/// (`update_address`) and why a stored one was refused (`update_address_error`).
pub const GATEWAY_STATUS_PATH: &str = "/~/api/v1/control/gateway/status";

/// The gateway route that writes the address: `PUT {"update_address": "…"}`,
/// where `""` resets it to the default.
pub const RELEASE_CHECK_PATH: &str = "/~/api/v1/control/release-check";

/// A `latest.json` URL that passed [`UpdateAddress::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAddress(Url);

/// The rule, as the error message names it.
pub const RULE: &str =
    "the update address must be a latest.json URL over https, or http to localhost, 127.0.0.1 or [::1]";

impl UpdateAddress {
    /// Reads a stored or typed value. Empty (after trimming) means the
    /// default address and parses to `None`.
    pub fn parse(raw: &str) -> Result<Option<Self>, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        let refuse = |why: &str| Err(format!("{RULE} ({why}): {raw}"));
        let url = match Url::parse(raw) {
            Ok(url) => url,
            Err(e) => return refuse(&format!("not an absolute URL, {e}")),
        };
        match transport_fault(&url) {
            None => Ok(Some(Self(url))),
            Some(why) => refuse(&why),
        }
    }

    pub fn url(&self) -> &Url {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for UpdateAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The rule for the bundle URL a `latest.json` names. An update address may
/// point at any `latest.json`, so its bundle URL needs the same transport rule.
pub const BUNDLE_RULE: &str =
    "an update bundle must download over https, or http to localhost, 127.0.0.1 or [::1]";

/// Checks the bundle URL an update offer names against the address's
/// transport rule.
pub fn check_bundle_url(url: &Url) -> Result<(), String> {
    match transport_fault(url) {
        None => Ok(()),
        Some(why) => Err(format!("{BUNDLE_RULE} ({why}): {url}")),
    }
}

/// Why `url` breaks the transport rule, or `None` when it keeps it.
fn transport_fault(url: &Url) -> Option<String> {
    if !url.username().is_empty() || url.password().is_some() {
        return Some("it carries credentials".to_string());
    }
    let Some(host) = url.host() else {
        return Some("it has no host".to_string());
    };
    match url.scheme() {
        "https" => None,
        "http" if is_loopback_name(&host) => None,
        "http" => Some("plain http goes only to a loopback address".to_string()),
        other => Some(format!("scheme {other} is not allowed")),
    }
}

fn is_loopback_name(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => *name == "localhost",
        Host::Ipv4(ip) => *ip == Ipv4Addr::LOCALHOST,
        Host::Ipv6(ip) => *ip == Ipv6Addr::LOCALHOST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(raw: &str) -> String {
        UpdateAddress::parse(raw)
            .unwrap_or_else(|e| panic!("{raw} refused: {e}"))
            .unwrap_or_else(|| panic!("{raw} read as unset"))
            .to_string()
    }

    fn refused(raw: &str) -> String {
        match UpdateAddress::parse(raw) {
            Err(e) => e,
            Ok(v) => panic!("{raw} accepted as {v:?}"),
        }
    }

    #[test]
    fn empty_and_blank_mean_the_default() {
        assert_eq!(UpdateAddress::parse("").unwrap(), None);
        assert_eq!(UpdateAddress::parse("  \n").unwrap(), None);
    }

    #[test]
    fn https_anywhere_is_accepted() {
        assert_eq!(
            accepted("https://example.com/releases/latest.json"),
            "https://example.com/releases/latest.json"
        );
        assert_eq!(
            accepted("  https://example.com:8443/x/latest.json?v=1  "),
            "https://example.com:8443/x/latest.json?v=1"
        );
    }

    #[test]
    fn http_to_each_loopback_name_is_accepted() {
        for raw in [
            "http://localhost:5173/api/v1/data/artifacts/releases/rc/latest.json",
            "http://LOCALHOST/latest.json",
            "http://127.0.0.1:5173/latest.json",
            "http://[::1]:5173/latest.json",
        ] {
            accepted(raw);
        }
    }

    #[test]
    fn http_to_any_other_host_is_refused() {
        for raw in [
            "http://example.com/latest.json",
            "http://localhost.evil.com/latest.json",
            "http://evil.com.localhost/latest.json",
            "http://localhost./latest.json",
            "http://127.0.0.2/latest.json",
            "http://192.168.1.10/latest.json",
            "http://[::2]/latest.json",
            "http://0.0.0.0/latest.json",
        ] {
            assert!(refused(raw).contains("loopback"), "{raw}");
        }
    }

    #[test]
    fn credentials_are_refused() {
        for raw in [
            "https://user@example.com/latest.json",
            "https://user:pw@example.com/latest.json",
            "http://user@localhost/latest.json",
            "http://localhost@evil.com/latest.json",
        ] {
            assert!(refused(raw).contains("credentials"), "{raw}");
        }
    }

    #[test]
    fn other_schemes_are_refused() {
        for raw in [
            "file:///tmp/latest.json",
            "ftp://example.com/latest.json",
            "javascript:alert(1)",
            "data:application/json,{}",
        ] {
            refused(raw);
        }
    }

    #[test]
    fn bare_and_relative_paths_are_refused() {
        for raw in [
            "/tmp/releases/rc/latest.json",
            "releases/rc/latest.json",
            "./latest.json",
            "latest.json",
            "localhost:5173/latest.json",
        ] {
            refused(raw);
        }
    }

    #[test]
    fn the_error_names_the_rule() {
        assert!(refused("http://example.com/latest.json").starts_with(RULE));
    }

    #[test]
    fn a_bundle_url_keeps_the_same_transport_rule() {
        let check = |raw: &str| check_bundle_url(&Url::parse(raw).unwrap());
        check("https://github.com/o/r/releases/download/v1/Lucidos.app.tar.gz").unwrap();
        check("http://127.0.0.1:5173/api/v1/data/artifacts/releases/rc/Lucidos.app.tar.gz")
            .unwrap();
        let refused = check("http://cdn.example.com/Lucidos.app.tar.gz").unwrap_err();
        assert!(refused.starts_with(BUNDLE_RULE), "{refused}");
        assert!(refused.contains("loopback"), "{refused}");
        check("https://user@example.com/Lucidos.app.tar.gz").unwrap_err();
        check("file:///tmp/Lucidos.app.tar.gz").unwrap_err();
    }
}
