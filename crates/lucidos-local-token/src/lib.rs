//! The machine-local token: proof that a caller is a process on this machine,
//! running as this user.
//!
//! Full design: `docs/plans/2026-08-17-clients-pair-to-the-gateway-and-webhooks-get-their-own-socket.md`.
//!
//! # Why a file, and not a loopback check
//!
//! The obvious design is "trust `127.0.0.1`", and it fails open here. Lucidos
//! is reached remotely through
//! `tailscale serve --bg --https=443 http://127.0.0.1:5252`, which proxies from
//! *this* machine. A request from a phone therefore arrives with a loopback
//! peer address, so trusting loopback would trust every remote caller.
//!
//! Reading a mode 0600 file is the thing a remote caller cannot do. It states
//! trust that already exists, since a local shell can read every credential and
//! drive every workspace anyway. `tailscaled` uses the same pattern for its own
//! LocalAPI, as do the Docker socket and kubeconfig.

use std::path::{Path, PathBuf};

/// Header carrying the token.
pub const HEADER_LOCAL_TOKEN: &str = "x-lucidos-local-token";

/// Bytes of entropy behind a token. 32 bytes is 256 bits, far past what a
/// guessing attack could reach through an HTTP endpoint.
const TOKEN_BYTES: usize = 32;

/// `~/.lucidos/local-token`. `None` only when `HOME` is unset.
///
/// Machine-global and outside every git tree, alongside `network.toml`.
pub fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".lucidos/local-token"))
}

/// The token, or `None` when this machine has no gateway that minted one.
///
/// A missing file is normal rather than an error. A workspace can be launched
/// with no gateway at all, and callers treat the absence as "send no header".
pub fn read() -> Option<String> {
    let raw = std::fs::read_to_string(path()?).ok()?;
    let token = raw.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Read the token, minting it on first use. The gateway owns this call.
///
/// Re-asserts mode 0600 every time. A file left readable by a stray `chmod` is
/// then repaired at startup rather than trusted as it stands.
pub fn ensure() -> std::io::Result<String> {
    let path = path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim().to_string();
        if !token.is_empty() {
            enforce_owner_only(&path)?;
            return Ok(token);
        }
    }
    let token = mint_hex(TOKEN_BYTES)?;
    write_owner_only(&path, &token)?;
    Ok(token)
}

/// `bytes` bytes from `/dev/urandom`, lowercase hex.
///
/// Read straight from the device rather than through a crate, because this
/// crate has no dependencies and must keep none. The tree ships macOS and
/// Linux only, so the device is always present.
pub fn mint_hex(bytes: usize) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut buf = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    let mut out = String::with_capacity(bytes * 2);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

/// Create the file with mode 0600 *at creation*, never create-then-chmod. The
/// second shape leaves a window where the secret is world-readable.
pub fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.flush()
}

/// Force mode 0600 on an existing file.
fn enforce_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
        if current != 0o600 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Compare two secrets without leaking their common prefix through timing.
///
/// Length is compared first and non-secretly, which is standard: the width of
/// a fixed-size token is not the secret. The byte loop then covers the full
/// length with no early exit.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_equality_without_early_exit() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "a"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn minting_is_hex_and_full_width() {
        let token = mint_hex(TOKEN_BYTES).expect("/dev/urandom readable");
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            token,
            mint_hex(TOKEN_BYTES).unwrap(),
            "tokens must not repeat"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("lucidos-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("local-token");
        write_owner_only(&path, "abc").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a world-readable token is a leaked token");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_loosened_token_file_is_repaired() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("lucidos-token-fix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("local-token");
        write_owner_only(&path, "abc").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        enforce_owner_only(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
