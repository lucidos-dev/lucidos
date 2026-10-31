//! When a certificate stops being valid: its `notAfter`, read straight from the
//! DER. The walk reads only the fields in front of `validity`, which RFC 5280
//! fixes in order, so it needs no X.509 crate.

use chrono::{DateTime, NaiveDateTime, Utc};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;

const SEQUENCE: u8 = 0x30;
const VERSION: u8 = 0xa0;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;

/// The `notAfter` of the first certificate in `pem`, the leaf.
pub fn not_after(pem: &[u8]) -> Option<DateTime<Utc>> {
    let leaf = CertificateDer::pem_slice_iter(pem).next()?.ok()?;
    leaf_not_after(&leaf)
}

fn leaf_not_after(der: &[u8]) -> Option<DateTime<Utc>> {
    let (certificate, _) = expect(der, SEQUENCE)?;
    let (tbs, _) = expect(certificate, SEQUENCE)?;
    let mut rest = tbs;
    if rest.first() == Some(&VERSION) {
        rest = tlv(rest)?.2;
    }
    // serialNumber, signature, issuer.
    for _ in 0..3 {
        rest = tlv(rest)?.2;
    }
    let (validity, _) = expect(rest, SEQUENCE)?;
    let (_, _, after_not_before) = tlv(validity)?;
    let (tag, time, _) = tlv(after_not_before)?;
    let time = std::str::from_utf8(time).ok()?;
    let full = match tag {
        // Two-digit years: 50 to 99 are the 1900s (RFC 5280 §4.1.2.5.1).
        UTC_TIME if time.get(..2)? >= "50" => format!("19{time}"),
        UTC_TIME => format!("20{time}"),
        GENERALIZED_TIME => time.to_string(),
        _ => return None,
    };
    NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%SZ")
        .ok()
        .map(|t| t.and_utc())
}

fn expect(der: &[u8], want: u8) -> Option<(&[u8], &[u8])> {
    let (tag, content, rest) = tlv(der)?;
    (tag == want).then_some((content, rest))
}

/// One tag-length-value: the tag, the content, and what follows it.
fn tlv(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, der) = der.split_first()?;
    let (&first, der) = der.split_first()?;
    let (len, der) = if first < 0x80 {
        (usize::from(first), der)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || der.len() < count {
            return None;
        }
        let (bytes, der) = der.split_at(count);
        let len = bytes
            .iter()
            .fold(0usize, |len, &b| (len << 8) | usize::from(b));
        (len, der)
    };
    (der.len() >= len).then(|| (tag, &der[..len], &der[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn minted_until(year: i32, month: u8, day: u8) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec!["n.example".into()]).unwrap();
        params.not_after = rcgen::date_time_ymd(year, month, day);
        let key = rcgen::KeyPair::generate().unwrap();
        params.self_signed(&key).unwrap().pem().into_bytes()
    }

    #[test]
    fn a_utc_time_expiry_is_read() {
        assert_eq!(
            not_after(&minted_until(2031, 5, 6)),
            Some(Utc.with_ymd_and_hms(2031, 5, 6, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn a_generalized_time_expiry_is_read() {
        // RFC 5280 switches to GeneralizedTime from 2050 on.
        assert_eq!(
            not_after(&minted_until(2051, 1, 2)),
            Some(Utc.with_ymd_and_hms(2051, 1, 2, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn anything_but_a_certificate_has_no_expiry() {
        assert_eq!(not_after(b"junk"), None);
        assert_eq!(leaf_not_after(&[0x30, 0x82, 0xff]), None);
    }
}
