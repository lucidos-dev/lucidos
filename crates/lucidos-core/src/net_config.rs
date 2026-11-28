pub const API_V1_PREFIX: &str = "/api/v1";

// INVARIANT: ---------------------------------------------------------------------------
// Wire scheme (http vs https) — the SINGLE source of truth.
// ---------------------------------------------------------------------------
//
// A Lucidos process serves TLS on its own port iff BOTH `LUCIDOS_TLS_CERT` and
// `LUCIDOS_TLS_KEY` point at non-empty paths. Every caller that needs that
// scheme MUST resolve it through [`tls_scheme`] / [`tls_scheme_from`] (or the
// constants below), never by re-deriving from `LUCIDOS_TLS_*` inline. A call to
// the gateway has no scheme to choose: every gateway serves its loopback CA on
// loopback (`lucidos_local_token::client::origin`).

pub const SCHEME_HTTP: &str = "http";
pub const SCHEME_HTTPS: &str = "https";

pub fn tls_scheme_from(cert: Option<&str>, key: Option<&str>) -> &'static str {
    match (cert, key) {
        (Some(c), Some(k)) if !c.trim().is_empty() && !k.trim().is_empty() => SCHEME_HTTPS,
        _ => SCHEME_HTTP,
    }
}

pub fn tls_scheme() -> &'static str {
    tls_scheme_from(
        std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
        std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
    )
}

/// INVARIANT: Install the one rustls crypto provider this crate uses.
///
/// **Nothing that opens a TLS connection works before this runs.** Several
/// crates in the tree enable rustls' `aws-lc-rs` feature and several enable
/// `ring`, so rustls refuses to pick one and panics at the first handshake.
/// `reqwest` never hits it, because its own feature set builds a config per
/// client. A bare `connect_async` does.
///
/// Idempotent. An `Err` means somebody already installed one, which is the
/// outcome this wants: a second caller must not panic just for asking.
///
/// Call it from every entry point that reaches the network, including a test
/// that does. The engine binary calls it first thing in `run`.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    mod guard_tests {
        /// The engine reads no gateway file: `network.toml` belongs to the gateway.
        #[test]
        fn the_engine_reads_no_network_toml() {
            let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("crates dir");
            let this_test = "lucidos-core/src/net_config.rs";
            let mut readers = Vec::new();
            let mut stack = vec![crates.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
                    let path = entry.path();
                    let rel = path
                        .strip_prefix(crates)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    if rel.starts_with("lucidos-gateway")
                        || rel.contains("node_modules")
                        || rel.contains("/target")
                    {
                        continue;
                    }
                    if path.is_dir() {
                        stack.push(path);
                    } else if rel.ends_with(".rs")
                        && rel != this_test
                        && std::fs::read_to_string(&path).is_ok_and(|text| {
                            text.contains("read_network_toml") || text.contains("network_toml_path")
                        })
                    {
                        readers.push(rel);
                    }
                }
            }
            assert!(
                readers.is_empty(),
                "engine code reads network.toml: {readers:?}"
            );
        }
    }

    use super::*;

    #[test]
    fn tls_scheme_from_requires_both_nonempty() {
        assert_eq!(
            tls_scheme_from(Some("/c.pem"), Some("/k.pem")),
            SCHEME_HTTPS
        );
        assert_eq!(tls_scheme_from(None, Some("/k.pem")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("/c.pem"), None), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(None, None), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some(""), Some("/k.pem")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("/c.pem"), Some("  ")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some(""), Some("")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("   "), Some("   ")), SCHEME_HTTP);
    }
}
