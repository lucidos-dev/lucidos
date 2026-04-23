use std::io::{Read, Write};
use std::path::Path;

use aes_gcm::aead::generic_array::typenum::U12;
use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Magic header identifying the chunked encryption format.
const MAGIC: &[u8; 8] = b"COGNOS01";

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
///
/// Supports both the chunked format (COGNOS01 header) and the legacy
/// single-nonce format for backwards compatibility.
pub fn decrypt(key: &[u8], mut input: impl Read, output: &mut impl Write) -> Result<(), BoxError> {
    let mut header = [0u8; 8];
    input.read_exact(&mut header)?;

    if &header == MAGIC {
        decrypt_chunked(key, &mut input, output)
    } else {
        decrypt_legacy(key, &header, &mut input, output)
    }
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

/// Legacy decryption for the old single-nonce format: nonce(12) || ciphertext+tag.
/// The first 8 bytes have already been read as `first_bytes`.
fn decrypt_legacy(
    key: &[u8],
    first_bytes: &[u8; 8],
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    // Read remaining 4 bytes of the 12-byte nonce
    let mut nonce_rest = [0u8; 4];
    input.read_exact(&mut nonce_rest)?;

    let mut ciphertext = Vec::new();
    input.read_to_end(&mut ciphertext)?;

    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(first_bytes);
    nonce_bytes[8..].copy_from_slice(&nonce_rest);
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| format!("decryption failed: {e}"))?;

    output.write_all(&plaintext)?;
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

/// Save a key to a file as base64.
pub fn save_key_file(path: &Path, key: &[u8]) -> Result<(), BoxError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, key_to_base64(key))?;
    Ok(())
}

/// Load the backup key from the workspace, auto-generating one if it doesn't exist.
pub fn ensure_key(workspace: &std::path::Path) -> Result<(Vec<u8>, bool), BoxError> {
    let key_path = super::key_file_path(workspace);
    match load_key_file(&key_path)? {
        Some(key) => Ok((key, false)),
        None => {
            let key = generate_key();
            save_key_file(&key_path, &key)?;
            Ok((key, true))
        }
    }
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
        let plaintext = b"Hello, CognOS backup encryption!";

        let mut encrypted = Vec::new();
        encrypt(&key, Cursor::new(plaintext), &mut encrypted).unwrap();

        // Chunked format: MAGIC(8) + nonce_prefix(8) + chunk_len(4) + ciphertext+tag(32+16) + end_marker(4)
        assert_eq!(encrypted.len(), 8 + 8 + 4 + plaintext.len() + 16 + 4);

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
    fn test_decrypt_legacy_format() {
        // Simulate the old format: nonce(12) || ciphertext+tag
        let key_bytes = generate_key();
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        let nonce = Aes256Gcm::generate_nonce(OsRng);

        let plaintext = b"legacy backup data";
        let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref()).unwrap();

        let mut legacy_data = Vec::new();
        legacy_data.extend_from_slice(&nonce);
        legacy_data.extend_from_slice(&ciphertext);

        // New decrypt should handle legacy format
        let mut decrypted = Vec::new();
        decrypt(&key_bytes, Cursor::new(&legacy_data), &mut decrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_load_save_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.key");

        // File doesn't exist yet
        assert!(load_key_file(&path).unwrap().is_none());

        // Save and load
        let key = generate_key();
        save_key_file(&path, &key).unwrap();

        let loaded = load_key_file(&path).unwrap().unwrap();
        assert_eq!(key, loaded);
    }
}
