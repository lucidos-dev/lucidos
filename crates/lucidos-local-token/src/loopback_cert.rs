use std::io;
use std::path::{Path, PathBuf};

pub const PUBLIC_CERT_FILE: &str = "loopback-ca.pem";

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
