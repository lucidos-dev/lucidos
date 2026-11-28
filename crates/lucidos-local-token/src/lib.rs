use std::path::{Path, PathBuf};

#[cfg(feature = "client")]
pub mod client;
pub mod loopback_cert;
#[cfg(feature = "test-support")]
pub mod test_support;

pub const HEADER_LOCAL_TOKEN: &str = "x-lucidos-local-token";

pub const HEADER_HOOK_SOCKET_TOKEN: &str = "x-lucidos-hook-socket-token";

pub const LOCAL_TOKEN_FILE: &str = lucidos_home::private::LOCAL_TOKEN;

pub const HOOK_SOCKET_TOKEN_FILE: &str = lucidos_home::private::HOOK_SOCKET_TOKEN;

pub const SHARED_PG_PASSWORD_FILE: &str = lucidos_home::private::SHARED_PG_PASSWORD;

pub const TOKEN_BYTES: usize = 32;

pub const MINT_LOCK_SUFFIX: &str = lucidos_home::private::MINT_LOCK_SUFFIX;

pub fn path_for(name: &str) -> Option<PathBuf> {
    lucidos_home::private_entry(name)
}

pub fn path() -> Option<PathBuf> {
    path_for(LOCAL_TOKEN_FILE)
}

pub fn read_named(name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path_for(name)?).ok()?;
    let token = raw.trim().to_string();
    (!token.is_empty()).then_some(token)
}

pub fn read() -> Option<String> {
    read_named(LOCAL_TOKEN_FILE)
}

pub fn ensure_named(name: &str) -> std::io::Result<String> {
    ensure_at(&lucidos_home::ensure_private_entry(name)?)
}

pub fn ensure_at(path: &Path) -> std::io::Result<String> {
    ensure_with(path, || {})
}

fn ensure_with(path: &Path, before_publish: impl FnOnce()) -> std::io::Result<String> {
    if let Some(parent) = path.parent() {
        lucidos_home::ensure_storage_root(parent)?;
    }
    let mint_lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(mint_lock(path)?)?;
    mint_lock.lock()?;
    if let Some(token) = read_published(path)? {
        return Ok(token);
    }
    let minted = mint_hex(TOKEN_BYTES)?;
    let staged = sibling(path, &format!("{}.tmp", std::process::id()))?;
    write_owner_only(&staged, &minted)?;
    before_publish();
    std::fs::rename(&staged, path)?;
    Ok(minted)
}

fn sibling(path: &Path, suffix: &str) -> std::io::Result<PathBuf> {
    Ok(path.with_file_name(format!(".{}.{suffix}", token_name(path)?)))
}

fn mint_lock(path: &Path) -> std::io::Result<PathBuf> {
    Ok(path.with_file_name(lucidos_home::private::mint_lock_name(token_name(path)?)))
}

fn token_name(path: &Path) -> std::io::Result<&str> {
    path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "token path has no name")
    })
}

fn read_published(path: &Path) -> std::io::Result<Option<String>> {
    let existing = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let token = existing.trim().to_string();
    if token.is_empty() {
        return Ok(None);
    }
    enforce_owner_only(path)?;
    Ok(Some(token))
}

pub fn ensure() -> std::io::Result<String> {
    ensure_named(LOCAL_TOKEN_FILE)
}

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
    file.sync_all()
}

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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "abc").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a world-readable token is a leaked token");
    }

    #[test]
    fn the_two_credentials_are_two_files_and_two_headers() {
        assert_ne!(LOCAL_TOKEN_FILE, HOOK_SOCKET_TOKEN_FILE);
        assert_ne!(HEADER_LOCAL_TOKEN, HEADER_HOOK_SOCKET_TOKEN);
        assert!(HEADER_HOOK_SOCKET_TOKEN.starts_with("x-lucidos-"));
    }

    #[test]
    fn a_named_path_is_its_private_entry() {
        let Some(local) = path_for(LOCAL_TOKEN_FILE) else {
            return;
        };
        let hook = path_for(HOOK_SOCKET_TOKEN_FILE).expect("HOME was readable a line ago");
        assert_ne!(local, hook);
        assert_eq!(local, path().expect("the same lookup"));
        assert_eq!(
            Some(&local),
            lucidos_home::private_entry(LOCAL_TOKEN_FILE).as_ref()
        );
        let home = lucidos_home::path().expect("HOME was readable a line ago");
        assert!(local.starts_with(&home), "{local:?}");
        assert!(hook.ends_with(HOOK_SOCKET_TOKEN_FILE), "{hook:?}");
    }

    #[test]
    fn a_mint_creates_once_and_then_reads_what_is_there() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        let minted = ensure_at(&path).expect("a writable scratch directory");
        assert_eq!(minted.len(), TOKEN_BYTES * 2);
        assert_eq!(
            ensure_at(&path).unwrap(),
            minted,
            "a second call must not replace a token callers already hold"
        );
    }

    #[test]
    fn a_writer_that_loses_the_race_reads_the_winners_token() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "the-winner").unwrap();
        assert_eq!(ensure_at(&path).unwrap(), "the-winner");
    }

    #[test]
    fn a_second_minter_adopts_the_token_the_first_publishes() {
        use std::sync::mpsc;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        let (staged_tx, staged_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let first = {
            let path = path.clone();
            std::thread::spawn(move || {
                ensure_with(&path, || {
                    staged_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                })
            })
        };
        staged_rx.recv().unwrap();
        let second = {
            let path = path.clone();
            std::thread::spawn(move || ensure_at(&path))
        };
        resume_tx.send(()).unwrap();
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        assert_eq!(second, first, "both minters must trust the published token");
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".lock"))
            .collect();
        assert_eq!(names, ["local-token"], "every staging file is gone");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_token_file_is_an_error_not_an_empty_slot() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "still-in-use").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let outcome = ensure_at(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            outcome.is_err(),
            "an unreadable token must surface, not be minted over: {outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().trim(),
            "still-in-use",
            "the unreadable token must survive"
        );
    }

    #[test]
    fn a_blank_file_from_an_earlier_mint_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "   \n").unwrap();
        let minted = ensure_at(&path).unwrap();
        assert_eq!(minted.len(), TOKEN_BYTES * 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), minted);
    }

    #[cfg(unix)]
    #[test]
    fn a_minted_token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        ensure_at(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a world-readable token is a leaked token");
    }

    #[cfg(unix)]
    #[test]
    fn minting_makes_the_home_a_storage_root() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_at(&dir.join("local-token")).unwrap();
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, lucidos_home::STORAGE_ROOT_MODE);
    }

    #[test]
    fn an_older_and_a_newer_minter_meet_one_token_in_an_older_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".lucidos");
        std::fs::create_dir_all(&home).unwrap();
        let older = ensure_at(&home.join(LOCAL_TOKEN_FILE)).unwrap();

        let newer = lucidos_home::private::entry_in(&home, LOCAL_TOKEN_FILE);
        assert_eq!(ensure_at(&newer).unwrap(), older);
        assert!(!lucidos_home::private::path_in(&home).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_loosened_token_file_is_repaired() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "abc").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        enforce_owner_only(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
