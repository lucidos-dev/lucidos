//! The *compile mode* of a Lucidos-source agent build (ADR 0392).
//!
//! A mode is what changes a build's fingerprint: today, where OpenSSL comes
//! from. Any change rebuilds the whole tree, so a worktree's `target/` pins its
//! mode, and every spawn and background task in that worktree reads the pin.
//!
//! Only a Lucidos source tree gets a mode. The agent compile env reaches every
//! coding agent, and `OPENSSL_NO_VENDOR` would break another repo's build.

use std::path::{Path, PathBuf};

const PIN_FILE: &str = ".lucidos-compile-mode";
const PREBUILT_ROOT: &str = "openssl";
const PREBUILT_COMPLETE: &str = ".complete";
const SEED_SCRIPT: &str = "scripts/build-openssl-prebuilt.sh";
const SEED_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);
/// The `openssl-src` features openssl-sys asks for when it vendors. The seed
/// builds with them, and the cache key names them, so a prebuilt made with
/// another set is never mistaken for this one.
const PREBUILT_FEATURES: &str = "legacy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenSslSource {
    Vendored,
    Prebuilt(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompileMode {
    openssl: OpenSslSource,
}

impl CompileMode {
    fn vendored() -> Self {
        Self {
            openssl: OpenSslSource::Vendored,
        }
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        match &self.openssl {
            OpenSslSource::Vendored => Vec::new(),
            OpenSslSource::Prebuilt(dir) => vec![
                ("OPENSSL_NO_VENDOR", "1".to_string()),
                ("OPENSSL_DIR", dir.to_string_lossy().into_owned()),
                ("OPENSSL_STATIC", "1".to_string()),
            ],
        }
    }

    fn to_pin(&self) -> String {
        match &self.openssl {
            OpenSslSource::Vendored => "openssl=vendored\n".to_string(),
            OpenSslSource::Prebuilt(dir) => {
                format!("openssl=prebuilt\nopenssl_dir={}\n", dir.display())
            }
        }
    }

    fn from_pin(text: &str) -> Option<Self> {
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        };
        let openssl = match value("openssl")? {
            "vendored" => OpenSslSource::Vendored,
            "prebuilt" => OpenSslSource::Prebuilt(PathBuf::from(value("openssl_dir")?)),
            _ => return None,
        };
        Some(Self { openssl })
    }
}

/// What resolving a worktree's mode found, before anything is written.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub mode: CompileMode,
    /// The pin to write, when it is missing or names a prebuilt that is gone.
    pub pin_to_write: Option<String>,
    /// The prebuilt this lockfile wants and nobody has built yet.
    pub seed: Option<Seed>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Seed {
    pub version: String,
    pub dest: PathBuf,
}

fn is_lucidos_source_tree(worktree: &Path) -> bool {
    worktree.join("crates/lucidos-engine/Cargo.toml").is_file()
}

/// The `openssl-src` version a checkout's lockfile pins.
fn lockfile_openssl_src_version(lockfile: &str) -> Option<String> {
    let mut lines = lockfile.lines();
    lines.find(|line| line.trim() == r#"name = "openssl-src""#)?;
    let version = lines.next()?.trim().strip_prefix("version = \"")?;
    Some(version.strip_suffix('"')?.to_string())
}

/// Where the prebuilt for `version` lives. The key carries the host, so one
/// cache never hands an x86_64 library to an arm64 build.
fn prebuilt_dir(cache_root: &Path, version: &str) -> PathBuf {
    cache_root.join(PREBUILT_ROOT).join(format!(
        "{version}-{PREBUILT_FEATURES}-{}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    ))
}

fn prebuilt_is_complete(dir: &Path) -> bool {
    dir.join(PREBUILT_COMPLETE).is_file()
}

/// Whether a `target/` already holds a vendored OpenSSL build. A worktree
/// built before pins existed has one, and must stay vendored.
fn target_has_vendored_openssl(target: &Path) -> bool {
    let Ok(profiles) = std::fs::read_dir(target) else {
        return false;
    };
    profiles.flatten().any(|profile| {
        let Ok(builds) = std::fs::read_dir(profile.path().join("build")) else {
            return false;
        };
        builds.flatten().any(|build| {
            build
                .file_name()
                .to_string_lossy()
                .starts_with("openssl-sys-")
                && build.path().join("out/openssl-build").is_dir()
        })
    })
}

/// Decide a Lucidos source tree's mode from what is on disk.
pub(crate) fn resolve(worktree: &Path, cache_root: &Path) -> Resolution {
    let target = worktree.join("target");
    let wanted = std::fs::read_to_string(worktree.join("Cargo.lock"))
        .ok()
        .and_then(|lock| lockfile_openssl_src_version(&lock))
        .map(|version| {
            let dest = prebuilt_dir(cache_root, &version);
            (version, dest)
        });
    let seed = wanted.as_ref().and_then(|(version, dest)| {
        (!prebuilt_is_complete(dest)).then(|| Seed {
            version: version.clone(),
            dest: dest.clone(),
        })
    });

    let ready = wanted
        .as_ref()
        .map(|(_, dest)| dest)
        .filter(|dest| prebuilt_is_complete(dest));
    let from_lockfile = || match ready {
        Some(dest) => CompileMode {
            openssl: OpenSslSource::Prebuilt(dest.clone()),
        },
        None => CompileMode::vendored(),
    };
    let pinned = std::fs::read_to_string(target.join(PIN_FILE))
        .ok()
        .and_then(|text| CompileMode::from_pin(&text));
    let (mode, write) = match pinned {
        Some(mode) => match &mode.openssl {
            OpenSslSource::Vendored => (mode, false),
            OpenSslSource::Prebuilt(dir) if Some(dir) == ready => (mode, false),
            // The library is gone, or the lockfile moved to another version.
            // A version bump rebuilds every OpenSSL dependent anyway.
            OpenSslSource::Prebuilt(_) => (from_lockfile(), true),
        },
        None if target_has_vendored_openssl(&target) => (CompileMode::vendored(), true),
        None => (from_lockfile(), true),
    };
    Resolution {
        pin_to_write: write.then(|| mode.to_pin()),
        mode,
        seed,
    }
}

/// `path` with `suffix` appended to its last component. The version in a
/// prebuilt's name carries dots, so `with_extension` would cut it.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Whether a seed ran recently enough that starting another would only repeat
/// it. Its log's age says when the last one started.
fn seed_ran_recently(log: &Path) -> bool {
    std::fs::metadata(log)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < SEED_RETRY_AFTER)
}

/// Start the seed in the background. `sh` forks it off and exits, so the
/// engine never holds a child to reap. The script's own lock refuses a second
/// seed of one version, and clears a lock that a killed seed left behind.
///
/// A failed seed is retried after [`SEED_RETRY_AFTER`], not on every spawn.
/// `RUSTC_WRAPPER` is empty because the engine's own env names the Apply
/// rebuild's sccache daemon, which a build slot's lowered priority must never
/// start (ADR 0343).
fn start_seed(worktree: &Path, seed: &Seed) {
    let log = with_suffix(&seed.dest, ".seed.log");
    if seed_ran_recently(&log) {
        return;
    }
    if let Some(parent) = log.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log!("[CompileMode] cannot create {}: {e}", parent.display());
            return;
        }
    }
    let spawned = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#""$0" "$1" "$2" "$3" > "$4" 2>&1 &"#)
        .arg(worktree.join(SEED_SCRIPT))
        .arg(&seed.version)
        .arg(PREBUILT_FEATURES)
        .arg(&seed.dest)
        .arg(&log)
        .env("RUSTC_WRAPPER", "")
        .current_dir(worktree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match spawned {
        Ok(_) => crate::log!(
            "[CompileMode] seeding prebuilt OpenSSL {} (log: {})",
            seed.version,
            log.display()
        ),
        Err(e) => crate::log!("[CompileMode] could not start the OpenSSL seed: {e}"),
    }
}

/// The compile-mode variables for an agent build in `worktree`. Pins the mode
/// on first sight, and seeds the prebuilt this lockfile wants.
pub(crate) fn env_for_worktree(
    worktree: &Path,
    cache_root: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let Some(cache_root) = cache_root else {
        return Vec::new();
    };
    if !is_lucidos_source_tree(worktree) {
        return Vec::new();
    }
    let resolution = resolve(worktree, cache_root);
    if let Some(pin) = &resolution.pin_to_write {
        let target = worktree.join("target");
        let written = std::fs::create_dir_all(&target)
            .and_then(|()| std::fs::write(target.join(PIN_FILE), pin));
        if let Err(e) = written {
            crate::log!("[CompileMode] cannot pin {}: {e}", target.display());
        }
    }
    if let Some(seed) = &resolution.seed {
        start_seed(worktree, seed);
    }
    resolution.mode.env()
}

#[cfg(test)]
#[path = "compile_mode_tests.rs"]
mod tests;
