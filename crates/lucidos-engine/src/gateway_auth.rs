//! Authenticating the engine's own calls to the gateway.
//!
//! The gateway's control plane requires a credential (`lucidos_gateway::auth`).
//! The engine is a local process, so it proves that by sending the machine-local
//! token, exactly as the `lucidos` CLI does.
//!
//! # Why a helper rather than a header per call site
//!
//! Five places call the gateway: the boot-phase report, the boot-failure
//! report, the workspace-label lookup, the Apply restart, and the unread-total
//! push check. A header added at each one leaves a sixth site free to be
//! written without it. That sixth site then fails as a silent 401, on a
//! best-effort path nobody watches.
//!
//! So every gateway-bound client is built through [`client_builder`], which
//! carries the credential as a default header.

use lucidos_local_token::{read as local_token, HEADER_LOCAL_TOKEN};
use reqwest::header::{HeaderMap, HeaderValue};

/// Default headers proving this call comes from a process on this machine.
///
/// Empty when no gateway has minted a token, which is the no-gateway launch.
/// An empty map is right there: the call has no gateway to reach anyway.
pub fn auth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(token) = local_token() {
        if let Ok(mut value) = HeaderValue::from_str(&token) {
            value.set_sensitive(true);
            headers.insert(HEADER_LOCAL_TOKEN, value);
        }
    }
    headers
}

/// A `reqwest` builder aimed at the gateway, carrying the credential.
///
/// `no_proxy` and `danger_accept_invalid_certs` are the loopback pair every
/// other intra-host client in the tree uses: each process ships a self-signed
/// dev cert, and an `HTTPS_PROXY` in the environment must never be consulted
/// for a `127.0.0.1` hop. Callers add their own timeout.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .default_headers(auth_headers())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_is_marked_sensitive_so_it_stays_out_of_debug_output() {
        // A `HeaderMap` lands in error and trace formatting on this path, and
        // a credential printed into a log is a credential leaked.
        let mut headers = HeaderMap::new();
        let mut value = HeaderValue::from_static("secret-token");
        value.set_sensitive(true);
        headers.insert(HEADER_LOCAL_TOKEN, value);
        assert!(!format!("{headers:?}").contains("secret-token"));
    }

    #[test]
    fn no_token_yields_no_header_rather_than_an_empty_one() {
        // An empty header value would be sent and rejected, which reads as a
        // wrong credential rather than as "this machine has no gateway".
        let headers = auth_headers();
        if let Some(value) = headers.get(HEADER_LOCAL_TOKEN) {
            assert!(!value.is_empty(), "an empty credential must not be sent");
        }
    }
}
