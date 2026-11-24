//! How a backup archive is sealed and opened.
//!
//! New archives are age files sealed to the workspace's backup recipient: an
//! open format that the `age` and `rage` tools open with no Lucidos at all.
//! The age stream authenticates its header and its last chunk, so a cut,
//! reordered or edited archive fails to open.
//!
//! The older `LUCIDOS1` format (and its pre-rebrand `COGNOS01` spelling) is a
//! symmetric AES-256-GCM stream with neither guarantee. It stays readable for
//! every archive already made. A workspace that has not switched to a
//! recipient still writes it, until the switch.

use std::io::{Read, Write};
use std::str::FromStr;

use aes_gcm::aead::generic_array::typenum::U12;
use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit};
use age::secrecy::ExposeSecret;

use crate::core::secret_store::SecretKey;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Magic header of the legacy chunked format.
const MAGIC: &[u8; 8] = b"LUCIDOS1";

/// Pre-rebrand magic. Still recognized on read so backups produced before
/// the cognos→lucidos rename remain decryptable.
const LEGACY_MAGIC: &[u8; 8] = b"COGNOS01";

/// The first bytes of every binary age file (`age-encryption.org/v1`).
const AGE_MAGIC: &[u8; 8] = b"age-encr";

/// Chunk size of the legacy format (1 MiB).
const CHUNK_SIZE: usize = 1_048_576;

/// The prefix of an age backup key, as `age-keygen` prints it.
const AGE_KEY_PREFIX: &str = "AGE-SECRET-KEY-";

/// The suffix of an archive sealed to a backup recipient.
pub const AGE_SUFFIX: &str = ".age";
/// The suffix of an archive sealed with a legacy symmetric key.
pub const LEGACY_SUFFIX: &str = ".enc";

/// What seals a new archive.
pub enum SealingKey {
    /// The workspace's backup recipient. Only the user holds the matching key.
    Recipient(age::x25519::Recipient),
    /// The symmetric key of a workspace that has not switched yet.
    Legacy(SecretKey),
}

impl SealingKey {
    /// The filename suffix an archive sealed with this key carries.
    pub fn archive_suffix(&self) -> &'static str {
        match self {
            Self::Recipient(_) => AGE_SUFFIX,
            Self::Legacy(_) => LEGACY_SUFFIX,
        }
    }
}

/// What opens an archive: the key text the user pasted, of either kind.
pub enum OpeningKey {
    /// An age backup key (`AGE-SECRET-KEY-1…`).
    Age(age::x25519::Identity),
    /// A legacy base64 backup key.
    Legacy(SecretKey),
}

impl FromStr for OpeningKey {
    type Err = BoxError;

    fn from_str(text: &str) -> Result<Self, BoxError> {
        let text = text.trim();
        if text.to_ascii_uppercase().starts_with(AGE_KEY_PREFIX) {
            return Ok(Self::Age(parse_backup_key(text)?));
        }
        SecretKey::from_base64(text).map(Self::Legacy).map_err(|_| {
            "not a backup key: paste the key that starts with AGE-SECRET-KEY-1, \
             or the base64 key of an older .enc backup"
                .into()
        })
    }
}

/// Parse an age backup key. The error never repeats the pasted text.
pub fn parse_backup_key(text: &str) -> Result<age::x25519::Identity, BoxError> {
    age::x25519::Identity::from_str(text.trim())
        .map_err(|_| "not a valid backup key: it should start with AGE-SECRET-KEY-1".into())
}

/// Parse a backup recipient (`age1…`) the user pasted. The error never repeats
/// the pasted text. A paste holding a backup key anywhere, such as a whole key
/// file, gets its own message: the user holds the right key in the wrong form.
pub fn parse_pasted_recipient(text: &str) -> Result<age::x25519::Recipient, BoxError> {
    let text = text.trim();
    age::x25519::Recipient::from_str(text).map_err(|_| {
        if text.to_ascii_uppercase().contains(AGE_KEY_PREFIX) {
            "that is a secret backup key: paste its public key, which starts with age1".into()
        } else {
            "not a valid age public key: paste the whole key that starts with age1".into()
        }
    })
}

/// A fresh key pair. The caller shows the backup key once and keeps only the
/// recipient.
pub fn generate_key_pair() -> (age::x25519::Identity, age::x25519::Recipient) {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    (identity, recipient)
}

/// The backup key as the user copies it.
pub fn backup_key_text(identity: &age::x25519::Identity) -> String {
    identity.to_string().expose_secret().to_string()
}

/// Seal `input` into `output` with `key`.
pub fn seal(key: &SealingKey, input: impl Read, output: &mut impl Write) -> Result<(), BoxError> {
    match key {
        SealingKey::Recipient(recipient) => seal_to_recipient(recipient, input, output),
        SealingKey::Legacy(key) => encrypt_legacy(key.as_bytes(), input, output),
    }
}

/// Open an archive of either format. The format comes from the archive's own
/// header, so a key of the wrong kind fails with a message naming the right one.
pub fn open(
    key: &OpeningKey,
    mut input: impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
    let mut header = [0u8; 8];
    input
        .read_exact(&mut header)
        .map_err(|e| format!("not a backup archive: {e}"))?;
    match (&header, key) {
        (AGE_MAGIC, OpeningKey::Age(identity)) => {
            let rejoined = std::io::Cursor::new(header).chain(input);
            open_age(identity, rejoined, output)
        }
        (AGE_MAGIC, OpeningKey::Legacy(_)) => Err(
            "this is an age backup: open it with the backup key that starts with AGE-SECRET-KEY-1"
                .into(),
        ),
        (MAGIC | LEGACY_MAGIC, OpeningKey::Legacy(key)) => {
            decrypt_legacy_body(key.as_bytes(), &mut input, output)
        }
        (MAGIC | LEGACY_MAGIC, OpeningKey::Age(_)) => Err(
            "this is an older .enc backup: open it with the base64 backup key it was made with"
                .into(),
        ),
        _ => Err(
            "not a backup archive: the file starts with neither an age nor a LUCIDOS1 header"
                .into(),
        ),
    }
}

fn seal_to_recipient(
    recipient: &age::x25519::Recipient,
    mut input: impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))?;
    let mut writer = encryptor.wrap_output(output)?;
    std::io::copy(&mut input, &mut writer)?;
    writer.finish()?;
    Ok(())
}

fn open_age(
    identity: &age::x25519::Identity,
    input: impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
    let decryptor =
        age::Decryptor::new(input).map_err(|e| format!("not a valid age backup: {e}"))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| match e {
            age::DecryptError::NoMatchingKeys => {
                "this backup key does not open this archive".to_string()
            }
            other => format!("cannot open the archive: {other}"),
        })?;
    std::io::copy(&mut reader, output)
        .map_err(|e| format!("the archive is damaged or incomplete: {e}"))?;
    Ok(())
}

/// The legacy format.
///
/// Layout: MAGIC(8) || nonce_prefix(8) || [chunk_len(4 LE) || ciphertext+tag]* || end_marker(4 zero bytes).
/// Each chunk uses the nonce nonce_prefix(8) || chunk_index(4 LE).
fn encrypt_legacy(
    key: &[u8],
    mut input: impl Read,
    output: &mut impl Write,
) -> Result<(), BoxError> {
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

/// Chunked decryption of everything after the legacy magic, never holding
/// more than one chunk in memory.
fn decrypt_legacy_body(
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

        // `encrypt_legacy` never writes a chunk larger than CHUNK_SIZE, so a
        // bigger length means a corrupt or hostile archive. The length is four
        // bytes read straight out of a downloaded file. Unchecked, the
        // allocation below zeroes up to 4 GiB before the short read can fail,
        // which OOM-kills the engine instead of failing the restore.
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

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
