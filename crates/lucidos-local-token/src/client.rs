//! The one client setup for a Lucidos loopback listener. Every in-tree client
//! of a loopback port builds through here, async or blocking. So the TLS and
//! proxy settings have one definition (decision 12 of
//! `docs/plans/2026-11-19-desktop-window-https.md`).
//!
//! - `no_proxy`: an `HTTPS_PROXY` in the environment, which a corporate
//!   machine exports globally, must never carry a loopback hop.
//! - rustls, trusting only the loopback CA's public copy. Another process
//!   squatting the port cannot present a leaf it signed (ADR 0745).
//! - No redirect is followed. A plaintext loopback request gets a `301`, and
//!   following it would turn a POST into a GET.
//!
//! A missing CA copy trusts nothing, so every https request fails on the
//! certificate. Callers add their own timeout and headers.

/// The origin of a Lucidos loopback listener on `port`. Every one serves TLS
/// from the loopback CA, so there is no scheme to choose.
pub fn origin(port: impl std::fmt::Display) -> String {
    format!("https://127.0.0.1:{port}")
}

/// An async client builder for a loopback listener.
pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .no_proxy()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .redirect(reqwest::redirect::Policy::none());
    match loopback_root() {
        Some(root) => builder.add_root_certificate(root),
        None => builder,
    }
}

/// A blocking client builder for a loopback listener.
#[cfg(feature = "blocking")]
pub fn blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    let builder = reqwest::blocking::Client::builder()
        .no_proxy()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .redirect(reqwest::redirect::Policy::none());
    match loopback_root() {
        Some(root) => builder.add_root_certificate(root),
        None => builder,
    }
}

#[cfg(not(feature = "test-support"))]
fn loopback_root() -> Option<reqwest::Certificate> {
    let pem = crate::loopback_cert::read().ok()?;
    reqwest::Certificate::from_pem(pem.as_bytes()).ok()
}

#[cfg(feature = "test-support")]
fn loopback_root() -> Option<reqwest::Certificate> {
    reqwest::Certificate::from_pem(crate::test_support::ca_pem().as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    /// `scripts/lib/loopback_curl.sh` cannot import this crate, so it carries
    /// a copy of the origin.
    #[test]
    fn the_shell_twin_names_the_same_origin() {
        const LOOPBACK_CURL_SH: &str = include_str!("../../../scripts/lib/loopback_curl.sh");
        let origin = super::origin("%s");
        let line = format!("printf '{origin}\\n' \"$1\"");
        assert!(LOOPBACK_CURL_SH.contains(&line), "loopback_curl.sh: {line}");
    }
}
