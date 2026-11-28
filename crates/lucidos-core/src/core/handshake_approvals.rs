use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const RUNTIME_DIR: &str = ".lucidos";

const APPROVALS_FILE: &str = "approved-handshake-scripts";

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

/// INVARIANT: Serializes read-modify-write on the record.
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

fn write_guard() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn approvals_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(RUNTIME_DIR).join(APPROVALS_FILE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSource {
    Authored,
    Approved,
    Seeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub hash: String,
    pub base_url: Option<String>,
    pub injects: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEntry {
    pub path: String,
    pub base_url: Option<String>,
    pub injects: Option<BTreeSet<String>>,
}

/// INVARIANT: How the record names the secrets one `script_handshake` entry injects.
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

/// INVARIANT: Whether every member of `injects` survives a round trip through the record.
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
///
/// Three callers, exactly as [`scope_is_recordable`] has. The proxy gates first,
/// for that 502. The seed filters, writing the record directly.
/// [`bind_injects_if_absent`] refuses as a floor under any future caller.
pub fn injects_are_recordable(injects: &BTreeSet<String>) -> bool {
    injects
        .iter()
        .all(|member| !member.contains(',') && !member.contains(char::is_whitespace))
}

pub fn scope_is_recordable(base_url: &str) -> bool {
    !base_url.contains(char::is_whitespace)
}

/// INVARIANT: Whether `rel_path` can be written to the path column and read back whole.
///
/// The third sibling, for the last column, and the same forgery. A path
/// carrying a newline ends its record, and its tail becomes a record of its own
/// with a caller-chosen hash, scope and injects. A later line wins in
/// [`parse`], so the tail can rebind a script already approved.
///
/// An inner space stays legal, as [`parse`] documents. Every control character
/// is refused, since none belongs in a script's name. So is whitespace at
/// either end: [`parse`] trims the line, so the path would read back as its
/// trimmed sibling and carry its hash there.
///
/// [`write_all`] refuses as the floor, and [`parse`] drops such a line. [`record`]
/// and [`seed_if_absent`] check first, and so do the approve route and the
/// runner's `script_path_rejection`.
pub fn path_is_recordable(rel_path: &str) -> bool {
    !rel_path.contains(char::is_control) && rel_path.trim() == rel_path
}

pub const UNRECORDABLE_PATH_REASON: &str =
    "contains a control character or starts or ends with whitespace";

fn unrecordable_path(rel_path: &str) -> BoxError {
    format!("refusing to record {rel_path:?}: it {UNRECORDABLE_PATH_REASON}").into()
}

/// INVARIANT: The record key for an `apis.json` `script` value.
///
/// Config says `scripts/auth/x.py`, the record says `data/scripts/auth/x.py`.
/// The runner derives the same key from the absolute path it runs. Both
/// spellings have to agree, so only this function decides.
pub fn config_path_key(script_rel_path: &str) -> String {
    format!("data/{}", config_path_under_data(script_rel_path))
}

/// INVARIANT: The `data/`-relative remainder of an `apis.json` `script` value.
///
/// **Two spellings name one file.** The documented value is
/// `scripts/auth/x.py`. A config written before resolution moved under `data/`
/// says `data/scripts/auth/x.py`, because the runner joined onto the workspace
/// root then. Both have to resolve, or every such workspace loses its proxies
/// on upgrade.
///
/// **The prefix is stripped once, never in a loop.** `data/data/scripts/x.py`
/// becomes `data/scripts/x.py`, which `script_path_rejection` then refuses. A
/// loop would launder that value into one that runs.
///
/// This narrows nothing. The caller joins the result back onto `data/`, so the
/// file still comes from there and nowhere else. `data/.env` reduces to `.env`,
/// which is still refused.
///
/// The HTTP script runner defines `script_path_rejection` in `lucidos-agents-api`.
pub fn config_path_under_data(script_rel_path: &str) -> &str {
    let rel = script_rel_path.trim_start_matches('/');
    rel.strip_prefix("data/").unwrap_or(rel)
}

pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::core::hex::hex_lower(&hasher.finalize())
}

pub fn entries(workspace_path: &Path) -> BTreeMap<String, Approval> {
    let path = approvals_path(workspace_path);
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse(&contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            crate::log_legacy!(
                "[HandshakeApprovals] Failed to read {}: {}. Treating as no approvals.",
                path.display(),
                e
            );
            BTreeMap::new()
        }
    }
}

pub fn is_approved(workspace_path: &Path, rel_path: &str, bytes: &[u8]) -> bool {
    entries(workspace_path)
        .get(rel_path)
        .is_some_and(|recorded| recorded.hash == content_hash(bytes))
}

pub fn scope_for(workspace_path: &Path, rel_path: &str) -> Option<String> {
    entries(workspace_path).get(rel_path)?.base_url.clone()
}

pub fn injects_for(workspace_path: &Path, rel_path: &str) -> Option<BTreeSet<String>> {
    entries(workspace_path).get(rel_path)?.injects.clone()
}

/// INVARIANT: Record `rel_path` at its current content hash, replacing any earlier hash
/// for that path. Returns whether the file on disk changed.
///
/// An existing scope and injected-secret set both survive. Re-authoring a
/// script changes what it does, not where its token belongs or which secrets it
/// may be handed. Dropping either would hand the next request a fresh
/// trust-on-first-use decision it should not get.
pub fn record(workspace_path: &Path, rel_path: &str, bytes: &[u8]) -> Result<bool, BoxError> {
    if !path_is_recordable(rel_path) {
        return Err(unrecordable_path(rel_path));
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome<T = String> {
    Bound,
    /// INVARIANT: A value was already recorded. The caller checks its own against it,
    /// which is what makes two concurrent first requests safe.
    AlreadyBound(T),
    NotBindable,
}

pub fn bind_scope_if_absent(
    workspace_path: &Path,
    rel_path: &str,
    base_url: &str,
) -> Result<BindOutcome, BoxError> {
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    let Some(approval) = current.get_mut(rel_path) else {
        // INVARIANT: Not recorded at all. The runner refuses it by hash, and saying so is
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
    // INVARIANT: `trim` takes the ends and leaves the middle, so an interior newline
    // survives it and would forge a second record. The caller gates on
    // [`scope_is_recordable`] and answers 502; this is the backstop that keeps
    // any other caller from writing one.
    if !scope_is_recordable(wanted) {
        return Err(format!(
            "refusing to record base_url for '{rel_path}': a scope containing whitespace \
             cannot be written to the approvals record"
        )
        .into());
    }
    if !runs_the_approved_bytes(workspace_path, rel_path, &approval.hash) {
        return Ok(BindOutcome::NotBindable);
    }
    approval.base_url = Some(wanted.to_string());
    write_all(workspace_path, &current)?;
    Ok(BindOutcome::Bound)
}

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
    // INVARIANT: The same forgery the scope column has. A credential name carrying a
    // newline ends this record and starts one of its own, on a hash the caller
    // picked. The proxy gates on [`injects_are_recordable`] and answers 502;
    // this is the backstop that keeps any other caller from writing one.
    if !injects_are_recordable(injects) {
        return Err(format!(
            "refusing to record injects for '{rel_path}': a secret name containing a comma \
             or whitespace cannot be written to the approvals record"
        )
        .into());
    }
    if !runs_the_approved_bytes(workspace_path, rel_path, &approval.hash) {
        return Ok(BindOutcome::NotBindable);
    }
    approval.injects = Some(injects.clone());
    write_all(workspace_path, &current)?;
    Ok(BindOutcome::Bound)
}

/// INVARIANT: Whether the file on disk is still the content this record approved.
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

pub fn forget(workspace_path: &Path, rel_path: &str) -> Result<bool, BoxError> {
    let _guard = write_guard();
    let mut current = entries(workspace_path);
    if current.remove(rel_path).is_none() {
        return Ok(false);
    }
    write_all(workspace_path, &current)?;
    Ok(true)
}

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
        if !path_is_recordable(&entry.path) {
            crate::log_legacy!(
                "[HandshakeApprovals] Not seeding {:?}: {}",
                entry.path,
                unrecordable_path(&entry.path)
            );
            continue;
        }
        let abs = workspace_path.join(&entry.path);
        match std::fs::read(&abs) {
            Ok(bytes) => {
                seeded.insert(
                    entry.path.clone(),
                    Approval {
                        hash: content_hash(&bytes),
                        // INVARIANT: Filtered on the same terms as `injects` below: the
                        // seed writes the record directly, so it cannot lean on
                        // the proxy's gate having run. Trimmed first, and the
                        // trimmed value is what lands, matching what
                        // `bind_scope_if_absent` records for the same config.
                        base_url: entry
                            .base_url
                            .as_deref()
                            .map(str::trim)
                            .filter(|url| !url.is_empty() && scope_is_recordable(url))
                            .map(str::to_string),
                        injects: entry
                            .injects
                            .clone()
                            .filter(|set| !set.is_empty() && injects_are_recordable(set)),
                    },
                );
            }
            Err(e) => {
                crate::log_legacy!("[HandshakeApprovals] Not seeding {}: {}", abs.display(), e)
            }
        }
    }
    // INVARIANT: Written even when nothing was found, so the marker exists and a later
    // start cannot seed a script that appeared in between.
    write_all(workspace_path, &seeded)?;
    Ok(seeded.into_keys().collect())
}

pub fn workspace_relative(workspace_path: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(workspace_path)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

/// INVARIANT: One `<sha256>  <base_url>  <injects>  <path>` line per entry, under the
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
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let (base_url, rest) = split_scope(rest.trim_start());
        let (injects, path) = split_injects(rest);
        // INVARIANT: A path [`write_all`] would refuse is dropped here too, so a hand edit
        // cannot plant a key that wedges every later write.
        if hash.len() != 64
            || !hash.chars().all(|c| c.is_ascii_hexdigit())
            || path.is_empty()
            || !path_is_recordable(path)
        {
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

fn is_injects_column(head: &str) -> bool {
    !head.is_empty()
        && head
            .split(',')
            .all(|member| member.starts_with("c:") || member.starts_with("o:"))
}

fn write_all(workspace_path: &Path, entries: &BTreeMap<String, Approval>) -> Result<(), BoxError> {
    if let Some(rel) = entries.keys().find(|rel| !path_is_recordable(rel)) {
        return Err(unrecordable_path(rel));
    }
    let path = approvals_path(workspace_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::from(HEADER);
    for (rel, approval) in entries {
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
    // INVARIANT: Per process, so a second writer cannot land on this one's temp file
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

    #[test]
    fn binding_an_unrecorded_script_records_nothing() {
        let ws = ws();
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/ghost.py", "https://x.test").unwrap(),
            BindOutcome::NotBindable
        );
        assert!(entries(ws.path()).is_empty());
    }

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

        write_script(ws.path(), "data/scripts/a.py", "approved");
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/a.py", "https://real.test").unwrap(),
            BindOutcome::Bound
        );
    }

    #[test]
    fn a_missing_script_cannot_bind_a_scope() {
        let ws = ws();
        record(ws.path(), "data/scripts/gone.py", b"x").unwrap();
        assert_eq!(
            bind_scope_if_absent(ws.path(), "data/scripts/gone.py", "https://x.test").unwrap(),
            BindOutcome::NotBindable
        );
    }

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

    #[test]
    fn a_credential_and_an_oauth_provider_never_collide() {
        assert_eq!(
            injected_secrets(Some("oauth:google"), ["google"]),
            secrets(&["c:oauth:google", "o:google"])
        );
        assert!(injected_secrets(None, std::iter::empty::<&str>()).is_empty());
    }

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

    #[test]
    fn a_scope_that_would_re_cut_the_line_is_not_recordable() {
        assert!(scope_is_recordable("https://api.example.com"));
        assert!(scope_is_recordable("https://api.example.com/v1/path?q=1"));
        assert!(!scope_is_recordable("https://ok.test/ https://evil.test"));
        assert!(!scope_is_recordable("https://ok.test/\nhttps://evil.test"));
        assert!(!scope_is_recordable("https://ok.test/\thttps://evil.test"));
    }

    #[test]
    fn a_newline_in_the_scope_cannot_forge_an_approval_line() {
        let ws = ws();
        let script = "data/scripts/auth/legit.py";
        write_script(ws.path(), script, "print(1)");
        record(ws.path(), script, b"print(1)").unwrap();

        let forged_hash = "a".repeat(64);
        let hostile = format!("https://ok.test/\n{forged_hash}  https://evil.test");

        let outcome = bind_scope_if_absent(ws.path(), script, &hostile);
        assert!(outcome.is_err(), "a scope with a newline must be refused");

        let after = entries(ws.path());
        assert_eq!(after.len(), 1, "no second record was forged");
        assert!(after.contains_key(script));
        assert_eq!(scope_for(ws.path(), script), None);

        assert!(
            !is_approved(ws.path(), script, b"whatever the attacker wrote"),
            "the runner must not accept bytes the forged line would have named"
        );
    }

    #[test]
    fn a_newline_in_an_injected_name_cannot_forge_an_approval_line() {
        let ws = ws();
        let script = "data/scripts/auth/legit.py";
        write_script(ws.path(), script, "print(1)");
        record(ws.path(), script, b"print(1)").unwrap();

        let forged_hash = "a".repeat(64);
        let forged = format!("c:ok\n{forged_hash}  -  -  data/scripts/auth/evil.py");
        let hostile = secrets(&[forged.as_str()]);

        let outcome = bind_injects_if_absent(ws.path(), script, &hostile);
        assert!(
            outcome.is_err(),
            "a secret name with a newline must be refused, got {outcome:?}"
        );

        let after = entries(ws.path());
        assert_eq!(after.len(), 1, "no second record was forged");
        assert!(
            after.contains_key(script),
            "the authored script must still be the recorded path, got {:?}",
            after.keys().collect::<Vec<_>>()
        );
        assert_eq!(injects_for(ws.path(), script), None);
    }

    #[test]
    fn a_path_that_would_re_cut_the_line_is_not_recordable() {
        assert!(path_is_recordable("data/scripts/auth/x.py"));
        assert!(path_is_recordable("data/scripts/auth/my script.py"));
        assert!(!path_is_recordable("data/scripts/auth/x.py\nmore"));
        assert!(!path_is_recordable("data/scripts/auth/x.py\rmore"));
        assert!(!path_is_recordable("data/scripts/auth/x.py\u{85}more"));
        assert!(!path_is_recordable("data/scripts/auth/\tx.py"));
        assert!(!path_is_recordable("data/scripts/auth/x.py "));
        assert!(!path_is_recordable("data/scripts/auth/x.py\u{3000}"));
        assert!(!path_is_recordable(" data/scripts/auth/x.py"));
    }

    #[test]
    fn a_trailing_space_cannot_move_a_hash_onto_an_approved_path() {
        let ws = ws();
        let victim = "data/scripts/auth/legit.py";
        write_script(ws.path(), victim, "print(1)");
        record(ws.path(), victim, b"print(1)").unwrap();

        assert!(record(ws.path(), &format!("{victim} "), b"print(2)").is_err());

        assert_eq!(entries(ws.path()).len(), 1);
        assert!(is_approved(ws.path(), victim, b"print(1)"));
        assert!(!is_approved(ws.path(), victim, b"print(2)"));
    }

    fn path_forging_a_line_for(victim: &str) -> String {
        let forged_hash = "a".repeat(64);
        format!("data/scripts/auth/x.py\n{forged_hash}  https://evil.test  -  {victim}")
    }

    #[test]
    fn a_newline_in_the_path_cannot_forge_an_approval_line() {
        let ws = ws();
        let victim = "data/scripts/auth/legit.py";
        write_script(ws.path(), victim, "print(1)");
        record(ws.path(), victim, b"print(1)").unwrap();

        let outcome = record(ws.path(), &path_forging_a_line_for(victim), b"print(2)");
        assert!(outcome.is_err(), "a path with a newline must be refused");

        let after = entries(ws.path());
        assert_eq!(after.len(), 1, "no second record was forged");
        assert_eq!(scope_for(ws.path(), victim), None);
        assert!(is_approved(ws.path(), victim, b"print(1)"));
    }

    #[test]
    fn the_seed_skips_a_path_that_would_forge_an_approval_line() {
        let ws = ws();
        let victim = "data/scripts/auth/legit.py";
        let hostile = path_forging_a_line_for(victim);
        write_script(ws.path(), victim, "print(1)");
        write_script(ws.path(), &hostile, "print(2)");

        let seeded = seed_if_absent(ws.path(), &unscoped(&[victim, hostile.as_str()])).unwrap();

        assert_eq!(seeded, vec![victim.to_string()]);
        let after = entries(ws.path());
        assert_eq!(after.len(), 1, "no second record was forged");
        assert_eq!(scope_for(ws.path(), victim), None);
    }

    #[test]
    fn write_all_refuses_a_path_that_would_forge_an_approval_line() {
        let ws = ws();
        let victim = "data/scripts/auth/legit.py";
        write_script(ws.path(), victim, "print(1)");
        record(ws.path(), victim, b"print(1)").unwrap();

        let mut current = entries(ws.path());
        current.insert(
            path_forging_a_line_for(victim),
            Approval {
                hash: content_hash(b"print(2)"),
                base_url: None,
                injects: None,
            },
        );

        assert!(write_all(ws.path(), &current).is_err());
        assert_eq!(entries(ws.path()).len(), 1, "the record was not rewritten");
    }

    #[test]
    fn a_hand_edited_path_with_a_control_character_is_not_read() {
        let hash = "a".repeat(64);
        let parsed = parse(&format!(
            "{hash}  -  -  data/scripts/auth/x\u{7}.py\n{hash}  -  -  data/scripts/auth/y.py\n"
        ));
        assert_eq!(
            parsed.keys().collect::<Vec<_>>(),
            vec!["data/scripts/auth/y.py"]
        );
    }

    #[test]
    fn a_padded_scope_still_binds_and_records_trimmed() {
        let ws = ws();
        let script = "data/scripts/auth/legit.py";
        write_script(ws.path(), script, "print(1)");
        record(ws.path(), script, b"print(1)").unwrap();

        let outcome = bind_scope_if_absent(ws.path(), script, "  https://api.example.com  ");
        assert!(matches!(outcome, Ok(BindOutcome::Bound)), "got {outcome:?}");
        assert_eq!(
            scope_for(ws.path(), script),
            Some("https://api.example.com".to_string())
        );
    }

    #[test]
    fn seeding_a_padded_scope_records_it_trimmed() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(
            ws.path(),
            &[SeedEntry {
                path: "data/scripts/auth/live.py".to_string(),
                base_url: Some("  https://api.example.com  ".to_string()),
                injects: None,
            }],
        )
        .unwrap();

        assert_eq!(
            scope_for(ws.path(), "data/scripts/auth/live.py"),
            Some("https://api.example.com".to_string())
        );
    }

    #[test]
    fn seeding_skips_a_scope_it_could_not_read_back() {
        let ws = ws();
        write_script(ws.path(), "data/scripts/auth/live.py", "print(1)");
        seed_if_absent(
            ws.path(),
            &[SeedEntry {
                path: "data/scripts/auth/live.py".to_string(),
                base_url: Some("https://ok.test/\nforged  https://evil.test".to_string()),
                injects: None,
            }],
        )
        .unwrap();

        assert_eq!(
            scope_for(ws.path(), "data/scripts/auth/live.py"),
            None,
            "an unrecordable scope is dropped, never written"
        );

        let after = entries(ws.path());
        assert_eq!(after.len(), 1, "no second record was forged");
        assert!(
            after.contains_key("data/scripts/auth/live.py"),
            "the seeded script must still be the recorded path, got {:?}",
            after.keys().collect::<Vec<_>>()
        );
    }

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

    #[test]
    fn the_data_prefixed_spelling_keys_the_same_record() {
        assert_eq!(
            config_path_key("data/scripts/auth/x.py"),
            config_path_key("scripts/auth/x.py")
        );
    }

    #[test]
    fn a_doubled_prefix_is_stripped_only_once() {
        assert_eq!(
            config_path_under_data("data/data/scripts/auth/x.py"),
            "data/scripts/auth/x.py"
        );
    }

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
