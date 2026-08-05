//! Lowercase-hex byte encoding, shared by every module that renders bytes as
//! text: the proxy signers, their HMAC helpers, the body-hash snapshots, the
//! WASM-host tests, and the thread-bound origin token in `api::actor`.
//!
//! Centralised here so the encoding format stays consistent. Historically
//! each proxy module grew its own near-identical helper (`hex_lower`,
//! `hex_string`, `hex_to_string`) and a couple of inline `format!` loops in
//! test code. It was `api/proxy_hex.rs` while the proxy was its only consumer;
//! the origin token made that name untrue.

/// Encode `bytes` as lowercase ASCII hex. Output length is `2 * bytes.len()`.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Decode an even-length ASCII hex string back to bytes; `None` on odd length
/// or any non-hex character.
///
/// The inverse of [`hex_lower`], added so a MAC that travels as hex can be
/// compared as *bytes* through `hmac`'s constant-time `verify_slice` rather
/// than as a string through `==`.
pub(crate) fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
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

    #[test]
    fn decode_round_trips_every_byte_value() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(hex_decode(&hex_lower(&all)).as_deref(), Some(&all[..]));
    }

    #[test]
    fn decode_rejects_odd_length_and_non_hex() {
        assert_eq!(hex_decode("abc"), None, "odd length");
        assert_eq!(hex_decode("zz"), None, "non-hex characters");
        assert_eq!(hex_decode("0g"), None, "one non-hex nibble");
        // `to_digit(16)` accepts uppercase, and a decoder that rejected it
        // would be surprising. Encoding stays lowercase; decoding is lenient.
        assert_eq!(hex_decode("AB").as_deref(), Some(&[0xabu8][..]));
    }

    #[test]
    fn decode_of_empty_is_empty_not_none() {
        assert_eq!(hex_decode("").as_deref(), Some(&[][..]));
    }
}
