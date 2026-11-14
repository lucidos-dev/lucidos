//! The private subtree: `~/.lucidos/private`, where every secret and control
//! file lives.
//!
//! An agent subprocess runs in an OS sandbox that denies this whole directory
//! (ADR 0645). A secret written here later is covered with no list to update.
//! Shared caches an agent uses, such as `venvs/`, stay outside it.
//!
//! # Homes an older release created
//!
//! Older releases kept these entries directly in the Lucidos home, and older
//! binaries keep running through an upgrade. So nothing moves them and nothing
//! links them. A shipped binary refuses a linked socket directory, and its
//! `network.toml` writer replaces a link with a file.
//!
//! Instead [`entry_in`] resolves each entry to the path an older binary also
//! opens, and the sandbox denies those old paths by name ([`legacy_entries`]).
//! Registry row: "Older homes keep their private entries in place" in
//! `docs/temporary-measures.md`.

use std::io;
use std::path::{Path, PathBuf};

/// The private subtree's name under the Lucidos home.
pub const DIR_NAME: &str = "private";

/// The full-authority local token.
pub const LOCAL_TOKEN: &str = "local-token";

/// The hook socket token: the credential for webhook delivery.
pub const HOOK_SOCKET_TOKEN: &str = "hook-socket-token";

/// The password of the shared dev Postgres cluster.
pub const SHARED_PG_PASSWORD: &str = "pg-shared-password";

/// The machine-global network config.
pub const NETWORK_TOML: &str = "network.toml";

/// The lock every [`NETWORK_TOML`] writer holds across its read-modify-write.
/// It sits beside the file.
pub const NETWORK_TOML_LOCK: &str = "network.toml.lock";

/// The paired-device store's file name. The copy here is the pre-isolation
/// seed every gateway reads once.
pub const PAIRED_DEVICES: &str = "paired-devices.json";

/// The engine sockets and the bundled Postgres socket directories.
pub const RUN_DIR: &str = "run";

/// The workspace key files of the file key store.
pub const SECRET_KEYS_DIR: &str = "secret-keys";

/// Local workspace snapshots.
pub const SNAPSHOTS_DIR: &str = "snapshots";

/// A webhook token an older release minted. Nothing reads it now, but it is
/// still a secret.
const LEGACY_WEBHOOK_TOKEN: &str = "webhook-token";

/// The suffix of a token's mint lock: `.<name>.lock`, beside the token.
pub const MINT_LOCK_SUFFIX: &str = "lock";

/// The mint lock's file name for the token `name`. Never delete one, or two
/// minters would lock two different files.
pub fn mint_lock_name(name: &str) -> String {
    format!(".{name}.{MINT_LOCK_SUFFIX}")
}

/// The tokens that carry a mint lock.
const MINTED: [&str; 3] = [LOCAL_TOKEN, HOOK_SOCKET_TOKEN, SHARED_PG_PASSWORD];

/// Every name an older release kept directly in the Lucidos home, lock files
/// included. A closed list: a new entry goes in the subtree only.
pub fn legacy_entries() -> Vec<String> {
    let mut names: Vec<String> = [
        NETWORK_TOML,
        NETWORK_TOML_LOCK,
        PAIRED_DEVICES,
        LEGACY_WEBHOOK_TOKEN,
        RUN_DIR,
        SECRET_KEYS_DIR,
        SNAPSHOTS_DIR,
        crate::GATEWAY_DIR_NAME,
    ]
    .iter()
    .chain(MINTED.iter())
    .map(|name| name.to_string())
    .collect();
    names.extend(MINTED.iter().map(|name| mint_lock_name(name)));
    names
}

/// The variable that relocates the private subtree. The local e2e harness
/// sets it, so a run never reads this machine's secrets.
pub const DIR_ENV: &str = "LUCIDOS_PRIVATE_DIR";

/// `$LUCIDOS_PRIVATE_DIR`, when set and non-empty.
pub fn relocated() -> Option<PathBuf> {
    relocated_from(std::env::var_os(DIR_ENV))
}

fn relocated_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|dir| !dir.is_empty()).map(PathBuf::from)
}

/// The private subtree under an explicit Lucidos home.
pub fn path_in(lucidos_home: &Path) -> PathBuf {
    lucidos_home.join(DIR_NAME)
}

/// Where the entry `name` lives under `lucidos_home`. Reads the disk, never
/// writes it.
///
/// A name an older release used resolves, first match wins:
/// 1. to the old path, when something is there;
/// 2. to the subtree, when something is there;
/// 3. to the old path, when the home holds any older entry. An older binary
///    creates it there, so both meet one copy;
/// 4. to the subtree, in a home no older release touched.
///
/// Any other name resolves to the subtree.
pub fn entry_in(lucidos_home: &Path, name: &str) -> PathBuf {
    let private = path_in(lucidos_home).join(name);
    if !legacy_entries().iter().any(|legacy| legacy == name) {
        return private;
    }
    let legacy = lucidos_home.join(name);
    if occupied(&legacy) {
        return legacy;
    }
    if occupied(&private) || !holds_legacy_entries(lucidos_home) {
        return private;
    }
    legacy
}

/// Whether an older release left anything in `lucidos_home`.
pub fn holds_legacy_entries(lucidos_home: &Path) -> bool {
    legacy_entries()
        .iter()
        .any(|name| occupied(&lucidos_home.join(name)))
}

/// Something is at `path`, a dangling link included. An `lstat` that fails
/// for another reason is unknown, and counts as occupied: the old path is the
/// one an older binary opens anyway.
fn occupied(path: &Path) -> bool {
    !matches!(
        std::fs::symlink_metadata(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound
    )
}

#[cfg(test)]
#[path = "private_tests.rs"]
mod tests;
