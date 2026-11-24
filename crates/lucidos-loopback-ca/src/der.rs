//! Just enough DER to read the subject public key out of a certificate. RFC
//! 5280 fixes the order of the fields in front of it, so no X.509 crate is
//! needed.

const SEQUENCE: u8 = 0x30;
const VERSION: u8 = 0xa0;

/// The whole `subjectPublicKeyInfo` element of the certificate `der`.
pub fn subject_public_key_info(der: &[u8]) -> Option<&[u8]> {
    let (certificate, _) = expect(der, SEQUENCE)?;
    let (tbs, _) = expect(certificate, SEQUENCE)?;
    let mut rest = tbs;
    if rest.first() == Some(&VERSION) {
        rest = tlv(rest)?.2;
    }
    // serialNumber, signature, issuer, validity, subject.
    for _ in 0..5 {
        rest = tlv(rest)?.2;
    }
    let (tag, content, after) = tlv(rest)?;
    (tag == SEQUENCE)
        .then(|| &rest[..rest.len() - after.len()])
        .filter(|_| !content.is_empty())
}

fn expect(der: &[u8], want: u8) -> Option<(&[u8], &[u8])> {
    let (tag, content, rest) = tlv(der)?;
    (tag == want).then_some((content, rest))
}

/// One tag-length-value element: its tag, its content, and what follows it.
fn tlv(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let width = usize::from(first & 0x7f);
        if width == 0 || width > std::mem::size_of::<usize>() || rest.len() < width {
            return None;
        }
        let len = rest[..width]
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, &rest[width..])
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}
