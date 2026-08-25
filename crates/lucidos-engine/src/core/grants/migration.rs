//! One-time seed of each workspace's permission grants from the machine-global
//! files they moved out of (ADR 0095).
//!
//! The split this implements is upgrade versus creation. An upgrade preserves
//! what the user already has, because taking a grant away is a change they did
//! not ask for and cannot see coming. A workspace created afterwards starts
//! empty, because nobody has decided anything about it yet.
//!
//! Two records keep that true. A machine-wide one in the user dir says the seed
//! has run, so a workspace created tomorrow is never visited. A per-workspace
//! stamp says this workspace was seeded, so a re-run cannot resurrect a grant
//! the user has since deleted.
//!
//! The originals are copied, never moved. A migration that deletes the only
//! copy of a permission set is not worth the disk it saves.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{BoxError, GrantFile};

/// Machine-wide record that the seed has run, written in the user dir beside
/// the global files it is about. A readable record rather than a bare
/// sentinel, so "why does my new workspace not have Slack?" has an answer on
/// disk.
const MACHINE_RECORD: &str = "grants-migrated-to-workspaces";

/// Per-workspace stamp, claimed before anything is copied.
const WORKSPACE_STAMP: &str = "grants-seeded";

/// Seed every workspace that exists now, unless that has already happened.
///
/// Best-effort and quiet on the happy path. Every failure logs and leaves the
/// machine record unwritten, so the next boot tries again.
///
/// Skipped under `e2e-test-hooks`, the same gate push delivery uses to keep a
/// real-world side effect out of the suite. An e2e engine boots with the
/// developer's own `HOME`. Unguarded, it would seed their live workspaces from
/// their personal grants and burn the one-time record, from a test.
pub fn run(user_dir: Option<&Path>, workspace_path: &Path) {
    if cfg!(feature = "e2e-test-hooks") {
        return;
    }
    let Some(user_dir) = user_dir else {
        // No user dir means no global files to inherit from.
        return;
    };
    seed_from(user_dir, &gateway_data_dir(user_dir), workspace_path);
}

/// The gateway's base dir, holding the workspace registry. Mirrors
/// `resolve_app_data` in `lucidos-gateway/src/server.rs`, which the engine
/// cannot call: ADR 0014 §1 keeps the two crates unlinked.
fn gateway_data_dir(user_dir: &Path) -> PathBuf {
    std::env::var_os("LUCIDOS_GATEWAY_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| user_dir.join("gateway"))
}

/// The registry document, cut down to the one field this needs. Mirrors
/// `Registry` in `lucidos-gateway/src/registry.rs`; a relative `dir` resolves
/// against the gateway data dir, an absolute one is used as it stands.
#[derive(serde::Deserialize)]
struct RegistryDoc {
    #[serde(default)]
    workspaces: Vec<RegistryEntry>,
}

#[derive(serde::Deserialize)]
struct RegistryEntry {
    dir: String,
}

/// Every registered workspace directory.
///
/// An absent registry is `Ok(empty)`, the fresh-install and no-gateway case. A
/// present but unparseable one is an error, never an empty list: reading it as
/// "no workspaces" would write the machine record and close the door on an
/// install whose grants were never copied.
fn registered_workspace_dirs(gateway_data: &Path) -> Result<Vec<PathBuf>, BoxError> {
    let path = gateway_data.join("config/workspaces.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display()).into()),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let doc: RegistryDoc =
        serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(doc
        .workspaces
        .into_iter()
        .map(|w| {
            let dir = PathBuf::from(w.dir);
            if dir.is_absolute() {
                dir
            } else {
                gateway_data.join(dir)
            }
        })
        .collect())
}

fn seed_from(user_dir: &Path, gateway_data: &Path, workspace_path: &Path) {
    let record = user_dir.join(MACHINE_RECORD);
    if record.exists() {
        return;
    }

    let registered = match registered_workspace_dirs(gateway_data) {
        Ok(dirs) => dirs,
        Err(e) => {
            crate::log!(
                "[Grants] Workspace registry unreadable ({e}). \
                 Seeding nothing and retrying on the next boot."
            );
            return;
        }
    };

    // The engine's own workspace is unioned in rather than assumed present. It
    // exists by definition, and an engine can run with no gateway.
    let mut targets: BTreeSet<PathBuf> = registered.iter().map(|p| resolve(p)).collect();
    targets.insert(resolve(workspace_path));

    let mut seeded: Vec<PathBuf> = Vec::new();
    for workspace in &targets {
        if !workspace.is_dir() {
            // A registered directory that is gone. Creating it here would
            // resurrect a workspace the user deleted.
            crate::log!("[Grants] Skipping {}: not on disk", workspace.display());
            continue;
        }
        match seed_workspace(user_dir, workspace) {
            Ok(true) => seeded.push(workspace.clone()),
            Ok(false) => {}
            // The stamp is already claimed, and the machine record below closes
            // the door for good, so this workspace inherits nothing. Say so:
            // the alternative is retrying, and a retry is what re-adds a grant
            // the user deleted.
            Err(e) => crate::log!(
                "[Grants] Seeding {} failed ({e}). It inherits no grants and asks on first use.",
                workspace.display()
            ),
        }
    }

    if let Err(e) = write_machine_record(&record, user_dir, &seeded) {
        crate::log!("[Grants] Could not write {}: {e}", record.display());
        return;
    }
    crate::log!(
        "[Grants] Permission grants are now per workspace. Seeded {} of {} workspace(s).",
        seeded.len(),
        targets.len()
    );
}

/// The path a set can compare. Two registry spellings of one workspace must not
/// seed it twice, and a path that cannot be canonicalized is used as it stands.
fn resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Copy the global grants into one workspace. `Ok(false)` means it was already
/// stamped and nothing was touched.
///
/// The stamp is claimed BEFORE anything is copied, which decides what a crash
/// mid-copy costs. This way the workspace is left empty and the user re-approves
/// once. The other order would re-copy on the next boot, and re-copying is how a
/// grant the user deleted comes back.
fn seed_workspace(user_dir: &Path, workspace: &Path) -> Result<bool, BoxError> {
    let dir = super::grants_dir(workspace);
    std::fs::create_dir_all(&dir)?;

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(WORKSPACE_STAMP))
    {
        Ok(mut f) => write!(f, "{}", stamp_body(user_dir))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e.into()),
    }

    for file in GrantFile::ALL {
        let Some(body) = inheritable_global(user_dir, file) else {
            continue;
        };
        super::write_raw(
            &dir,
            file,
            &format!("{}{}", origin_header(user_dir, file), body),
        )?;
    }
    Ok(true)
}

/// The global file's contents, when it holds at least one grant worth
/// inheriting. Absent, unreadable and grant-free all read as nothing to copy,
/// which is the ordinary fresh-install case rather than a failure.
fn inheritable_global(user_dir: &Path, file: GrantFile) -> Option<String> {
    let path = user_dir.join(file.file_name());
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::log!("[Grants] Cannot read {}: {e}", path.display());
            return None;
        }
    };
    if super::parse_patterns(&body).is_empty() {
        return None;
    }
    Some(body)
}

/// Prepended to a seeded file, so a later reader knows the grants were
/// inherited rather than chosen here.
fn origin_header(user_dir: &Path, file: GrantFile) -> String {
    format!(
        "# Inherited on {date} from {source}, when grants became per workspace.\n\
         # Nothing here is shared with another workspace. Delete a line to drop that grant.\n",
        date = today(),
        source = user_dir.join(file.file_name()).display(),
    )
}

fn stamp_body(user_dir: &Path) -> String {
    format!(
        "# This workspace was seeded on {date} from the grant files in {source}.\n\
         # Delete this file only to make the seed run again here.\n",
        date = today(),
        source = user_dir.display(),
    )
}

fn write_machine_record(
    record: &Path,
    user_dir: &Path,
    seeded: &[PathBuf],
) -> Result<(), BoxError> {
    let mut body = format!(
        "# Permission grants became per workspace on {date} (ADR 0095).\n\
         # The originals stay in {source}, unread. A later release removes them.\n\
         # A workspace created after this date starts with no grants, and asks on first use.\n\
         # Seeded {count} workspace(s):\n",
        date = today(),
        source = user_dir.display(),
        count = seeded.len(),
    );
    for workspace in seeded {
        body.push_str(&format!("{}\n", workspace.display()));
    }
    if let Some(parent) = record.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(record, body)?;
    Ok(())
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
#[path = "migration_tests.rs"]
mod tests;
