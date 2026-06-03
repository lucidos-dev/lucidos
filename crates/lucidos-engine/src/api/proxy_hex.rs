//! Lowercase-hex byte encoding shared across the proxy modules
//! (signers, HMAC helpers, body-hash snapshots, WASM-host tests).
//!
//! Centralised here so the encoding format stays consistent — historically
//! each proxy module grew its own near-identical helper (`hex_lower`,
//! `hex_string`, `hex_to_string`) and a couple of inline `format!` loops in
//! test code.

/// Encode `bytes` as lowercase ASCII hex. Output length is `2 * bytes.len()`.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_string() {
        assert_eq!(hex_lower(&[]), "");
    }

    #[test]
    fn encodes_known_bytes_lowercase() {
        assert_eq!(hex_lower(&[0x01, 0x23, 0xab, 0xcd]), "0123abcd");
    }

    #[test]
    fn pads_single_nibble_values_with_leading_zero() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xa0]), "000fa0");
    }
}
