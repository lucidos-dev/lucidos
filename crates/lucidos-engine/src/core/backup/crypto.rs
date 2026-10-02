use std::io::{Read, Write};
use std::path::Path;

use aes_gcm::aead::generic_array::typenum::U12;
use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Magic header identifying the chunked encryption format. Written by `encrypt`.
const MAGIC: &[u8; 8] = b"LUCIDOS1";

/// Pre-rebrand magic. Still recognized on read so backups produced before
/// the cognos→lucidos rename remain decryptable.
const LEGACY_MAGIC: &[u8; 8] = b"COGNOS01";

/// Chunk size for streaming encryption (1 MB).
const CHUNK_SIZE: usize = 1_048_576;

/// Generate a random 32-byte AES-256 key.
pub fn generate_key() -> Vec<u8> {
    Aes256Gcm::generate_key(OsRng).to_vec()
}

/// Encode a key as base64 for display/storage.
pub fn key_to_base64(key: &[u8]) -> String {
    BASE64.encode(key)
}

/// Decode a base64-encoded key, validating it is exactly 32 bytes.
pub fn key_from_base64(s: &str) -> Result<Vec<u8>, BoxError> {
    let bytes = BASE64.decode(s.trim())?;
    if bytes.len() != 32 {
        return Err(format!("invalid key length: expected 32 bytes, got {}", bytes.len()).into());
    }
    Ok(bytes)
}

/// Encrypt data using AES-256-GCM with chunked streaming.
///
/// Format: MAGIC(8) || nonce_prefix(8) || [chunk_len(4 LE) || ciphertext+tag]* || end_marker(4 zero bytes)
///
/// Each chunk uses a unique nonce: nonce_prefix(8) || chunk_index(4 LE).
/// This avoids loading the entire input into memory.
pub fn encrypt(key: &[u8], mut input: impl Read, output: &mut impl Write) -> Result<(), BoxError> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    // Use the first 8 bytes of a generated nonce as our prefix
    let full_nonce = Aes256Gcm::generate_nonce(OsRng);
    let nonce_prefix: [u8; 8] = full_nonce[..8].try_into().unwrap();

    output.write_all(MAGIC)?;
    output.write_all(&nonce_prefix)?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut chunk_index: u32 = 0;

    loop {
        let bytes_read = read_full(&mut input, &mut buf)?;
        if bytes_read == 0 {
            // End marker
            output.write_all(&0u32.to_le_bytes())?;
            break;
        }

        let nonce = make_nonce(&nonce_prefix, chunk_index);
        let ciphertext = cipher
            .encrypt(&nonce, &buf[..bytes_read])
            .map_err(|e| format!("encryption failed at chunk {chunk_index}: {e}"))?;

        output.write_all(&(bytes_read as u32).to_le_bytes())?;
        output.write_all(&ciphertext)?;

        chunk_index = chunk_index
            .checked_add(1)
            .ok_or("too many chunks: nonce counter overflow")?;
    }

    Ok(())
}

/// Decrypt data produced by `encrypt`.
pub fn decrypt(key: &[u8], mut input: impl Read, output: &mut impl Write) -> Result<(), BoxError> {
    let mut header = [0u8; 8];
    input.read_exact(&mut header)?;

    if &header != MAGIC && &header != LEGACY_MAGIC {
        return Err(format!(
            "invalid magic header: expected {:?}, got {:?}",
            MAGIC, header
        )
        .into());
    }
    decrypt_chunked(key, &mut input, output)
}

/// Chunked decryption: reads chunk-by-chunk, never holding more than one chunk in memory.
fn decrypt_chunked(
    key: &[u8],
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_prefix = [0u8; 8];
    input.read_exact(&mut nonce_prefix)?;

    let mut chunk_index: u32 = 0;

    loop {
        let mut len_bytes = [0u8; 4];
        input.read_exact(&mut len_bytes)?;
        let chunk_len = u32::from_le_bytes(len_bytes) as usize;

        if chunk_len == 0 {
            break;
        }

        // `encrypt` never writes a chunk larger than CHUNK_SIZE, so a bigger
        // length means a corrupt or hostile archive. The length is four bytes
        // read straight out of a downloaded file. Unchecked, the allocation
        // below zeroes up to 4 GiB before the short read can fail, which
        // OOM-kills the engine instead of failing the restore.
        if chunk_len > CHUNK_SIZE {
            return Err(format!(
                "corrupt archive: chunk {chunk_index} declares {chunk_len} bytes, \
                 above the {CHUNK_SIZE}-byte maximum"
            )
            .into());
        }

        // Ciphertext is chunk_len + 16 bytes (GCM tag)
        let ct_len = chunk_len + 16;
        let mut ciphertext = vec![0u8; ct_len];
        input.read_exact(&mut ciphertext)?;

        let nonce = make_nonce(&nonce_prefix, chunk_index);
        let plaintext = cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|e| format!("decryption failed at chunk {chunk_index}: {e}"))?;

        output.write_all(&plaintext)?;

        chunk_index = chunk_index.checked_add(1).ok_or("too many chunks")?;
    }

    Ok(())
}

/// Build a 12-byte nonce from an 8-byte prefix and a 4-byte chunk index.
fn make_nonce(prefix: &[u8; 8], index: u32) -> aes_gcm::Nonce<U12> {
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(prefix);
    nonce_bytes[8..].copy_from_slice(&index.to_le_bytes());
    nonce_bytes.into()
}

/// Read exactly `buf.len()` bytes, or fewer if EOF is reached.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> Result<usize, std::io::Error> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

/// Load a base64-encoded key from a file. Returns None if the file doesn't exist.
pub fn load_key_file(path: &Path) -> Result<Option<Vec<u8>>, BoxError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(key_from_base64(&contents)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write a key to `path` as base64, only if no file is there yet. Returns
/// `false` when another writer got there first, leaving its key untouched.
///
/// The key goes to a temp file beside `path`, then links into place without
/// clobbering, so a reader never sees a half-written key. This one file
/// decrypts every backup the workspace uploads. `NamedTempFile` creates it
/// `0600` on the open, so it is never readable by others, not even briefly.
fn create_key_file_exclusive(path: &Path, key: &[u8]) -> Result<bool, BoxError> {
    let dir = path
        .parent()
        .ok_or("backup key path has no parent directory")?;
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(key_to_base64(key).as_bytes())?;
    temp.as_file().sync_all()?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        // The directory took the temp file, so the link itself is what failed.
        // exFAT and FAT on macOS reject the no-clobber rename with `ENOTSUP`.
        Err(_) => create_key_file_in_place(path, key),
    }
}

/// Create the key file with `O_EXCL`, for filesystems that cannot link
/// without clobbering. It still never overwrites a key. A concurrent reader
/// can see the file before the write lands, and then fails on a short key.
fn create_key_file_in_place(path: &Path, key: &[u8]) -> Result<bool, BoxError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let written = file
        .write_all(key_to_base64(key).as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        // An empty key file would fail every later backup until removed.
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(true)
}

/// Load the backup key from the workspace, auto-generating one if it doesn't exist.
///
/// No caller ever returns a key that is not on disk, and at most one reports
/// it as new. A caller that kept a key it failed to persist would encrypt
/// backups nobody can restore. Concurrent callers on a keyless workspace all
/// get the one key, except where `create_key_file_in_place` lets one fail.
pub fn ensure_key(workspace: &std::path::Path) -> Result<(Vec<u8>, bool), BoxError> {
    let key_path = super::key_file_path(workspace);
    if let Some(key) = load_key_file(&key_path)? {
        return Ok((key, false));
    }
    let key = generate_key();
    if create_key_file_exclusive(&key_path, &key)? {
        return Ok((key, true));
    }
    match load_key_file(&key_path)? {
        Some(winner) => Ok((winner, false)),
        None => Err(format!(
            "backup key path {} exists but holds no readable key",
            key_path.display()
        )
        .into()),
    }
}

/// Whether a usable backup key is already persisted for this workspace.
///
/// Pure read: unlike [`ensure_key`], this NEVER generates or writes a key. It
/// lets the Settings → Backup page choose its button label ("Show backup key"
/// when a key exists vs "Generate new backup key" when none does) without the
/// mere act of checking minting one — the exact footgun that surfaced a "New
/// backup key generated" toast for a workspace that already had backups. A
/// present-but-unreadable/corrupt file is treated as "no usable key" so the
/// caller routes to generate rather than to a reveal that would error.
pub fn key_exists(workspace: &std::path::Path) -> bool {
    load_key_file(&super::key_file_path(workspace))
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_generate_key_is_32_bytes() {
        let key = generate_key();
        assert_eq!(key.len(), 32);

        // Two generated keys should differ
        let key2 = generate_key();
        assert_ne!(key, key2);
    }

    #[test]
    fn test_key_to_base64_roundtrip() {
        let key = generate_key();
        let encoded = key_to_base64(&key);
        let decoded = key_from_base64(&encoded).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn test_key_from_base64_invalid() {
        // Bad base64
        assert!(key_from_base64("not-valid-base64!!!").is_err());

        // Valid base64 but wrong length (16 bytes instead of 32)
        let short_key = BASE64.encode(vec![0u8; 16]);
        let err = key_from_base64(&short_key).unwrap_err();
        assert!(err.to_string().contains("expected 32 bytes"));

        // Valid base64 but too long (64 bytes)
        let long_key = BASE64.encode(vec![0u8; 64]);
        let err = key_from_base64(&long_key).unwrap_err();
        assert!(err.to_string().contains("expected 32 bytes"));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = generate_key();
        let plaintext = b"Hello, Lucidos backup encryption!";

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(plaintext), &mut encrypted).unwrap();

        // Chunked format: MAGIC(8) + nonce_prefix(8) + chunk_len(4) + ciphertext+tag(32+16) + end_marker(4)
        assert_eq!(encrypted.len(), 8 + 8 + 4 + plaintext.len() + 16 + 4);

        let mut decrypted = Vec::new();
        decrypt(&key, Cursor::new(&encrypted), &mut decrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_accepts_legacy_cognos_magic() {
        // Pre-rebrand backups carry b"COGNOS01" in the first 8 bytes.
        // Patch a freshly-encrypted blob's header in place and verify it
        // still decrypts.
        let key = generate_key();
        let plaintext = b"pre-rebrand backup";

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(plaintext), &mut encrypted).unwrap();
        encrypted[..8].copy_from_slice(LEGACY_MAGIC);

        let mut decrypted = Vec::new();
        decrypt(&key, Cursor::new(&encrypted), &mut decrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_large_multi_chunk() {
        let key = generate_key();
        // 2.5 MB — spans 3 chunks (1MB + 1MB + 0.5MB)
        let plaintext: Vec<u8> = (0..2_621_440).map(|i| (i % 251) as u8).collect();

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(&plaintext), &mut encrypted).unwrap();

        let mut decrypted = Vec::new();
        decrypt(&key, Cursor::new(&encrypted), &mut decrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_exact_chunk_boundary() {
        let key = generate_key();
        // Exactly 1MB — one full chunk
        let plaintext = vec![42u8; CHUNK_SIZE];

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(&plaintext), &mut encrypted).unwrap();

        let mut decrypted = Vec::new();
        decrypt(&key, Cursor::new(&encrypted), &mut decrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_empty() {
        let key = generate_key();

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(b""), &mut encrypted).unwrap();

        // Just header + end marker
        assert_eq!(encrypted.len(), 8 + 8 + 4);

        let mut decrypted = Vec::new();
        decrypt(&key, Cursor::new(&encrypted), &mut decrypted).unwrap();

        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = generate_key();
        let key2 = generate_key();
        let plaintext = b"secret data";

        let mut encrypted = Vec::new();
        encrypt(&key1, Cursor::new(plaintext), &mut encrypted).unwrap();

        let mut decrypted = Vec::new();
        let result = decrypt(&key2, Cursor::new(&encrypted), &mut decrypted);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("decryption failed"));
    }

    #[test]
    fn test_decrypt_corrupted_data_fails() {
        let key = generate_key();
        let plaintext = b"important data";

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(plaintext), &mut encrypted).unwrap();

        // Corrupt a byte in the ciphertext (after header + nonce_prefix + chunk_len = 20 bytes)
        encrypted[22] ^= 0xFF;

        let mut decrypted = Vec::new();
        let result = decrypt(&key, Cursor::new(&encrypted), &mut decrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_data_too_short() {
        let key = generate_key();
        let mut out = Vec::new();
        let result = decrypt(&key, Cursor::new(&[0u8; 5]), &mut out);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_key_file_absent_and_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.key");
        assert!(load_key_file(&path).unwrap().is_none());

        let key = generate_key();
        std::fs::write(&path, key_to_base64(&key)).unwrap();
        assert_eq!(load_key_file(&path).unwrap().unwrap(), key);
    }

    /// The key is CREATED owner-only, never created and then narrowed.
    ///
    /// No runtime assertion can see the difference: both shapes end at `0600`.
    /// What differs is the mode the file exists at in between, which under the
    /// usual `022` umask is `0644`. So this test pins both writers at the
    /// source: each sets `0600` on the open, and neither writes or chmods.
    #[test]
    fn the_key_file_is_created_owner_only_rather_than_chmodded_afterwards() {
        let source = include_str!("crypto.rs");
        let body_of = |name: &str| {
            source
                .split_once(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("{name} must be declared here"))
                .1
                .split_once("\n}\n")
                .unwrap_or_else(|| panic!("{name} must have a body"))
                .0
        };
        let writers = [
            ("create_key_file_exclusive", "NamedTempFile::new_in("),
            ("create_key_file_in_place", "mode(&mut options, 0o600)"),
        ];
        for (name, owner_only_open) in writers {
            let body = body_of(name);
            assert!(
                body.contains(owner_only_open),
                "{name} must open the key owner-only: {body}"
            );
            for banned in ["fs::write(", "set_permissions("] {
                assert!(
                    !body.contains(banned),
                    "{name}: {banned} creates the key at the umask's mode first"
                );
            }
        }
    }

    /// The fallback for filesystems without a no-clobber link still lets
    /// exactly one concurrent writer create the key, owner-only.
    #[test]
    fn the_in_place_fallback_lets_exactly_one_writer_create_the_key() {
        const WRITERS: usize = 16;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.key");
        let barrier = std::sync::Barrier::new(WRITERS);
        let results: Vec<(Vec<u8>, bool)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..WRITERS)
                .map(|_| {
                    scope.spawn(|| {
                        let key = generate_key();
                        barrier.wait();
                        let created = create_key_file_in_place(&path, &key).unwrap();
                        (key, created)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let winners: Vec<_> = results.iter().filter(|(_, created)| *created).collect();
        assert_eq!(winners.len(), 1, "exactly one writer may create the key");
        assert_eq!(load_key_file(&path).unwrap().unwrap(), winners[0].0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    /// The scheduled backup (and the manual path) rely on `ensure_key` to never
    /// skip when no key exists: it must generate + persist a fresh 32-byte key
    /// at the workspace's `.lucidos/backup.key`, then return the same key
    /// idempotently on subsequent calls. This is the exact shared helper the
    /// scheduled cron now uses instead of skipping the backup.
    #[test]
    fn test_ensure_key_generates_when_absent_and_is_idempotent() {
        let workspace = tempfile::tempdir().unwrap();
        let key_path = crate::core::backup::key_file_path(workspace.path());

        // No pre-existing key — the scheduled-backup precondition.
        assert!(load_key_file(&key_path).unwrap().is_none());

        // First call generates + persists: is_new is true, never a skip.
        let (key, is_new) = ensure_key(workspace.path()).unwrap();
        assert!(
            is_new,
            "first ensure_key must report a freshly generated key"
        );
        assert_eq!(key.len(), 32, "generated key must be a 32-byte AES-256 key");

        // The key landed at the same path the manual path writes to, in the
        // same base64-on-disk format `load_key_file` round-trips.
        assert!(key_path.exists());
        assert_eq!(load_key_file(&key_path).unwrap().unwrap(), key);

        // Second call is idempotent: same key, no regeneration.
        let (key2, is_new2) = ensure_key(workspace.path()).unwrap();
        assert!(!is_new2, "second ensure_key must reuse the existing key");
        assert_eq!(key, key2);
    }

    /// `POST /backup/key` and a backup run can both reach `ensure_key` on a
    /// workspace with no key. Each caller must get the key that stays on disk.
    /// A caller holding a losing key would upload a backup nothing can decrypt.
    #[test]
    fn concurrent_ensure_key_calls_all_return_the_persisted_key() {
        const CALLERS: usize = 16;
        for _ in 0..50 {
            let workspace = tempfile::tempdir().unwrap();
            let barrier = std::sync::Barrier::new(CALLERS);
            let results: Vec<(Vec<u8>, bool)> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..CALLERS)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            ensure_key(workspace.path()).unwrap()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            let persisted = load_key_file(&crate::core::backup::key_file_path(workspace.path()))
                .unwrap()
                .expect("a key must be on disk");
            for (key, _) in &results {
                assert_eq!(key, &persisted, "a caller got a key that is not on disk");
            }
            let minted = results.iter().filter(|(_, is_new)| *is_new).count();
            assert_eq!(minted, 1, "exactly one caller may report a new key");
        }
    }

    /// A key path that exists but reads as missing, such as a dangling
    /// symlink, refuses the link and yields no key. `ensure_key` must fail
    /// rather than retry forever on a request thread.
    #[cfg(unix)]
    #[test]
    fn ensure_key_fails_on_a_key_path_that_exists_but_cannot_be_read() {
        let workspace = tempfile::tempdir().unwrap();
        let key_path = crate::core::backup::key_file_path(workspace.path());
        std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(workspace.path().join("missing"), &key_path).unwrap();

        let err = ensure_key(workspace.path()).unwrap_err();
        assert!(err.to_string().contains("holds no readable key"), "{err}");
    }

    /// The key `ensure_key` mints is owner-only, and no temp file stays behind.
    #[cfg(unix)]
    #[test]
    fn a_minted_key_is_owner_only_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let key_path = crate::core::backup::key_file_path(workspace.path());
        ensure_key(workspace.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600,
            "a minted key file must be owner-only"
        );
        let entries: Vec<_> = std::fs::read_dir(key_path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![key_path.file_name().unwrap().to_owned()]);
    }

    /// `key_exists` is the read-only counterpart to `ensure_key`: it reports
    /// whether a key is on disk WITHOUT ever creating one. This is what lets the
    /// Settings → Backup button label itself ("Show backup key" vs "Generate
    /// new backup key") without the mere act of checking minting a key — the
    /// exact behavior behind a "New backup key generated" toast appearing for a
    /// workspace that already had backups.
    #[test]
    fn test_key_exists_is_read_only_and_reflects_presence() {
        let workspace = tempfile::tempdir().unwrap();
        let key_path = crate::core::backup::key_file_path(workspace.path());

        // Absent: reports false AND must not create the file as a side effect.
        assert!(!key_exists(workspace.path()));
        assert!(!key_path.exists(), "checking existence must not mint a key");
        // Repeated checks stay read-only — no key materializes.
        assert!(!key_exists(workspace.path()));
        assert!(!key_path.exists());

        // After a real generation, it reports true.
        let (key, is_new) = ensure_key(workspace.path()).unwrap();
        assert!(is_new);
        assert!(key_exists(workspace.path()));

        // Checking existence on a present key never alters the stored bytes.
        assert!(key_exists(workspace.path()));
        assert_eq!(load_key_file(&key_path).unwrap().unwrap(), key);
    }

    /// Revealing the key (the read path the GET endpoint uses) must never
    /// overwrite or regenerate it: reading an existing key returns the same
    /// bytes every time, and reading an ABSENT key returns `None` without
    /// creating a file. Together with the `ensure_key` idempotency test this
    /// pins the core guarantee the user asked about — "Show backup key" cannot
    /// replace the key that protects existing backups.
    #[test]
    fn test_reveal_is_non_destructive() {
        let workspace = tempfile::tempdir().unwrap();
        let key_path = crate::core::backup::key_file_path(workspace.path());

        // Reading an absent key never mints one.
        assert!(load_key_file(&key_path).unwrap().is_none());
        assert!(!key_path.exists());

        // Generate once, then read repeatedly — the bytes are stable.
        let (key, _) = ensure_key(workspace.path()).unwrap();
        for _ in 0..3 {
            assert_eq!(load_key_file(&key_path).unwrap().unwrap(), key);
        }
    }
}
