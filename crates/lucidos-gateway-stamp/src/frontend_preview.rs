//! Stable preview identity shared by the launching gateway and relay peers.

use sha2::{Digest as _, Sha256};

pub const ENV_PREVIEW_KEY: &str = "LUCIDOS_FRONTEND_PREVIEW_KEY";

/// A routing key, not a credential. Authorization belongs to the gateway.
pub fn key(gateway_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"lucidos-frontend-preview:");
    digest.update(gateway_id.as_bytes());
    digest
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_domain_separated_and_does_not_expose_identity() {
        assert_eq!(key("example-gateway"), "2aecb1b0a6619fe0");
        assert_ne!(key("another-gateway"), key("example-gateway"));
        assert!(!key("example-gateway").contains("example-gateway"));
        assert_eq!(key("☃").len(), 16);
    }
}
