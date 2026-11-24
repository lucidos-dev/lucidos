//! Where the loopback CA's public certificate lives. The CA crate writes it,
//! and a loopback client reads it. Both resolve it here, so the path has one
//! definition (invariant I17 of `docs/plans/2026-11-19-desktop-window-https.md`).
//! The frontend preview's certificate pairs, which the gateway hands each
//! engine, are named here too.

use std::io;
use std::path::{Path, PathBuf};

/// The public copy's file name, beside the private subtree.
pub const PUBLIC_CERT_FILE: &str = "loopback-ca.pem";

/// The directory beside the public copy that holds the frontend preview's
/// leaf. Outside the private subtree, because the agent sandbox Vite runs in
/// hides that subtree.
pub const PREVIEW_LEAF_DIR: &str = "preview-tls";

/// The preview leaf's pair, which the gateway hands every engine it spawns
/// and the engine hands Vite. Vite serves loopback with it.
pub const PREVIEW_TLS_CERT_ENV: &str = "LUCIDOS_PREVIEW_TLS_CERT";
pub const PREVIEW_TLS_KEY_ENV: &str = "LUCIDOS_PREVIEW_TLS_KEY";

/// The gateway's user certificate, handed on the same way when one exists.
/// Vite serves every other name with it. Absent, Vite binds loopback only.
pub const PREVIEW_USER_TLS_CERT_ENV: &str = "LUCIDOS_PREVIEW_USER_TLS_CERT";
pub const PREVIEW_USER_TLS_KEY_ENV: &str = "LUCIDOS_PREVIEW_USER_TLS_KEY";

/// A certificate and key, by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPair {
    pub cert: String,
    pub key: String,
}

impl TlsPair {
    /// Both paths, trimmed, or `None`: half a pair is no pair.
    pub fn from_values(cert: Option<&str>, key: Option<&str>) -> Option<TlsPair> {
        let cert = cert.map(str::trim).filter(|c| !c.is_empty())?;
        let key = key.map(str::trim).filter(|k| !k.is_empty())?;
        Some(TlsPair {
            cert: cert.to_string(),
            key: key.to_string(),
        })
    }

    pub fn files(&self) -> [&str; 2] {
        [&self.cert, &self.key]
    }

    fn from_env(cert_env: &str, key_env: &str) -> Option<TlsPair> {
        let var = |name: &str| std::env::var(name).ok();
        Self::from_values(var(cert_env).as_deref(), var(key_env).as_deref())
    }
}

/// The preview leaf this process was handed.
pub fn preview_leaf_from_env() -> Option<TlsPair> {
    TlsPair::from_env(PREVIEW_TLS_CERT_ENV, PREVIEW_TLS_KEY_ENV)
}

/// The user certificate this process was handed for the preview.
pub fn preview_user_from_env() -> Option<TlsPair> {
    TlsPair::from_env(PREVIEW_USER_TLS_CERT_ENV, PREVIEW_USER_TLS_KEY_ENV)
}

/// The public copy in the Lucidos home. A relocated private subtree takes the
/// copy beside it, so an e2e run never overwrites the user's. `None` only when
/// `HOME` is unset and nothing relocates the private subtree.
pub fn public_cert_path() -> Option<PathBuf> {
    public_cert_in(lucidos_home::private::relocated(), lucidos_home::path())
}

/// The CA certificate as PEM, from the public copy at `path`. Reads nothing
/// private. A missing copy names its cure.
pub fn read_at(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "cannot read the loopback CA certificate {}: {e}. Start Lucidos once to create \
                 it.",
                path.display()
            ),
        )
    })
}

/// Where the gateway writes the preview leaf: beside the public copy.
pub fn preview_leaf_dir() -> Option<PathBuf> {
    Some(public_cert_path()?.with_file_name(PREVIEW_LEAF_DIR))
}

/// [`read_at`] the public copy in the Lucidos home.
pub fn read() -> io::Result<String> {
    let path = public_cert_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory, so no loopback CA certificate",
        )
    })?;
    read_at(&path)
}

fn public_cert_in(relocated: Option<PathBuf>, lucidos_home: Option<PathBuf>) -> Option<PathBuf> {
    match relocated {
        Some(private) => Some(private.with_file_name(PUBLIC_CERT_FILE)),
        None => Some(lucidos_home?.join(PUBLIC_CERT_FILE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_copy_follows_a_relocated_private_subtree() {
        let home = PathBuf::from("/home/user/.lucidos");
        assert_eq!(
            public_cert_in(None, Some(home.clone())),
            Some(home.join(PUBLIC_CERT_FILE))
        );
        assert_eq!(
            public_cert_in(Some(PathBuf::from("/tmp/e2e/private")), Some(home)),
            Some(PathBuf::from("/tmp/e2e").join(PUBLIC_CERT_FILE))
        );
        assert_eq!(public_cert_in(None, None), None);
    }

    #[test]
    fn half_a_pair_is_no_pair() {
        let (cert, key) = ("/certs/cert.pem", "/certs/key.pem");
        for (c, k) in [(None, None), (Some(cert), None), (Some(cert), Some(" "))] {
            assert_eq!(TlsPair::from_values(c, k), None, "{c:?} / {k:?}");
        }
        assert_eq!(
            TlsPair::from_values(Some(" /certs/cert.pem "), Some("/certs/key.pem\n")),
            TlsPair::from_values(Some(cert), Some(key))
        );
        assert_eq!(
            TlsPair::from_values(Some(cert), Some(key)).unwrap().files(),
            [cert, key]
        );
    }

    /// `scripts/lib/loopback_curl.sh` cannot import this crate, so it carries
    /// a copy of the file name.
    #[test]
    fn the_shell_twin_names_the_same_copy() {
        const LOOPBACK_CURL_SH: &str = include_str!("../../../scripts/lib/loopback_curl.sh");
        let file = format!("LUCIDOS_LOOPBACK_CA_FILE=\"{PUBLIC_CERT_FILE}\"");
        assert!(LOOPBACK_CURL_SH.contains(&file), "loopback_curl.sh: {file}");
        let relocated = format!("\"$(dirname \"${}\")\"", lucidos_home::private::DIR_ENV);
        assert!(
            LOOPBACK_CURL_SH.contains(&relocated),
            "loopback_curl.sh follows {}",
            lucidos_home::private::DIR_ENV
        );
    }
}
