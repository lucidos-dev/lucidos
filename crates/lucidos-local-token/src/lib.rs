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
//!
//! # Two credentials, because a scope must be a secret
//!
//! [`LOCAL_TOKEN_FILE`] carries full authority. [`HOOK_SOCKET_TOKEN_FILE`]
//! carries one route: the engine's webhook delivery. Two files rather than one token
//! plus a scope field, because a scope the caller states is not a scope. The
//! bearer would state the widest one.
//!
//! The split has a caller behind it. The gateway's hook socket forwards a
//! delivery that arrived from the open internet (ADR 0097), and it strips every
//! inbound `x-lucidos-*` before doing so. Handing that hop the full-authority
//! token would put it one forwarding bug away from the whole engine API.
//!
//! Both are mode 0600 and both live beside `network.toml`. The gateway and
//! the engine each read or mint both, whichever boots first
//! (`api::local_auth::EngineAuth::resolve`). For the local token, the engine's
//! own build-watch and release scripts have no other credential (ADR 0169).
//! For the hook socket token, an engine that cached "absent" at boot would
//! refuse every delivery from a gateway that started later. Only the gateway's
//! hook socket ever presents that one.

//! # Where the gateway sends them
//!
//! Only to an engine's engine socket, and only once the kernel names the
//! listener as this user (`lucidos-engine-socket`, ADR 0445). A loopback TCP
//! port proves nothing about its owner: another local user can bind one an
//! engine left free, and would receive whatever the gateway sent next.

use std::path::{Path, PathBuf};

/// Header carrying the full-authority token.
pub const HEADER_LOCAL_TOKEN: &str = "x-lucidos-local-token";

/// Header carrying the hook socket token.
///
/// A separate name, not a scope claimed inside the local token's header. A
/// scope the caller names is not a scope: whoever holds a credential would
/// simply name the widest one. Which header the secret arrives in is the
/// engine's own reading, and the two secrets differ.
pub const HEADER_HOOK_SOCKET_TOKEN: &str = "x-lucidos-hook-socket-token";

/// The full-authority credential's file name.
pub const LOCAL_TOKEN_FILE: &str = "local-token";

/// The hook socket token's file name: the credential for webhook delivery.
///
/// Distinct from [`LOCAL_TOKEN_FILE`] so the hook socket can prove exactly one
/// thing. It is handed to whatever `tailscale funnel` exposes, which is the one
/// Lucidos surface a user may point at the open internet (ADR 0097). A process
/// holding it must not be able to restart a workspace.
pub const HOOK_SOCKET_TOKEN_FILE: &str = "hook-socket-token";

/// The file holding the password of the shared dev cluster.
///
/// Docker Desktop cannot hand a Unix socket from its VM to the host. So the dev
/// cluster listens on loopback TCP, which every local user can reach, and a
/// minted secret keeps them out. The dev scripts mint the same file under the
/// same lock (`scripts/lib/pg_secret.sh`).
pub const SHARED_PG_PASSWORD_FILE: &str = "pg-shared-password";

/// Bytes of entropy behind a token. 32 bytes is 256 bits, far past what a
/// guessing attack could reach through an HTTP endpoint.
pub const TOKEN_BYTES: usize = 32;

/// Suffix of the lock file every minter of `<name>` holds: `.<name>.lock`,
/// beside the token. `scripts/lib/pg_secret.sh` takes the same lock.
pub const MINT_LOCK_SUFFIX: &str = "lock";

/// `~/.lucidos/<name>`. `None` only when `HOME` is unset.
///
/// Machine-global and outside every git tree, alongside `network.toml`.
pub fn path_for(name: &str) -> Option<PathBuf> {
    Some(lucidos_home::path()?.join(name))
}

/// `~/.lucidos/local-token`. `None` only when `HOME` is unset.
pub fn path() -> Option<PathBuf> {
    path_for(LOCAL_TOKEN_FILE)
}

/// The named token, or `None` when this machine has no gateway that minted one.
///
/// A missing file is normal rather than an error. A workspace can be launched
/// with no gateway at all, and callers treat the absence as "send no header".
pub fn read_named(name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path_for(name)?).ok()?;
    let token = raw.trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// The full-authority token. See [`read_named`].
pub fn read() -> Option<String> {
    read_named(LOCAL_TOKEN_FILE)
}

/// Read the named token, minting it on first use.
///
/// Re-asserts mode 0600 every time. A file left readable by a stray `chmod` is
/// then repaired at startup rather than trusted as it stands.
///
/// **First writer wins.** The gateway is no longer the only caller: an engine
/// launched with no gateway mints its own, so two of them starting together
/// race for the same path. Every minter holds the mint lock across its read,
/// mint and publish, so the second one reads what the first published. The
/// publish is a rename of a whole temp file, so a lock-free reader never sees
/// a partial token either.
pub fn ensure_named(name: &str) -> std::io::Result<String> {
    let path = path_for(name)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    ensure_at(&path)
}

/// [`ensure_named`] at an explicit path, so the mint is testable without
/// writing into the machine's own `~/.lucidos`.
fn ensure_at(path: &Path) -> std::io::Result<String> {
    ensure_with(path, || {})
}

/// [`ensure_at`], with `before_publish` run between the temp write and the
/// rename. The seam a test pauses on to replay a rival's interleaving.
fn ensure_with(path: &Path, before_publish: impl FnOnce()) -> std::io::Result<String> {
    if let Some(parent) = path.parent() {
        lucidos_home::ensure_storage_root(parent)?;
    }
    let mint_lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(sibling(path, MINT_LOCK_SUFFIX)?)?;
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

/// `.<name>.<suffix>` beside `path`. Beside it, because a rename cannot cross
/// filesystems. The mint lock is one such file: never delete it, or two
/// minters would lock two different files.
fn sibling(path: &Path, suffix: &str) -> std::io::Result<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "token path has no name")
    })?;
    Ok(path.with_file_name(format!(".{name}.{suffix}")))
}

/// The token at `path`, or `None` when nothing is published yet.
///
/// Called under the mint lock, where no writer is ever part-way through. So a
/// blank file is debris from an earlier release, whose mint wrote in place
/// and could be interrupted. It reads as nothing published, and is replaced.
///
/// A read error other than "no such file" is returned rather than reported as
/// absent. An unreadable file is NOT an empty one: treating a `chmod 000` token
/// as missing would mint over a secret other processes still present.
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

/// Mint or read the full-authority token. See [`ensure_named`].
pub fn ensure() -> std::io::Result<String> {
    ensure_named(LOCAL_TOKEN_FILE)
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
///
/// Replaces what is there. Shared with the gateway's paired-device store,
/// because that creation-time mode is the invariant the module header calls
/// security-critical. A second copy is a second place to keep it right.
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "abc").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a world-readable token is a leaked token");
    }

    #[test]
    fn the_two_credentials_are_two_files_and_two_headers() {
        // The whole scope mechanism. One file, or one header carrying a scope
        // field, would let the hook socket's credential name full authority.
        assert_ne!(LOCAL_TOKEN_FILE, HOOK_SOCKET_TOKEN_FILE);
        assert_ne!(HEADER_LOCAL_TOKEN, HEADER_HOOK_SOCKET_TOKEN);
        // Both are `x-lucidos-*`, which is the prefix the gateway's hook socket
        // strips from every inbound delivery.
        assert!(HEADER_HOOK_SOCKET_TOKEN.starts_with("x-lucidos-"));
    }

    #[test]
    fn a_named_path_stays_inside_the_lucidos_directory() {
        // `HOME` is set in every environment this runs in; skip rather than
        // fail if it somehow is not, since the `None` arm is the contract.
        let Some(local) = path_for(LOCAL_TOKEN_FILE) else {
            return;
        };
        let hook = path_for(HOOK_SOCKET_TOKEN_FILE).expect("HOME was readable a line ago");
        assert_ne!(local, hook);
        assert_eq!(local, path().expect("the same lookup"));
        assert!(local.ends_with(".lucidos/local-token"), "{local:?}");
        assert!(hook.ends_with(".lucidos/hook-socket-token"), "{hook:?}");
    }

    /// The engine mints when no gateway has, and finds the gateway's value
    /// when one did. Both halves of the phase 2b decision, in one test.
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

    /// The race two gateway-less engines can lose. The loser must end up with
    /// the value that landed, never with the one it minted and threw away.
    #[test]
    fn a_writer_that_loses_the_race_reads_the_winners_token() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("local-token");
        write_owner_only(&path, "the-winner").unwrap();
        assert_eq!(ensure_at(&path).unwrap(), "the-winner");
    }

    /// Two minters, the first paused after staging its token and before it
    /// publishes. The second starts then, and must end up with the first's
    /// token rather than mint its own. Synchronised by channels and the mint
    /// lock, never by a sleep.
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

    /// A token this process cannot read is still a token other processes are
    /// presenting. Minting over it would strand every one of them.
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
        // Restore before asserting, so a failure still leaves a removable dir.
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

    /// An earlier release minted in place and could be interrupted, leaving
    /// a blank file. Under the mint lock no writer is part-way through, so the
    /// blank file is debris and a fresh token replaces it.
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
