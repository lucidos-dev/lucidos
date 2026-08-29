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
//!
//! **A record also says where the token may go.** A handshake script mints its
//! own token, so no stored credential speaks for it and ADR 0144 decision 4 has
//! nothing to check. The `base_url` column is that scope. It is filled by the
//! boot seed, or bound on first use, and enforced from then on. Rewriting
//! `apis.json` therefore cannot redirect a minted token to another host.
//!
//! **And which secrets the script may be handed.** A `script_handshake` entry
//! names a stored credential and a list of OAuth providers, and the engine
//! injects both into the script's environment. Neither travels to the entry's
//! `base_url`, so the credential scope gate has nothing true to say about them.
//! The `injects` column is the guard instead. It binds the same way the scope
//! does, so an `apis.json` rewrite cannot swap one secret for another.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Engine-owned runtime directory holding the record, beside the permission
/// grants. Gitignored, excluded from backup, unreachable by the file tools.
const RUNTIME_DIR: &str = ".lucidos";

const APPROVALS_FILE: &str = "approved-handshake-scripts";

/// Stands in for an unbound column, so every line has four fields.
const UNBOUND: &str = "-";

const HEADER: &str = "\
# Handshake scripts this workspace will run, one per line:
#   <sha256>  <base_url>  <injects>  <path>
# Lines starting with '#' are ignored. The path is relative to the workspace root.
# The engine records a script the Lucidos Agent's file tools wrote, and one
# `lucidos handshake approve <path>` names. Editing a script any other way
# changes its hash, and the next proxy call refuses it until it is approved
# again. Deleting a line revokes it.
#
# base_url is the ONE upstream this script's token may be sent to. '-' means it
# is not bound yet, and the next proxy call binds it to whatever apis.json then
# says. Edit the column here to move a script to another host, or to let a
# second provider share it.
#
# injects is the set of secrets apis.json may hand this script, comma separated:
# 'c:<credential>' for a stored credential, 'o:<provider>' for a connected OAuth
# account. '-' means it is not bound yet. Once bound, an apis.json entry asking
# for a different set is refused. See docs/adr/0144-app-authority-guard-at-use.md.
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

/// One recorded script: the exact content the engine will run, the one upstream
/// the token it mints may be sent to, and the secrets it may be handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// Lowercase hex SHA-256 of the approved bytes.
    pub hash: String,
    /// Where this script's token may go. `None` until the boot seed or the
    /// first proxy call binds it.
    pub base_url: Option<String>,
    /// Which secrets an `apis.json` entry may inject, as [`injected_secrets`]
    /// spells them. `None` until the boot seed or the first proxy call binds it.
    pub injects: Option<BTreeSet<String>>,
}

/// What the boot seed records for one script, read off `apis.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEntry {
    /// The record key, as [`config_path_key`] spells it.
    pub path: String,
    /// The one upstream every entry naming this script uses. `None` when they
    /// disagree, which is not an answer, so it binds on first use instead.
    pub base_url: Option<String>,
    /// The secrets every entry naming this script injects, on the same terms.
    pub injects: Option<BTreeSet<String>>,
}

/// How the record names the secrets one `script_handshake` entry injects.
///
/// `c:` for a stored credential and `o:` for an OAuth provider, so a credential
/// named `oauth:google` stays distinct from the `google` provider. Neither
/// prefix can be confused with a path, which always starts with `data/`.
///
/// An entry injecting nothing yields an empty set, and an empty set is never
/// recorded. There is nothing to guard when no secret moves, so "bound to
/// nothing" would be a state with no meaning.
pub fn injected_secrets<'a>(
    credential: Option<&str>,
    oauth_providers: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<String> {
    credential
        .map(|name| format!("c:{name}"))
        .into_iter()
        .chain(
            oauth_providers
                .into_iter()
                .map(|provider| format!("o:{provider}")),
        )
        .collect()
}

/// Whether every member of `injects` survives a round trip through the record.
///
/// The column is comma joined inside a whitespace-split line, and a credential
/// name is free text with no validation. A name carrying a comma or any
/// whitespace would silently re-cut the line. The parser would read part of it
/// as the set and the rest as the path. An approved script would then stop
/// running, and a line would appear for a path nobody wrote.
///
/// Refused rather than escaped, deliberately. The record is documented as hand
/// editable, and escapes would make it less so. An escape also has to round
/// trip exactly across every Unicode whitespace character, which is a second
/// thing to get wrong. The caller answers 502 naming the fix: rename the
/// credential.
pub fn injects_are_recordable(injects: &BTreeSet<String>) -> bool {
    injects
        .iter()
        .all(|member| !member.contains(',') && !member.contains(char::is_whitespace))
}

/// The record key for an `apis.json` `script` value.
///
/// Config says `scripts/auth/x.py`, the record says `data/scripts/auth/x.py`.
/// The runner derives the same key from the absolute path it runs. Both
/// spellings have to agree, so only this function decides.
pub fn config_path_key(script_rel_path: &str) -> String {
    format!("data/{}", config_path_under_data(script_rel_path))
}

/// The `data/`-relative remainder of an `apis.json` `script` value.
///
/// **Two spellings name one file.** The documented value is
/// `scripts/auth/x.py`. A config written before resolution moved under `data/`
/// says `data/scripts/auth/x.py`, because the runner joined onto the workspace
/// root then. Both have to resolve, or every such workspace loses its proxies
/// on upgrade.
///
/// **The prefix is stripped once, never in a loop.** `data/data/scripts/x.py`
/// becomes `data/scripts/x.py`, which [`script_path_rejection`] then refuses. A
/// loop would launder that value into one that runs.
///
/// This narrows nothing. The caller joins the result back onto `data/`, so the
/// file still comes from there and nowhere else. `data/.env` reduces to `.env`,
/// which is still refused.
///
/// [`script_path_rejection`]: crate::api::proxy_script_runner::script_path_rejection
pub fn config_path_under_data(script_rel_path: &str) -> &str {
    let rel = script_rel_path.trim_start_matches('/');
    rel.strip_prefix("data/").unwrap_or(rel)
}

/// Lowercase hex SHA-256 of `bytes`, the form every line stores.
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::api::hex::hex_lower(&hasher.finalize())
}

/// Every recorded path and what was recorded for it. An unreadable file yields
/// nothing, so a read failure refuses scripts rather than running them.
pub fn entries(workspace_path: &Path) -> BTreeMap<String, Approval> {
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
        .is_some_and(|recorded| recorded.hash == content_hash(bytes))
}

/// The upstream this script's minted token is bound to, if one is recorded.
pub fn scope_for(workspace_path: &Path, rel_path: &str) -> Option<String> {
    entries(workspace_path).get(rel_path)?.base_url.clone()
}

/// The secrets this script may be handed, if a set is recorded.
pub fn injects_for(workspace_path: &Path, rel_path: &str) -> Option<BTreeSet<String>> {
    entries(workspace_path).get(rel_path)?.injects.clone()
}

/// Record `rel_path` at its current content hash, replacing any earlier hash
/// for that path. Returns whether the file on disk changed.
///
/// An existing scope and injected-secret set both survive. Re-authoring a
/// script changes what it does, not where its token belongs or which secrets it
/// may be handed. Dropping either would hand the next request a fresh
/// trust-on-first-use decision it should not get.
pub fn record(workspace_path: &Path, rel_path: &str, bytes: &[u8]) -> Result<bool, BoxError> {
    let hash = content_hash(bytes);
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    let existing = current.get(rel_path);
    if existing.map(|a| &a.hash) == Some(&hash) {
        return Ok(false);
    }
    let base_url = existing.and_then(|a| a.base_url.clone());
    let injects = existing.and_then(|a| a.injects.clone());
    current.insert(
        rel_path.to_string(),
        Approval {
            hash,
            base_url,
            injects,
        },
    );
    write_all(workspace_path, &current)?;
    Ok(true)
}

/// What [`bind_scope_if_absent`] or [`bind_injects_if_absent`] found. `T` is
/// whatever that column holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome<T = String> {
    /// This call recorded the value.
    Bound,
    /// A value was already recorded. The caller checks its own against it,
    /// which is what makes two concurrent first requests safe.
    AlreadyBound(T),
    /// Nothing to bind: no record for this path, or the bytes on disk are not
    /// the approved ones. The runner's hash check is what answers either way.
    NotBindable,
}

/// Bind `rel_path`'s token to `base_url`, but only while it has no scope.
///
/// Trust on first sight, exactly as [`CredentialStore::infer_scope_if_empty`]
/// does for a credential that predates ADR 0144. A script authored now has no
/// `apis.json` entry yet, so authorship has nothing to record. The first
/// request that would use it decides, and every later one is checked.
///
/// [`CredentialStore::infer_scope_if_empty`]: crate::core::CredentialStore::infer_scope_if_empty
pub fn bind_scope_if_absent(
    workspace_path: &Path,
    rel_path: &str,
    base_url: &str,
) -> Result<BindOutcome, BoxError> {
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    let Some(approval) = current.get_mut(rel_path) else {
        // Not recorded at all. The runner refuses it by hash, and saying so is
        // its job: inventing a line here would approve a script nobody wrote.
        return Ok(BindOutcome::NotBindable);
    };
    if let Some(existing) = &approval.base_url {
        return Ok(BindOutcome::AlreadyBound(existing.clone()));
    }
    let wanted = base_url.trim();
    if wanted.is_empty() {
        return Ok(BindOutcome::NotBindable);
    }
    if !runs_the_approved_bytes(workspace_path, rel_path, &approval.hash) {
        return Ok(BindOutcome::NotBindable);
    }
    approval.base_url = Some(wanted.to_string());
    write_all(workspace_path, &current)?;
    Ok(BindOutcome::Bound)
}

/// Bind which secrets `rel_path` may be handed, but only while it has no set.
///
/// The sibling of [`bind_scope_if_absent`], and the same trade. A script
/// authored now has no `apis.json` entry, so authorship has nothing to read.
/// The first request that would inject a secret decides, and every later one is
/// checked against it.
///
/// An empty `injects` records nothing and reports `NotBindable`. No secret
/// moves, so there is nothing for a record to protect.
pub fn bind_injects_if_absent(
    workspace_path: &Path,
    rel_path: &str,
    injects: &BTreeSet<String>,
) -> Result<BindOutcome<BTreeSet<String>>, BoxError> {
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    let Some(approval) = current.get_mut(rel_path) else {
        return Ok(BindOutcome::NotBindable);
    };
    if let Some(existing) = &approval.injects {
        return Ok(BindOutcome::AlreadyBound(existing.clone()));
    }
    if injects.is_empty() {
        return Ok(BindOutcome::NotBindable);
    }
    if !runs_the_approved_bytes(workspace_path, rel_path, &approval.hash) {
        return Ok(BindOutcome::NotBindable);
    }
    approval.injects = Some(injects.clone());
    write_all(workspace_path, &current)?;
    Ok(BindOutcome::Bound)
}

/// Whether the file on disk is still the content this record approved.
///
/// Bind only the bytes the engine would actually run. Otherwise a caller swaps
/// the script out, binds their own host through the refused call, then restores
/// the approved bytes and collects the token. `false` on a read failure is the
/// fail-closed side: no bind.
fn runs_the_approved_bytes(workspace_path: &Path, rel_path: &str, hash: &str) -> bool {
    std::fs::read(workspace_path.join(rel_path))
        .map(|bytes| content_hash(&bytes) == hash)
        .unwrap_or(false)
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

/// Record every path in `scripts` that exists on disk, each with the scope its
/// `apis.json` entry names. Runs only when this workspace has no record at all,
/// and returns what it recorded.
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
    scripts: &[SeedEntry],
) -> Result<Vec<String>, BoxError> {
    let _guard = write_guard();
    if approvals_path(workspace_path).exists() {
        return Ok(Vec::new());
    }
    let mut seeded = BTreeMap::new();
    for entry in scripts {
        let abs = workspace_path.join(&entry.path);
        match std::fs::read(&abs) {
            Ok(bytes) => {
                seeded.insert(
                    entry.path.clone(),
                    Approval {
                        hash: content_hash(&bytes),
                        base_url: entry.base_url.clone(),
                        injects: entry
                            .injects
                            .clone()
                            .filter(|set| !set.is_empty() && injects_are_recordable(set)),
                    },
                );
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

/// One `<sha256>  <base_url>  <injects>  <path>` line per entry, under the
/// header.
///
/// **Every earlier shape still parses**, so a record from before either column
/// existed still runs its scripts. The path is what tells the columns apart.
/// Every key [`config_path_key`] produces starts with `data/`. A scope is `-`
/// or a URL. An injects set is `-` or `c:` / `o:` members. Neither can look
/// like a path, so a two, three or four token line reads unambiguously, and the
/// path may still contain spaces.
fn parse(contents: &str) -> BTreeMap<String, Approval> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // The hash is the first token; everything after the gap is the two
        // columns and the path, so a path containing a space still round-trips.
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let (base_url, rest) = split_scope(rest.trim_start());
        let (injects, path) = split_injects(rest);
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) || path.is_empty() {
            continue;
        }
        out.insert(
            path.to_string(),
            Approval {
                hash: hash.to_ascii_lowercase(),
                base_url,
                injects,
            },
        );
    }
    out
}

/// Split the scope off the front of what follows the hash.
fn split_scope(rest: &str) -> (Option<String>, &str) {
    let Some((head, tail)) = rest.split_once(char::is_whitespace) else {
        return (None, rest);
    };
    if head == UNBOUND {
        return (None, tail.trim_start());
    }
    if head.contains("://") {
        return (Some(head.to_string()), tail.trim_start());
    }
    (None, rest)
}

/// Split the injected-secret set off the front of what follows the scope.
fn split_injects(rest: &str) -> (Option<BTreeSet<String>>, &str) {
    let Some((head, tail)) = rest.split_once(char::is_whitespace) else {
        return (None, rest);
    };
    if head == UNBOUND {
        return (None, tail.trim_start());
    }
    if is_injects_column(head) {
        let set = head.split(',').map(str::to_string).collect();
        return (Some(set), tail.trim_start());
    }
    (None, rest)
}

/// Whether `head` is the injects column rather than the first token of a path.
///
/// Every member carries the `c:` or `o:` prefix [`injected_secrets`] writes, and
/// every path starts with `data/`, so the two shapes cannot collide.
fn is_injects_column(head: &str) -> bool {
    !head.is_empty()
        && head
            .split(',')
            .all(|member| member.starts_with("c:") || member.starts_with("o:"))
}

/// Overwrite the record atomically, through a sibling temp file and a rename.
/// The runner re-reads per call, so a change takes effect with no restart.
fn write_all(workspace_path: &Path, entries: &BTreeMap<String, Approval>) -> Result<(), BoxError> {
    let path = approvals_path(workspace_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::from(HEADER);
    for (rel, approval) in entries {
        // An empty set writes as unbound. Nothing is injected, so there is
        // nothing for the next call to be checked against.
        let injects = match &approval.injects {
            Some(set) if !set.is_empty() => set.iter().cloned().collect::<Vec<_>>().join(","),
            _ => UNBOUND.to_string(),
        };
        body.push_str(&approval.hash);
        body.push_str("  ");
        body.push_str(approval.base_url.as_deref().unwrap_or(UNBOUND));
        body.push_str("  ");
        body.push_str(&injects);
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

    /// A seed list naming scripts no `apis.json` entry scopes.
    fn unscoped(paths: &[&str]) -> Vec<SeedEntry> {
        paths
            .iter()
            .map(|p| SeedEntry {
                path: p.to_string(),
                base_url: None,
                injects: None,
            })
            .collect()
    }

    fn secrets(members: &[&str]) -> BTreeSet<String> {
        members.iter().map(|m| m.to_string()).collect()
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
        let seeded = seed_if_absent(ws.path(), &unscoped(&["data/scripts/auth/live.py"])).unwrap();
        assert_eq!(seeded, vec!["data/scripts/auth/live.py".to_string()]);
        assert!(is_approved(
            ws.path(),
            "data/scripts/auth/live.py",
            b"print(1)"
        ));
    }

    /// A workspace upgrading gets its scope from the entry that already names
    /// the script, so a later `apis.json` rewrite cannot redirect its token.
    #[test]
    fn seeding_records_the_scope_apis_json_names() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(
            ws.path(),
            &[SeedEntry {
                path: "data/scripts/auth/live.py".to_string(),
                base_url: Some("https://api.example.test".to_string()),
                injects: None,
            }],
        )
        .unwrap();
        assert_eq!(
            scope_for(ws.path(), "data/scripts/auth/live.py").as_deref(),
            Some("https://api.example.test")
        );
    }

    /// The guard would be worthless if every start blessed whatever `apis.json`
    /// currently names: an attacker would only have to wait for a restart.
    #[test]
    fn seeding_runs_once_and_never_blesses_a_later_script() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(ws.path(), &unscoped(&["data/scripts/auth/live.py"])).unwrap();

        write_script(ws.path(), "data/scripts/auth/planted.py", "evil");
        let seeded = seed_if_absent(
            ws.path(),
            &unscoped(&["data/scripts/auth/live.py", "data/scripts/auth/planted.py"]),
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
            seed_if_absent(ws.path(), &unscoped(&["data/scripts/auth/planted.py"])).unwrap();
        assert!(seeded.is_empty());
    }

    #[test]
    fn seeding_skips_a_script_that_is_not_on_disk() {
        let ws = ws();
        let seeded = seed_if_absent(ws.path(), &unscoped(&["data/scripts/auth/gone.py"])).unwrap();
        assert!(seeded.is_empty());
        assert!(approvals_path(ws.path()).exists());
    }

    /// Trust on first sight, once. What binds a script is the first request
    /// that would use it, and every later host is checked against that.
    #[test]
    fn a_scope_binds_once_and_never_moves() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "x");
        record(ws.path(), "data/scripts/a.py", b"x").unwrap();
        assert!(scope_for(ws.path(), "data/scripts/a.py").is_none());

        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://first.test").unwrap(),
            BindOutcome::Bound
        );
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://second.test").unwrap(),
            BindOutcome::AlreadyBound("https://first.test".to_string()),
            "a bound script is never rebound, and the caller is told what holds"
        );
        assert_eq!(
            scope_for(ws.path(), "data/scripts/a.py").as_deref(),
            Some("https://first.test")
        );
    }

    /// Binding never invents an approval. An unrecorded script has to be
    /// refused by the runner, which is the half that reads the bytes.
    #[test]
    fn binding_an_unrecorded_script_records_nothing() {
        let ws = ws();
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/ghost.py", "https://x.test").unwrap(),
            BindOutcome::NotBindable
        );
        assert!(entries(ws.path()).is_empty());
    }

    /// A script whose bytes no longer match its approval binds nothing. Left
    /// open, a caller swaps the file out, binds their host through the call the
    /// runner then refuses, and restores the approved bytes to collect.
    #[test]
    fn a_script_that_would_not_run_cannot_bind_a_scope() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "approved");
        record(ws.path(), "data/scripts/a.py", b"approved").unwrap();
        write_script(ws.path(), "data/scripts/a.py", "swapped");

        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://evil.test").unwrap(),
            BindOutcome::NotBindable
        );
        assert!(scope_for(ws.path(), "data/scripts/a.py").is_none());

        // Restoring the approved bytes leaves it unbound, so the next
        // legitimate call is still the one that decides.
        write_script(ws.path(), "data/scripts/a.py", "approved");
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://real.test").unwrap(),
            BindOutcome::Bound
        );
    }

    /// A recorded script whose file is gone binds nothing either.
    #[test]
    fn a_missing_script_cannot_bind_a_scope() {
        let ws = ws();
        record(ws.path(), "data/scripts/gone.py", b"x").unwrap();
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/gone.py", "https://x.test").unwrap(),
            BindOutcome::NotBindable
        );
    }

    /// Re-authoring a script changes what it does, not where its token belongs.
    #[test]
    fn re_recording_keeps_the_scope() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "one");
        record(ws.path(), "data/scripts/a.py", b"one").unwrap();
        bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://first.test").unwrap();
        record(ws.path(), "data/scripts/a.py", b"two").unwrap();
        assert!(is_approved(ws.path(), "data/scripts/a.py", b"two"));
        assert_eq!(
            scope_for(ws.path(), "data/scripts/a.py").as_deref(),
            Some("https://first.test")
        );
    }

    /// A credential and an OAuth provider are told apart by their prefix, so a
    /// credential literally named `oauth:google` cannot pass for the `google`
    /// provider. That name is real: it is the legacy spelling of an OAuth
    /// client registration.
    #[test]
    fn a_credential_and_an_oauth_provider_never_collide() {
        assert_eq!(
            injected_secrets(Some("oauth:google"), ["google"]),
            secrets(&["c:oauth:google", "o:google"])
        );
        assert!(injected_secrets(None, std::iter::empty::<&str>()).is_empty());
    }

    /// The swap this column exists to stop. `apis.json` is writable over the
    /// API, so an entry naming an approved script can be rewritten to ask for a
    /// different secret. The record decides, not the entry.
    #[test]
    fn an_injected_set_binds_once_and_never_moves() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "x");
        record(ws.path(), "data/scripts/a.py", b"x").unwrap();
        assert!(injects_for(ws.path(), "data/scripts/a.py").is_none());

        let first = secrets(&["c:comfort-cloud"]);
        assert_eq!(
            bind_injects_if_absent(ws.path(), "data/scripts/a.py", &first).unwrap(),
            BindOutcome::Bound
        );
        assert_eq!(
            bind_injects_if_absent(ws.path(), "data/scripts/a.py", &secrets(&["c:openrouter"]))
                .unwrap(),
            BindOutcome::AlreadyBound(first.clone()),
            "a bound script is never rebound, and the caller is told what holds"
        );
        assert_eq!(injects_for(ws.path(), "data/scripts/a.py"), Some(first));
    }

    /// A credential name is free text, and the column is comma joined inside a
    /// whitespace-split line. A name carrying either would re-cut the line, so
    /// the set is refused before anything is written.
    #[test]
    fn a_name_that_would_re_cut_the_line_is_not_recordable() {
        assert!(injects_are_recordable(&secrets(&[
            "c:comfort-cloud",
            "o:google"
        ])));
        assert!(injects_are_recordable(&BTreeSet::new()));
        assert!(!injects_are_recordable(&secrets(&["c:my key"])));
        assert!(!injects_are_recordable(&secrets(&["c:a,b"])));
        assert!(!injects_are_recordable(&secrets(&["c:two\nlines"])));
    }

    /// The seed writes the record directly, so it filters the same way rather
    /// than trusting the gate to have run first.
    #[test]
    fn seeding_skips_an_injected_set_it_could_not_read_back() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(
            ws.path(),
            &[SeedEntry {
                path: "data/scripts/auth/live.py".to_string(),
                base_url: Some("https://api.example.test".to_string()),
                injects: Some(secrets(&["c:my key"])),
            }],
        )
        .unwrap();
        assert!(is_approved(
            ws.path(),
            "data/scripts/auth/live.py",
            b"print(1)"
        ));
        assert!(
            injects_for(ws.path(), "data/scripts/auth/live.py").is_none(),
            "an unwritable set seeds as unbound, and the gate refuses by name"
        );
    }

    /// Injecting nothing is not a state. There is no secret to protect, so
    /// there is nothing to record and nothing to check later.
    #[test]
    fn an_empty_set_records_nothing() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "x");
        record(ws.path(), "data/scripts/a.py", b"x").unwrap();
        assert_eq!(
            bind_injects_if_absent(ws.path(), "data/scripts/a.py", &BTreeSet::new()).unwrap(),
            BindOutcome::NotBindable
        );
        assert!(injects_for(ws.path(), "data/scripts/a.py").is_none());
    }

    /// The same guard the scope has. A caller must not swap the script out,
    /// bind a secret through the call the runner refuses, then restore the
    /// approved bytes and collect.
    #[test]
    fn a_script_that_would_not_run_cannot_bind_an_injected_set() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "approved");
        record(ws.path(), "data/scripts/a.py", b"approved").unwrap();
        write_script(ws.path(), "data/scripts/a.py", "swapped");
        assert_eq!(
            bind_injects_if_absent(ws.path(), "data/scripts/a.py", &secrets(&["c:x"])).unwrap(),
            BindOutcome::NotBindable
        );
        assert!(injects_for(ws.path(), "data/scripts/a.py").is_none());
    }

    /// Re-authoring changes what a script does, not what it may be handed.
    #[test]
    fn re_recording_keeps_the_injected_set() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/a.py", "one");
        record(ws.path(), "data/scripts/a.py", b"one").unwrap();
        let bound = secrets(&["c:k", "o:google"]);
        bind_injects_if_absent(ws.path(), "data/scripts/a.py", &bound).unwrap();
        record(ws.path(), "data/scripts/a.py", b"two").unwrap();
        assert!(is_approved(ws.path(), "data/scripts/a.py", b"two"));
        assert_eq!(injects_for(ws.path(), "data/scripts/a.py"), Some(bound));
    }

    /// A workspace upgrading gets its set from the entry that already names the
    /// script, so a later `apis.json` rewrite cannot swap what it receives.
    #[test]
    fn seeding_records_the_injected_set_apis_json_names() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        let bound = secrets(&["c:firebase-key", "o:google"]);
        seed_if_absent(
            ws.path(),
            &[SeedEntry {
                path: "data/scripts/auth/live.py".to_string(),
                base_url: Some("https://api.example.test".to_string()),
                injects: Some(bound.clone()),
            }],
        )
        .unwrap();
        assert_eq!(
            injects_for(ws.path(), "data/scripts/auth/live.py"),
            Some(bound)
        );
    }

    /// Both columns and a spaced path in one line, round-tripped. The path is
    /// what tells the columns apart, so the spaces have to survive.
    #[test]
    fn a_scope_an_injected_set_and_a_spaced_path_round_trip() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/my auth.py", "x");
        record(ws.path(), "data/scripts/my auth.py", b"x").unwrap();
        bind_scope_if_absent(ws.path(), "data/scripts/my auth.py", "https://api.test/v1").unwrap();
        let bound = secrets(&["c:k", "o:google"]);
        bind_injects_if_absent(ws.path(), "data/scripts/my auth.py", &bound).unwrap();
        assert!(is_approved(ws.path(), "data/scripts/my auth.py", b"x"));
        assert_eq!(
            scope_for(ws.path(), "data/scripts/my auth.py").as_deref(),
            Some("https://api.test/v1")
        );
        assert_eq!(
            injects_for(ws.path(), "data/scripts/my auth.py"),
            Some(bound)
        );
    }

    /// Every shape a live workspace may hold, in one file. A packaged install's
    /// record is two-column, and one written since the scope landed is three.
    /// Both must keep running their scripts after this upgrade.
    #[test]
    fn two_three_and_four_column_lines_parse_side_by_side() {
        let ws = ws();
        let hash = content_hash(b"print(1)");
        std::fs::create_dir_all(ws.path().join(".lucidos")).unwrap();
        std::fs::write(
            approvals_path(ws.path()),
            format!(
                "{hash}  data/scripts/auth/two.py\n\
                 {hash}  https://a.test  data/scripts/auth/three.py\n\
                 {hash}  -  data/scripts/auth/three-unbound.py\n\
                 {hash}  https://b.test  c:k,o:google  data/scripts/auth/four.py\n\
                 {hash}  -  -  data/scripts/auth/four-unbound.py\n"
            ),
        )
        .unwrap();
        let recorded = entries(ws.path());
        assert_eq!(recorded.len(), 5, "every line must parse: {recorded:?}");
        for path in recorded.keys() {
            assert!(is_approved(ws.path(), path, b"print(1)"), "{path}");
        }
        assert_eq!(
            scope_for(ws.path(), "data/scripts/auth/three.py").as_deref(),
            Some("https://a.test")
        );
        assert!(injects_for(ws.path(), "data/scripts/auth/three.py").is_none());
        assert_eq!(
            injects_for(ws.path(), "data/scripts/auth/four.py"),
            Some(secrets(&["c:k", "o:google"]))
        );
        assert!(injects_for(ws.path(), "data/scripts/auth/four-unbound.py").is_none());
    }

    /// A record written before the scope column existed still runs its scripts.
    #[test]
    fn a_two_column_line_still_parses_with_no_scope() {
        let ws = ws();
        let hash = content_hash(b"print(1)");
        std::fs::create_dir_all(ws.path().join(".lucidos")).unwrap();
        std::fs::write(
            approvals_path(ws.path()),
            format!("{hash}  data/scripts/auth/my auth.py\n"),
        )
        .unwrap();
        assert!(is_approved(
            ws.path(),
            "data/scripts/auth/my auth.py",
            b"print(1)"
        ));
        assert!(scope_for(ws.path(), "data/scripts/auth/my auth.py").is_none());
    }

    /// A scope round-trips through a write, and so does a spaced path beside
    /// it: the scope is the middle token and carries a scheme, the path is
    /// everything after it.
    #[test]
    fn a_scope_and_a_spaced_path_round_trip() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/my auth.py", "x");
        record(ws.path(), "data/scripts/my auth.py", b"x").unwrap();
        bind_scope_if_absent(ws.path(), "data/scripts/my auth.py", "https://api.test/v1").unwrap();
        assert!(is_approved(ws.path(), "data/scripts/my auth.py", b"x"));
        assert_eq!(
            scope_for(ws.path(), "data/scripts/my auth.py").as_deref(),
            Some("https://api.test/v1")
        );
    }

    /// One spelling for the config value and the record key, or the seed would
    /// bless a path the runner never looks up.
    #[test]
    fn a_config_script_value_keys_the_same_record_the_runner_reads() {
        assert_eq!(
            config_path_key("scripts/auth/x.py"),
            "data/scripts/auth/x.py"
        );
        assert_eq!(
            config_path_key("/scripts/auth/x.py"),
            "data/scripts/auth/x.py"
        );
    }

    /// The spelling a config written before `data/`-relative resolution carries.
    /// It names the same file, so it has to key the same line. Keying it
    /// `data/data/...` seeded nothing and left a live workspace with no
    /// handshake at all.
    #[test]
    fn the_data_prefixed_spelling_keys_the_same_record() {
        assert_eq!(
            config_path_key("data/scripts/auth/x.py"),
            config_path_key("scripts/auth/x.py")
        );
    }

    /// Stripped once, so a hand-doubled prefix stays visible for the runner's
    /// guard to refuse rather than being laundered into a runnable path.
    #[test]
    fn a_doubled_prefix_is_stripped_only_once() {
        assert_eq!(
            config_path_under_data("data/data/scripts/auth/x.py"),
            "data/scripts/auth/x.py"
        );
    }

    /// The prefix comes off on a segment boundary. A sibling directory whose
    /// name merely starts with `data` keeps every character.
    #[test]
    fn a_directory_named_like_data_keeps_its_prefix() {
        assert_eq!(
            config_path_under_data("database/scripts/x.py"),
            "database/scripts/x.py"
        );
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
