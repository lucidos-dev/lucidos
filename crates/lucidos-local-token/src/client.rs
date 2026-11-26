pub fn origin(port: impl std::fmt::Display) -> String {
    format!("https://127.0.0.1:{port}")
}

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
    #[test]
    fn the_shell_twin_names_the_same_origin() {
        const LOOPBACK_CURL_SH: &str = include_str!("../../../scripts/lib/loopback_curl.sh");
        let origin = super::origin("%s");
        let line = format!("printf '{origin}\\n' \"$1\"");
        assert!(LOOPBACK_CURL_SH.contains(&line), "loopback_curl.sh: {line}");
    }
}
