use std::io;
use std::path::{Path, PathBuf};

pub const PUBLIC_CERT_FILE: &str = "loopback-ca.pem";

pub const PREVIEW_LEAF_DIR: &str = "preview-tls";

pub const PREVIEW_TLS_CERT_ENV: &str = "LUCIDOS_PREVIEW_TLS_CERT";
pub const PREVIEW_TLS_KEY_ENV: &str = "LUCIDOS_PREVIEW_TLS_KEY";

pub const PREVIEW_USER_TLS_CERT_ENV: &str = "LUCIDOS_PREVIEW_USER_TLS_CERT";
pub const PREVIEW_USER_TLS_KEY_ENV: &str = "LUCIDOS_PREVIEW_USER_TLS_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPair {
    pub cert: String,
    pub key: String,
}

impl TlsPair {
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

pub fn preview_leaf_from_env() -> Option<TlsPair> {
    TlsPair::from_env(PREVIEW_TLS_CERT_ENV, PREVIEW_TLS_KEY_ENV)
}

pub fn preview_user_from_env() -> Option<TlsPair> {
    TlsPair::from_env(PREVIEW_USER_TLS_CERT_ENV, PREVIEW_USER_TLS_KEY_ENV)
}

pub fn public_cert_path() -> Option<PathBuf> {
    public_cert_in(lucidos_home::private::relocated(), lucidos_home::path())
}

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

pub fn preview_leaf_dir() -> Option<PathBuf> {
    Some(public_cert_path()?.with_file_name(PREVIEW_LEAF_DIR))
}

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
