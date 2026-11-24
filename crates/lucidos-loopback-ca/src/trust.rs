//! Trusting the loopback CA in the user's own certificate store.
//!
//! - **macOS**: the login keychain, user trust domain, through `security`.
//!   WKWebView, Safari and Chrome read it. Never the admin domain (`-d`) or
//!   the System keychain.
//! - **Linux**: the NSS user database (`~/.pki/nssdb`), which Chrome reads,
//!   the snap Chromium's database and each Firefox profile, through
//!   `certutil`. Never `/etc`.
//!
//! Status comes from the store's own tool, never from a guess. Each call runs
//! once: nothing here loops or retries.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use sha2::{Digest, Sha256};

use crate::{CaDir, COMMON_NAME};

/// Where the cure for a missing `certutil` points, per distribution family.
pub const CERTUTIL_MISSING: &str = "certutil is not installed, so Lucidos cannot trust its local \
     certificate. Install it (Debian and Ubuntu: libnss3-tools; Fedora: nss-tools; Arch: nss), \
     then run `lucidos tls trust`.";

/// The command that trusts the CA, named in every failure.
const TRY_AGAIN: &str = "Run `lucidos tls trust` to try again.";

/// Firefox profile roots under `$HOME`: the plain install (the classic path
/// and the XDG one newer releases use), the snap and the flatpak. Each holds a
/// `profiles.ini` naming its profiles.
const FIREFOX_ROOTS: [&str; 4] = [
    ".mozilla/firefox",
    ".config/mozilla/firefox",
    "snap/firefox/common/.mozilla/firefox",
    ".var/app/org.mozilla.firefox/.mozilla/firefox",
];

/// The NSS user database under `$HOME`.
const NSS_USER_DB: &str = ".pki/nssdb";

/// The snap Chromium's own NSS database under `$HOME`, which Ubuntu installs
/// by default. The snap cannot read the user database.
const SNAP_CHROMIUM_DB: &str = "snap/chromium/current/.pki/nssdb";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// Whether this user's browsers and webviews trust the loopback CA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustStatus {
    Trusted,
    /// Not trusted yet; says which store lacks it.
    Untrusted(String),
    /// The store's tool could not answer; says how to fix that.
    Unavailable(String),
}

/// One command, as data, so a test can read every argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argv {
    pub program: &'static str,
    pub args: Vec<OsString>,
}

impl Argv {
    fn new(program: &'static str, args: impl IntoIterator<Item = impl Into<OsString>>) -> Argv {
        Argv {
            program,
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// Runs an [`Argv`]. The seam a test replaces.
pub trait Run {
    fn run(&self, argv: &Argv) -> io::Result<Output>;
}

/// Runs commands for real.
pub struct System;

impl Run for System {
    fn run(&self, argv: &Argv) -> io::Result<Output> {
        std::process::Command::new(argv.program)
            .args(&argv.args)
            .output()
    }
}

/// The stores to act on: the platform and the user's home.
pub struct Stores {
    pub platform: Platform,
    pub home: PathBuf,
}

impl Stores {
    /// This machine's platform and `$HOME`.
    pub fn current() -> io::Result<Stores> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Stores {
            platform: Platform::current(),
            home: PathBuf::from(home),
        })
    }
}

/// Whether the user's stores trust the CA in `ca`.
pub fn status(ca: &CaDir) -> TrustStatus {
    match Stores::current() {
        Ok(stores) => status_with(&System, &stores, ca),
        Err(e) => TrustStatus::Unavailable(e.to_string()),
    }
}

/// Trust the CA in `ca` in the user's stores. On macOS this shows the
/// system's password prompt.
pub fn trust(ca: &CaDir) -> io::Result<()> {
    trust_with(&System, &Stores::current()?, ca)
}

/// Trust the CA in `ca`. On macOS it skips a CA the keychain already trusts, so
/// a re-run install shows no second password prompt. Linux always re-adds it:
/// its status sees only that the CA is present, not its trust flags.
pub fn ensure_trusted(ca: &CaDir) -> io::Result<()> {
    ensure_trusted_with(&System, &Stores::current()?, ca)
}

pub fn ensure_trusted_with(run: &dyn Run, stores: &Stores, ca: &CaDir) -> io::Result<()> {
    let prompts = stores.platform == Platform::MacOs;
    if prompts && status_with(run, stores, ca) == TrustStatus::Trusted {
        return Ok(());
    }
    trust_with(run, stores, ca)
}

/// Remove the CA in `ca` from the user's stores, for an uninstall.
pub fn untrust(ca: &CaDir) -> io::Result<()> {
    untrust_with(&System, &Stores::current()?, ca)
}

/// Remove the CA's trust, then the CA and its public copy, for an uninstall.
/// False when there was no CA to forget.
///
/// The CA is per user, so another install for this user loses it too. Its
/// gateway mints a fresh one at its next start and asks for trust again.
pub fn forget(ca: &CaDir) -> io::Result<bool> {
    forget_with(&System, &Stores::current()?, ca)
}

pub fn forget_with(run: &dyn Run, stores: &Stores, ca: &CaDir) -> io::Result<bool> {
    if !ca.public_cert_path().exists() && !ca.path().exists() {
        return Ok(false);
    }
    if ca.public_cert_path().exists() || ca.cert_path().exists() {
        untrust_with(run, stores, ca)?;
    }
    for path in [ca.path(), ca.public_cert_path()] {
        let gone = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        gone.map_err(|e| io::Error::new(e.kind(), format!("delete {}: {e}", path.display())))?;
    }
    Ok(true)
}

pub fn status_with(run: &dyn Run, stores: &Stores, ca: &CaDir) -> TrustStatus {
    let pem = match ca.read_certificate() {
        Ok(pem) => pem,
        Err(e) => return TrustStatus::Untrusted(e.to_string()),
    };
    match stores.platform {
        Platform::MacOs => match run.run(&macos::verify(&ca.cert_path())) {
            Ok(out) if out.status.success() => TrustStatus::Trusted,
            Ok(_) => TrustStatus::Untrusted(
                "the login keychain does not trust the Lucidos loopback CA yet".to_string(),
            ),
            Err(e) => TrustStatus::Unavailable(format!("cannot run `security`: {e}")),
        },
        Platform::Linux => {
            let dbs = linux::databases(&stores.home);
            if dbs.is_empty() {
                return TrustStatus::Untrusted(format!(
                    "no NSS database exists yet at ~/{NSS_USER_DB}"
                ));
            }
            let mut missing = Vec::new();
            for db in dbs {
                match run.run(&linux::list(&db)) {
                    Ok(out) if out.status.success() && holds(&out.stdout, &pem) => {}
                    Ok(_) => missing.push(db.display().to_string()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return TrustStatus::Unavailable(CERTUTIL_MISSING.to_string())
                    }
                    Err(e) => return TrustStatus::Unavailable(format!("cannot run certutil: {e}")),
                }
            }
            if missing.is_empty() {
                TrustStatus::Trusted
            } else {
                TrustStatus::Untrusted(format!("not trusted in {}", missing.join(", ")))
            }
        }
    }
}

pub fn trust_with(run: &dyn Run, stores: &Stores, ca: &CaDir) -> io::Result<()> {
    ca.read_certificate()?;
    match stores.platform {
        Platform::MacOs => {
            let keychain = macos::login_keychain(run)?;
            succeed(run, &macos::add(&ca.cert_path(), &keychain), "security")
        }
        Platform::Linux => {
            let user_db = stores.home.join(NSS_USER_DB);
            if !user_db.join("cert9.db").exists() {
                std::fs::create_dir_all(&user_db)?;
                succeed(run, &linux::create(&user_db), "certutil")?;
            }
            for db in linux::databases(&stores.home) {
                // Replaces an older CA under the same nickname. Absent is the
                // usual case, and the add below reports any real failure.
                let _ = run.run(&linux::delete(&db));
                succeed(run, &linux::add(&db, &ca.cert_path()), "certutil")?;
            }
            Ok(())
        }
    }
}

/// Removes only what is there, so a CA the user never trusted, or trusted in
/// some stores only, still uninstalls cleanly.
pub fn untrust_with(run: &dyn Run, stores: &Stores, ca: &CaDir) -> io::Result<()> {
    match stores.platform {
        Platform::MacOs => {
            // The private certificate stands in when the public copy is gone,
            // so an uninstall still finds what the keychain holds.
            let pem = ca
                .read_certificate()
                .or_else(|_| lucidos_local_token::loopback_cert::read_at(&ca.cert_path()))?;
            let keychain = macos::login_keychain(run)?;
            // Removing a trust setting that never existed fails. Only a CA
            // still trusted afterwards makes that failure real.
            if let Err(e) = succeed(run, &macos::remove_trust(&ca.cert_path()), "security") {
                let still_trusted = run
                    .run(&macos::verify(&ca.cert_path()))
                    .map_or(true, |out| out.status.success());
                if still_trusted {
                    return Err(e);
                }
            }
            let digest = fingerprint(&pem)?;
            if macos::holds(run, &keychain, &digest)? {
                succeed(run, &macos::delete(&keychain, &digest), "security")?;
            }
            Ok(())
        }
        Platform::Linux => {
            for db in linux::databases(&stores.home) {
                if run_tool(run, &linux::list(&db), "certutil")?
                    .status
                    .success()
                {
                    succeed(run, &linux::delete(&db), "certutil")?;
                }
            }
            Ok(())
        }
    }
}

/// Run `argv`, failing only when `tool` itself cannot start.
fn run_tool(run: &dyn Run, argv: &Argv, tool: &str) -> io::Result<Output> {
    run.run(argv).map_err(|e| {
        if tool == "certutil" && e.kind() == io::ErrorKind::NotFound {
            io::Error::new(io::ErrorKind::NotFound, CERTUTIL_MISSING)
        } else {
            io::Error::new(e.kind(), format!("cannot run {tool}: {e}. {TRY_AGAIN}"))
        }
    })
}

/// Run `argv` and fail, naming the tool and its own words, unless it succeeds.
fn succeed(run: &dyn Run, argv: &Argv, tool: &str) -> io::Result<()> {
    let out = run_tool(run, argv, tool)?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(io::Error::other(format!(
        "`{tool} {}` failed: {}. {TRY_AGAIN}",
        argv.args
            .first()
            .map(|a| a.to_string_lossy())
            .unwrap_or_default(),
        said.trim()
    )))
}

/// Whether a PEM listing holds the certificate `pem`, whatever its wrapping.
fn holds(listing: &[u8], pem: &str) -> bool {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    squash(&String::from_utf8_lossy(listing)).contains(&squash(pem))
}

/// The SHA-256 fingerprint of the certificate `pem`, uppercase hex, as
/// `security delete-certificate -Z` takes it.
fn fingerprint(pem: &str) -> io::Result<String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let der = CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(Sha256::digest(der.as_ref())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect())
}

mod macos {
    use super::*;

    /// Exit 0 when the keychain trusts the CA. `-l` because the leaf checked
    /// is itself a CA, `-L` to stay offline.
    pub fn verify(ca: &Path) -> Argv {
        Argv::new(
            "security",
            [
                OsString::from("verify-cert"),
                "-c".into(),
                ca.into(),
                "-l".into(),
                "-L".into(),
                "-q".into(),
            ],
        )
    }

    pub fn ask_login_keychain() -> Argv {
        Argv::new("security", ["login-keychain"])
    }

    /// No `-d`: the user trust domain, set with the user's own password.
    pub fn add(ca: &Path, keychain: &Path) -> Argv {
        Argv::new(
            "security",
            [
                OsString::from("add-trusted-cert"),
                "-r".into(),
                "trustRoot".into(),
                "-k".into(),
                keychain.into(),
                ca.into(),
            ],
        )
    }

    pub fn remove_trust(ca: &Path) -> Argv {
        Argv::new(
            "security",
            [OsString::from("remove-trusted-cert"), ca.into()],
        )
    }

    pub fn delete(keychain: &Path, sha256: &str) -> Argv {
        Argv::new(
            "security",
            [
                OsString::from("delete-certificate"),
                "-Z".into(),
                sha256.into(),
                keychain.into(),
            ],
        )
    }

    /// Lists every certificate under the CA's name in `keychain`, with its
    /// SHA-256 hash. Exits non-zero when there is none.
    pub fn find(keychain: &Path) -> Argv {
        Argv::new(
            "security",
            [
                OsString::from("find-certificate"),
                "-a".into(),
                "-Z".into(),
                "-c".into(),
                COMMON_NAME.into(),
                keychain.into(),
            ],
        )
    }

    /// Whether `keychain` holds the certificate with SHA-256 `sha256`.
    pub fn holds(run: &dyn Run, keychain: &Path, sha256: &str) -> io::Result<bool> {
        let out = run_tool(run, &find(keychain), "security")?;
        Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).contains(sha256))
    }

    /// The login keychain, as `security` names it.
    pub fn login_keychain(run: &dyn Run) -> io::Result<PathBuf> {
        let argv = ask_login_keychain();
        let out = run
            .run(&argv)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot run security: {e}")))?;
        let path = String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_matches('"')
            .to_string();
        if !out.status.success() || path.is_empty() {
            return Err(io::Error::other(format!(
                "this user has no login keychain, so Lucidos cannot trust its local \
                 certificate. {TRY_AGAIN}"
            )));
        }
        Ok(PathBuf::from(path))
    }
}

mod linux {
    use super::*;

    /// Every NSS database this user's browsers read that exists: the user
    /// database, the snap Chromium's, then each Firefox profile
    /// `profiles.ini` names.
    pub fn databases(home: &Path) -> Vec<PathBuf> {
        let mut dbs: Vec<PathBuf> = [home.join(NSS_USER_DB), home.join(SNAP_CHROMIUM_DB)]
            .into_iter()
            .chain(
                FIREFOX_ROOTS
                    .iter()
                    .flat_map(|root| firefox_profiles(&home.join(root))),
            )
            .filter(|db| db.join("cert9.db").exists())
            .collect();
        dbs.dedup();
        dbs
    }

    /// The profile directories a Firefox `profiles.ini` under `root` names.
    pub fn firefox_profiles(root: &Path) -> Vec<PathBuf> {
        let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else {
            return Vec::new();
        };
        let mut profiles = Vec::new();
        let mut in_profile = false;
        let mut path: Option<String> = None;
        let mut relative = true;
        let mut flush = |path: &mut Option<String>, relative: bool| {
            if let Some(p) = path.take() {
                profiles.push(if relative {
                    root.join(p)
                } else {
                    PathBuf::from(p)
                });
            }
        };
        for line in ini.lines().map(str::trim) {
            if line.starts_with('[') {
                if in_profile {
                    flush(&mut path, relative);
                }
                in_profile = line.starts_with("[Profile");
                relative = true;
            } else if in_profile {
                if let Some(p) = line.strip_prefix("Path=") {
                    path = Some(p.to_string());
                } else if let Some(r) = line.strip_prefix("IsRelative=") {
                    relative = r != "0";
                }
            }
        }
        if in_profile {
            flush(&mut path, relative);
        }
        profiles
    }

    fn db_arg(db: &Path) -> OsString {
        let mut arg = OsString::from("sql:");
        arg.push(db);
        arg
    }

    pub fn list(db: &Path) -> Argv {
        Argv::new(
            "certutil",
            [
                OsString::from("-L"),
                "-d".into(),
                db_arg(db),
                "-n".into(),
                COMMON_NAME.into(),
                "-a".into(),
            ],
        )
    }

    pub fn create(db: &Path) -> Argv {
        Argv::new(
            "certutil",
            [
                OsString::from("-N"),
                "-d".into(),
                db_arg(db),
                "--empty-password".into(),
            ],
        )
    }

    /// `C,,`: trusted to issue server certificates, nothing else.
    pub fn add(db: &Path, ca: &Path) -> Argv {
        Argv::new(
            "certutil",
            [
                OsString::from("-A"),
                "-d".into(),
                db_arg(db),
                "-t".into(),
                "C,,".into(),
                "-n".into(),
                COMMON_NAME.into(),
                "-i".into(),
                ca.into(),
            ],
        )
    }

    pub fn delete(db: &Path) -> Argv {
        Argv::new(
            "certutil",
            [
                OsString::from("-D"),
                "-d".into(),
                db_arg(db),
                "-n".into(),
                COMMON_NAME.into(),
            ],
        )
    }
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
