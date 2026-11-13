use super::*;
use std::io::Cursor;

fn legacy_key() -> SealingKey {
    SealingKey::Legacy(SecretKey::generate())
}

fn opening_for(key: &SealingKey) -> OpeningKey {
    match key {
        SealingKey::Legacy(k) => OpeningKey::Legacy(SecretKey::from_bytes(k.as_bytes()).unwrap()),
        SealingKey::Recipient(_) => panic!("a recipient opens nothing"),
    }
}

fn seal_bytes(key: &SealingKey, plaintext: &[u8]) -> Vec<u8> {
    let mut sealed = Vec::new();
    seal(key, Cursor::new(plaintext), &mut sealed).unwrap();
    sealed
}

fn open_bytes(key: &OpeningKey, sealed: &[u8]) -> Result<Vec<u8>, BoxError> {
    let mut out = Vec::new();
    open(key, Cursor::new(sealed), &mut out)?;
    Ok(out)
}

/// An age pair, sealed to the recipient and opened with the identity.
fn age_pair() -> (SealingKey, OpeningKey) {
    let (identity, recipient) = generate_key_pair();
    (SealingKey::Recipient(recipient), OpeningKey::Age(identity))
}

/// Where the age payload starts: after the header's `--- <mac>` line and the
/// 16-byte stream nonce. Each payload chunk is 64 KiB plus a 16-byte tag.
fn age_payload_start(sealed: &[u8]) -> usize {
    let mac_line = sealed
        .windows(5)
        .position(|w| w == b"\n--- ")
        .expect("an age header");
    let header_end = mac_line
        + 1
        + sealed[mac_line + 1..]
            .iter()
            .position(|b| *b == b'\n')
            .unwrap()
        + 1;
    header_end + 16
}
const AGE_CHUNK: usize = 64 * 1024 + 16;

// --- age ---

#[test]
fn an_age_archive_round_trips_through_the_backup_key_text() {
    let (identity, recipient) = generate_key_pair();
    let plaintext: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let sealed = seal_bytes(&SealingKey::Recipient(recipient), &plaintext);
    assert!(sealed.starts_with(b"age-encryption.org/v1\n"));

    // What the user pasted back, not the in-memory identity.
    let pasted: OpeningKey = backup_key_text(&identity).parse().unwrap();
    assert!(matches!(pasted, OpeningKey::Age(_)));
    assert_eq!(open_bytes(&pasted, &sealed).unwrap(), plaintext);
}

#[test]
fn an_age_archive_refuses_another_backup_key() {
    let (seal_key, _) = age_pair();
    let (_, other) = age_pair();
    let sealed = seal_bytes(&seal_key, b"secret");
    let err = open_bytes(&other, &sealed).unwrap_err().to_string();
    assert!(err.contains("does not open this archive"), "{err}");
}

#[test]
fn an_age_archive_cut_at_a_chunk_boundary_fails() {
    let (seal_key, open_key) = age_pair();
    let plaintext = vec![9u8; 3 * 64 * 1024];
    let sealed = seal_bytes(&seal_key, &plaintext);
    let cut = age_payload_start(&sealed) + AGE_CHUNK;
    assert!(cut < sealed.len());
    assert!(open_bytes(&open_key, &sealed[..cut]).is_err());
}

#[test]
fn an_age_archive_with_two_chunks_swapped_fails() {
    let (seal_key, open_key) = age_pair();
    let plaintext: Vec<u8> = (0..3 * 64 * 1024).map(|i| (i % 7) as u8).collect();
    let mut sealed = seal_bytes(&seal_key, &plaintext);
    let first = age_payload_start(&sealed);
    let (head, tail) = sealed[first..].split_at_mut(AGE_CHUNK);
    head.swap_with_slice(&mut tail[..AGE_CHUNK]);
    assert!(open_bytes(&open_key, &sealed).is_err());
}

#[test]
fn an_age_archive_with_a_flipped_header_byte_fails() {
    let (seal_key, open_key) = age_pair();
    let mut sealed = seal_bytes(&seal_key, b"header guarded");
    // Inside the recipient stanza, past the version line.
    sealed[30] ^= 0x01;
    assert!(open_bytes(&open_key, &sealed).is_err());
}

#[test]
fn a_backup_key_parses_its_own_recipient() {
    let (identity, recipient) = generate_key_pair();
    let parsed = parse_backup_key(&backup_key_text(&identity)).unwrap();
    assert_eq!(parsed.to_public().to_string(), recipient.to_string());
    assert_eq!(
        parse_pasted_recipient(&recipient.to_string())
            .unwrap()
            .to_string(),
        recipient.to_string()
    );
}

#[test]
fn a_bad_key_error_never_repeats_the_pasted_text() {
    let pasted = "AGE-SECRET-KEY-1NOTAREALKEYATALLBUTLONGENOUGHTOMATTER";
    let err = parse_backup_key(pasted).err().unwrap().to_string();
    assert!(!err.contains("NOTAREALKEY"), "{err}");
    let err = "plainly-not-a-key"
        .parse::<OpeningKey>()
        .err()
        .unwrap()
        .to_string();
    assert!(!err.contains("plainly"), "{err}");
}

/// The own-key step wants a public key. A backup key pasted there is told
/// apart, and no message repeats the paste or a raw bech32 reason.
#[test]
fn a_bad_recipient_error_names_the_public_key_and_never_repeats_the_paste() {
    let (identity, recipient) = generate_key_pair();
    let backup_key = backup_key_text(&identity);
    let err = parse_pasted_recipient(&backup_key)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("secret backup key"), "{err}");
    assert!(err.contains("age1"), "{err}");
    let lowercase = backup_key.to_ascii_lowercase();
    let err = parse_pasted_recipient(&lowercase)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("secret backup key"), "{err}");
    let key_file = format!("# created: now\n# public key: {recipient}\n{backup_key}\n");
    let err = parse_pasted_recipient(&key_file).err().unwrap().to_string();
    assert!(err.contains("secret backup key"), "{err}");

    let damaged = format!("{}x", recipient);
    for pasted in [
        backup_key.as_str(),
        lowercase.as_str(),
        key_file.as_str(),
        damaged.as_str(),
        "plainly-not-a-key",
        "",
    ] {
        let err = parse_pasted_recipient(pasted).err().unwrap().to_string();
        assert!(!err.contains("HRP"), "{err}");
        assert!(!err.contains("checksum"), "{err}");
        if !pasted.is_empty() {
            assert!(!err.contains(pasted), "{err}");
        }
    }
    let err = parse_pasted_recipient(&damaged).err().unwrap().to_string();
    assert!(err.contains("not a valid age public key"), "{err}");
}

#[test]
fn a_recipient_parses_with_surrounding_whitespace() {
    let (_, recipient) = generate_key_pair();
    let pasted = format!("  {recipient}\n");
    assert_eq!(
        parse_pasted_recipient(&pasted).unwrap().to_string(),
        recipient.to_string()
    );
}

#[test]
fn a_key_of_the_wrong_kind_names_the_right_one() {
    let (age_seal, age_open) = age_pair();
    let legacy = legacy_key();
    let legacy_open = opening_for(&legacy);

    let age_archive = seal_bytes(&age_seal, b"x");
    let err = open_bytes(&legacy_open, &age_archive)
        .unwrap_err()
        .to_string();
    assert!(err.contains("AGE-SECRET-KEY-1"), "{err}");

    let legacy_archive = seal_bytes(&legacy, b"x");
    let err = open_bytes(&age_open, &legacy_archive)
        .unwrap_err()
        .to_string();
    assert!(err.contains(".enc"), "{err}");
}

#[test]
fn each_key_kind_names_its_suffix() {
    let (age_seal, _) = age_pair();
    assert_eq!(age_seal.archive_suffix(), ".age");
    assert_eq!(legacy_key().archive_suffix(), ".enc");
}

// --- legacy ---

/// An archive the `LUCIDOS1` encoder wrote before age existed, with the
/// all-sevens test key. Restore must keep opening archives like it.
const LEGACY_FIXTURE: &str = "TFVDSURPUzFsekoputEYfCQAAABsc2JwDC3brTqQEveyGUVsHCp4Hz/ACPMdWcCpbGXzE8Itx/EuDcyFg7nOY21xOomUt3yOAAAAAA==";

#[test]
fn a_legacy_archive_from_before_age_still_opens() {
    use base64::Engine as _;
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(LEGACY_FIXTURE)
        .unwrap();
    let key: OpeningKey = SecretKey::from_bytes(&[7u8; 32])
        .unwrap()
        .to_base64()
        .parse()
        .unwrap();
    assert_eq!(
        open_bytes(&key, &sealed).unwrap(),
        b"archive made by the LUCIDOS1 encoder"
    );
}

#[test]
fn test_encrypt_decrypt_roundtrip() {
    let key = legacy_key();
    let plaintext = b"Hello, Lucidos backup encryption!";
    let encrypted = seal_bytes(&key, plaintext);

    // Chunked format: MAGIC(8) + nonce_prefix(8) + chunk_len(4) + ciphertext+tag(32+16) + end_marker(4)
    assert_eq!(encrypted.len(), 8 + 8 + 4 + plaintext.len() + 16 + 4);
    assert_eq!(
        open_bytes(&opening_for(&key), &encrypted).unwrap(),
        plaintext
    );
}

#[test]
fn test_decrypt_accepts_legacy_cognos_magic() {
    // Pre-rebrand backups carry b"COGNOS01" in the first 8 bytes.
    let key = legacy_key();
    let mut encrypted = seal_bytes(&key, b"pre-rebrand backup");
    encrypted[..8].copy_from_slice(LEGACY_MAGIC);
    assert_eq!(
        open_bytes(&opening_for(&key), &encrypted).unwrap(),
        b"pre-rebrand backup"
    );
}

#[test]
fn test_encrypt_decrypt_large_multi_chunk() {
    let key = legacy_key();
    // 2.5 MB spans 3 chunks (1MB + 1MB + 0.5MB)
    let plaintext: Vec<u8> = (0..2_621_440).map(|i| (i % 251) as u8).collect();
    let encrypted = seal_bytes(&key, &plaintext);
    assert_eq!(
        open_bytes(&opening_for(&key), &encrypted).unwrap(),
        plaintext
    );
}

#[test]
fn test_encrypt_decrypt_exact_chunk_boundary() {
    let key = legacy_key();
    let plaintext = vec![42u8; CHUNK_SIZE];
    let encrypted = seal_bytes(&key, &plaintext);
    assert_eq!(
        open_bytes(&opening_for(&key), &encrypted).unwrap(),
        plaintext
    );
}

#[test]
fn test_encrypt_decrypt_empty() {
    let key = legacy_key();
    let encrypted = seal_bytes(&key, b"");
    // Just header + end marker
    assert_eq!(encrypted.len(), 8 + 8 + 4);
    assert!(open_bytes(&opening_for(&key), &encrypted)
        .unwrap()
        .is_empty());
}

#[test]
fn test_decrypt_wrong_key_fails() {
    let encrypted = seal_bytes(&legacy_key(), b"secret data");
    let err = open_bytes(&opening_for(&legacy_key()), &encrypted).unwrap_err();
    assert!(err.to_string().contains("decryption failed"));
}

#[test]
fn test_decrypt_corrupted_data_fails() {
    let key = legacy_key();
    let mut encrypted = seal_bytes(&key, b"important data");
    // Corrupt a byte in the ciphertext (after header + nonce_prefix + chunk_len = 20 bytes)
    encrypted[22] ^= 0xFF;
    assert!(open_bytes(&opening_for(&key), &encrypted).is_err());
}

#[test]
fn test_decrypt_data_too_short() {
    assert!(open_bytes(&opening_for(&legacy_key()), &[0u8; 5]).is_err());
}

#[test]
fn an_unknown_header_is_not_an_archive() {
    let err = open_bytes(&opening_for(&legacy_key()), b"PK\x03\x04 not a backup")
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a backup archive"), "{err}");
}
