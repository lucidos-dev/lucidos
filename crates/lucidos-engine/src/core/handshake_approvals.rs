//! Which `script_handshake` scripts the engine will run.
//!
//! A handshake script is the one file under workspace `data/` that the engine
//! executes (`api::proxy_script_runner`). Both `data/scripts/` and
//! `data/config/apis.json` are writable over the unauthenticated API. An app UI
//! reaches that API with the user's authority, so the write side cannot decide
//! what may run. This record decides instead: the runner spawns a script only
//! when its workspace-relative path and current SHA-256 are both here
//! (ADR 0144).
//!
//! It lives in `<workspace>/.lucidos/` for the reason `core::grants` gives
//! about permission grants: the file tools resolve nothing under `.lucidos/`
//! except `tmp/`, and a record the caller can rewrite is not a record.
//!
//! **Authorship, never assertion.** Two writers record. The engine's in-process
//! file tools do it when they write under `data/scripts/`, so the author is
//! whoever drove those tools. `lucidos handshake approve` does it through a
//! route no browser can reach. Nothing lets a caller vouch for content it did
//! not write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Engine-owned runtime directory holding the record, beside the permission
/// grants. Gitignored, excluded from backup, unreachable by the file tools.
const RUNTIME_DIR: &str = ".lucidos";

const APPROVALS_FILE: &str = "approved-handshake-scripts";

const HEADER: &str = "\
# Handshake scripts this workspace will run, one per line: <sha256>  <path>.
# Lines starting with '#' are ignored. The path is relative to the workspace root.
# The engine records a script the Lucidos Agent's file tools wrote, and one
# `lucidos handshake approve <path>` names. Editing a script any other way
# changes its hash, and the next proxy call refuses it until it is approved
# again. Deleting a line revokes it. See docs/adr/0144-app-authority-guard-at-use.md.
";

/// Serializes read-modify-write on the record.
///
/// Every writer is in the engine process: the file tools, the startup seed,
/// and the approve route the CLI calls. Two at once would each read the same
/// map and write back their own. One approval would then vanish with no error
/// anywhere, and a script would silently stop running.
///
/// A process lock is the whole answer because nothing outside this process
/// writes the file through these functions. A user editing it by hand is not
/// racing anybody, and the rename below is atomic either way.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Take [`WRITE_LOCK`], recovering a poisoned one.
///
/// A panic while holding it would otherwise wedge every approval for the life
/// of the process. The data is a file re-read under the lock, so a poisoned
/// guard has nothing stale in it to protect against.
fn write_guard() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where `workspace_path` keeps the record.
pub fn approvals_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(RUNTIME_DIR).join(APPROVALS_FILE)
}

/// Why a script was recorded. Carried on `HandshakeScriptApproved` so the
/// timeline distinguishes an authored script from a seeded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSource {
    /// An in-process file tool wrote the file.
    Authored,
    /// `lucidos handshake approve` named it.
    Approved,
    /// Present before this workspace had a record at all. See [`seed_if_absent`].
    Seeded,
}

/// Lowercase hex SHA-256 of `bytes`, the form every line stores.
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::api::hex::hex_lower(&hasher.finalize())
}

/// Every recorded path and its hash. An unreadable file yields nothing, so a
/// read failure refuses scripts rather than running them.
pub fn entries(workspace_path: &Path) -> BTreeMap<String, String> {
    let path = approvals_path(workspace_path);
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse(&contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            crate::log!(
                "[HandshakeApprovals] Failed to read {}: {}. Treating as no approvals.",
                path.display(),
                e
            );
            BTreeMap::new()
        }
    }
}

/// Whether this exact content at this exact path may run.
pub fn is_approved(workspace_path: &Path, rel_path: &str, bytes: &[u8]) -> bool {
    entries(workspace_path)
        .get(rel_path)
        .is_some_and(|recorded| *recorded == content_hash(bytes))
}

/// Record `rel_path` at its current content hash, replacing any earlier hash
/// for that path. Returns whether the file on disk changed.
pub fn record(workspace_path: &Path, rel_path: &str, bytes: &[u8]) -> Result<bool, BoxError> {
    let hash = content_hash(bytes);
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    if current.get(rel_path) == Some(&hash) {
        return Ok(false);
    }
    current.insert(rel_path.to_string(), hash);
    write_all(workspace_path, &current)?;
    Ok(true)
}

/// Drop `rel_path`, so the next run refuses it. Returns whether anything went.
pub fn forget(workspace_path: &Path, rel_path: &str) -> Result<bool, BoxError> {
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    if current.remove(rel_path).is_none() {
        return Ok(false);
    }
    write_all(workspace_path, &current)?;
    Ok(true)
}

/// Record every path in `rel_paths` that exists on disk, but only when this
/// workspace has no record at all. Returns what was recorded.
///
/// The file's own existence is the marker, so this runs once per workspace.
/// Re-seeding on every start would bless a script an attacker planted between
/// two starts, which is the whole thing the record exists to stop.
///
/// The one-time pass is trust on first sight, and deliberate: a workspace whose
/// handshake worked before this ADR must keep working with no user action. A
/// restore from backup drops `.lucidos/`, so a restored workspace seeds again
/// on the same reasoning.
pub fn seed_if_absent(
    workspace_path: &Path,
    rel_paths: &[String],
) -> Result<Vec<String>, BoxError> {
    let _guard = write_guard();
    if approvals_path(workspace_path).exists() {
        return Ok(Vec::new());
    }
    let mut seeded = BTreeMap::new();
    for rel in rel_paths {
        let abs = workspace_path.join(rel);
        match std::fs::read(&abs) {
            Ok(bytes) => {
                seeded.insert(rel.clone(), content_hash(&bytes));
            }
            Err(e) => crate::log!("[HandshakeApprovals] Not seeding {}: {}", abs.display(), e),
        }
    }
    // Written even when nothing was found, so the marker exists and a later
    // start cannot seed a script that appeared in between.
    write_all(workspace_path, &seeded)?;
    Ok(seeded.into_keys().collect())
}

/// `<workspace>`-relative form of `abs`, the key every entry uses. `None` when
/// the path is outside the workspace, which the runner's traversal guard has
/// already refused.
pub fn workspace_relative(workspace_path: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(workspace_path)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

/// One `<sha256>  <path>` line per entry, under the header.
fn parse(contents: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // The hash is the first token; everything after the gap is the path, so
        // a path containing a space still round-trips.
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let path = rest.trim_start();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) || path.is_empty() {
            continue;
        }
        out.insert(path.to_string(), hash.to_ascii_lowercase());
    }
    out
}

/// Overwrite the record atomically, through a sibling temp file and a rename.
/// The runner re-reads per call, so a change takes effect with no restart.
fn write_all(workspace_path: &Path, entries: &BTreeMap<String, String>) -> Result<(), BoxError> {
    let path = approvals_path(workspace_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::from(HEADER);
    for (rel, hash) in entries {
        body.push_str(hash);
        body.push_str("  ");
        body.push_str(rel);
        body.push('\n');
    }
    // Per process, so a second writer cannot land on this one's temp file
    // between its write and its rename. Callers are serialized by
    // [`WRITE_LOCK`], which leaves only another process to collide with.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_script(ws: &Path, rel: &str, body: &str) {
        let abs = ws.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, body).unwrap();
    }

    #[test]
    fn an_unrecorded_script_is_not_approved() {
        let ws = ws();
        assert!(!is_approved(
            ws.path(),
            "data/scripts/auth/x.py",
            b"print(1)"
        ));
    }

    #[test]
    fn recording_then_asking_says_yes() {
        let ws = ws();
        record(ws.path(), "data/scripts/auth/x.py", b"print(1)").unwrap();
        assert!(is_approved(
            ws.path(),
            "data/scripts/auth/x.py",
            b"print(1)"
        ));
    }

    /// The whole point of hashing the content: a recorded path whose bytes then
    /// change is refused, so overwriting a blessed script buys nothing.
    #[test]
    fn changed_content_at_a_recorded_path_is_refused() {
        let ws = ws();
        record(ws.path(), "data/scripts/auth/x.py", b"print(1)").unwrap();
        assert!(!is_approved(
            ws.path(),
            "data/scripts/auth/x.py",
            b"import os; os.system('curl evil')"
        ));
    }

    #[test]
    fn a_recorded_hash_does_not_travel_to_another_path() {
        let ws = ws();
        record(ws.path(), "data/scripts/auth/x.py", b"print(1)").unwrap();
        assert!(!is_approved(
            ws.path(),
            "data/scripts/auth/y.py",
            b"print(1)"
        ));
    }

    #[test]
    fn re_recording_the_same_content_is_a_no_op() {
        let ws = ws();
        assert!(record(ws.path(), "data/scripts/a.py", b"x").unwrap());
        assert!(!record(ws.path(), "data/scripts/a.py", b"x").unwrap());
    }

    #[test]
    fn re_recording_new_content_replaces_the_hash() {
        let ws = ws();
        record(ws.path(), "data/scripts/a.py", b"one").unwrap();
        record(ws.path(), "data/scripts/a.py", b"two").unwrap();
        assert!(!is_approved(ws.path(), "data/scripts/a.py", b"one"));
        assert!(is_approved(ws.path(), "data/scripts/a.py", b"two"));
        assert_eq!(entries(ws.path()).len(), 1, "one line per path");
    }

    #[test]
    fn forgetting_revokes() {
        let ws = ws();
        record(ws.path(), "data/scripts/a.py", b"x").unwrap();
        assert!(forget(ws.path(), "data/scripts/a.py").unwrap());
        assert!(!is_approved(ws.path(), "data/scripts/a.py", b"x"));
        assert!(!forget(ws.path(), "data/scripts/a.py").unwrap());
    }

    #[test]
    fn a_path_with_a_space_round_trips() {
        let ws = ws();
        record(ws.path(), "data/scripts/my auth.py", b"x").unwrap();
        assert!(is_approved(ws.path(), "data/scripts/my auth.py", b"x"));
    }

    #[test]
    fn the_record_lives_outside_data() {
        let ws = ws();
        record(ws.path(), "data/scripts/a.py", b"x").unwrap();
        let path = approvals_path(ws.path());
        assert!(path.starts_with(ws.path().join(".lucidos")));
        assert!(
            !path.starts_with(ws.path().join("data")),
            "an API caller must not be able to write the record"
        );
    }

    #[test]
    fn a_comment_and_a_malformed_line_are_ignored() {
        let ws = ws();
        std::fs::create_dir_all(ws.path().join(".lucidos")).unwrap();
        std::fs::write(
            approvals_path(ws.path()),
            "# a comment\nnot-a-hash  data/scripts/a.py\n\n",
        )
        .unwrap();
        assert!(entries(ws.path()).is_empty());
    }

    #[test]
    fn seeding_records_what_apis_json_already_names() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        let seeded = seed_if_absent(ws.path(), &["data/scripts/auth/live.py".to_string()]).unwrap();
        assert_eq!(seeded, vec!["data/scripts/auth/live.py".to_string()]);
        assert!(is_approved(
            ws.path(),
            "data/scripts/auth/live.py",
            b"print(1)"
        ));
    }

    /// The guard would be worthless if every start blessed whatever `apis.json`
    /// currently names: an attacker would only have to wait for a restart.
    #[test]
    fn seeding_runs_once_and_never_blesses_a_later_script() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(ws.path(), &["data/scripts/auth/live.py".to_string()]).unwrap();

        write_script(ws.path(), "data/scripts/auth/planted.py", "evil");
        let seeded = seed_if_absent(
            ws.path(),
            &[
                "data/scripts/auth/live.py".to_string(),
                "data/scripts/auth/planted.py".to_string(),
            ],
        )
        .unwrap();
        assert!(seeded.is_empty(), "the second pass must record nothing");
        assert!(!is_approved(
            ws.path(),
            "data/scripts/auth/planted.py",
            b"evil"
        ));
    }

    /// A workspace with no handshake at all still gets the marker, or the pass
    /// above would fire on the next start.
    #[test]
    fn seeding_nothing_still_leaves_the_marker() {
        let ws = ws();
        seed_if_absent(ws.path(), &[]).unwrap();
        assert!(approvals_path(ws.path()).exists());

        write_script(ws.path(), "data/scripts/auth/planted.py", "evil");
        let seeded =
            seed_if_absent(ws.path(), &["data/scripts/auth/planted.py".to_string()]).unwrap();
        assert!(seeded.is_empty());
    }

    #[test]
    fn seeding_skips_a_script_that_is_not_on_disk() {
        let ws = ws();
        let seeded = seed_if_absent(ws.path(), &["data/scripts/auth/gone.py".to_string()]).unwrap();
        assert!(seeded.is_empty());
        assert!(approvals_path(ws.path()).exists());
    }

    #[test]
    fn workspace_relative_strips_the_root() {
        let ws = ws();
        let abs = ws.path().join("data/scripts/auth/x.py");
        assert_eq!(
            workspace_relative(ws.path(), &abs).as_deref(),
            Some("data/scripts/auth/x.py")
        );
        assert_eq!(
            workspace_relative(ws.path(), Path::new("/etc/passwd")),
            None
        );
    }
}
