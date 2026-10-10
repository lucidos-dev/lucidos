//! Three-way merge of a plugin update against the user's local edits.
//!
//! An update overwrites every file the plugin ships. When the user has edited
//! one of those files, the three inputs a merge needs are all in hand at
//! staging time: **base** is the file at the install commit, **ours** is what
//! is on disk now, and **theirs** is the staged new version.
//!
//! What counts as "modified" is not decided here. This consumes
//! [`registry::modification_status_for`], the same function behind the Plugins
//! list's Modified badge, so the two cannot drift.
//!
//! The plan is computed in full before the writer touches a byte. That is what
//! lets the panel state a per-file outcome, and lets a failure leave nothing
//! half-written.

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::plugins::PlannedFile;
use crate::core::DATA_DIR;

use super::registry::{modification_status_for, PluginBaseline};

/// Above this, on any side, a file is replaced rather than merged. A plugin
/// ships prose, markup and small scripts. Anything at this scale was not
/// hand-edited, and holding three copies of it for the staging TTL is not worth
/// the merge.
const MAX_MERGE_BYTES: usize = 1024 * 1024;

/// What the confirm will do with one shipped path the user has locally edited.
/// A path the user has NOT edited never becomes a [`LocalChange`] at all, and
/// neither does one whose copy already equals the staged version. Both are
/// overwritten exactly as they always were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalChangeOutcome {
    /// Upstream's edits and the user's combined cleanly. The merged content
    /// goes to disk and the user keeps their patch.
    Merged,
    /// Both sides changed the same region. Upstream wins on disk and the user's
    /// version is saved aside. These files are LLM context, so conflict markers
    /// would become an instruction the engine acts on.
    Conflict,
    /// Locally edited but not mergeable: a trigger projection, a binary, or an
    /// oversized file. Upstream wins on disk and the user's version is saved
    /// aside, so `Replaced` always implies a saved copy exists.
    Replaced,
    /// The user had DELETED the file, and upstream still ships it, so it comes
    /// back. Its own outcome rather than a `Replaced`, because there is no
    /// content to save aside and saying otherwise would be a false promise.
    Restored,
}

impl LocalChangeOutcome {
    /// Wire value for the staged preview and the confirm response.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Conflict => "conflict",
            Self::Replaced => "replaced",
            Self::Restored => "restored",
        }
    }

    /// Whether confirming discards content that must be copied aside first.
    /// `Merged` keeps the edit in the file, and `Restored` has no content to
    /// keep: the user deleted it.
    fn discards_local_work(self) -> bool {
        matches!(self, Self::Conflict | Self::Replaced)
    }
}

/// One shipped path the user edited, and what the update will do with it.
///
/// A plan sits in the pending-install map for up to an hour, so it holds file
/// content only where it must. The merged bytes and the patch are needed at
/// confirm time and are bounded by [`MAX_MERGE_BYTES`]. The user's own copy is
/// NOT retained: saving it aside copies it off disk, which the drift check has
/// just proved unchanged. Retaining it would mean holding an arbitrarily large
/// modified binary in memory for the whole staging window.
pub(crate) struct LocalChange {
    pub(crate) data_relative: String,
    pub(crate) outcome: LocalChangeOutcome,
    /// The merged bytes, for [`LocalChangeOutcome::Merged`] only.
    merged: Option<Vec<u8>>,
    /// A unified diff from the install commit to the user's copy, saving a
    /// discarded edit as something re-appliable rather than a whole file alone.
    /// Generated for every change, since the user may still switch the panel's
    /// keep control off and discard one that would have merged. `None` for
    /// content no diff can express, such as a binary or an oversized file.
    patch: Option<String>,
    /// The user's copy as it hashed when the plan was made, streamed off disk.
    /// Re-read at confirm so an edit landing inside the staging window cannot
    /// be silently clobbered. `None` when they had deleted the file.
    ours_oid: Option<git2::Oid>,
}

/// Every locally-edited path an update would overwrite. Empty for a fresh
/// install, for a plugin nobody has touched, and for a legacy record with no
/// baseline commit to diff against.
#[derive(Default)]
pub(crate) struct MergePlan {
    pub(crate) changes: Vec<LocalChange>,
    /// Edited paths whose copy already equalled the staged version, with the
    /// hash that made them equal. No decision, so the panel never lists them
    /// and the confirm never resolves them. Still watched for drift: the user
    /// edits these files, and one edited inside the staging window is real
    /// work the confirm would otherwise overwrite unseen.
    unchanged: Vec<(String, git2::Oid)>,
}

impl MergePlan {
    /// The `local_changes` block of the staged preview: what the panel lists
    /// before the user confirms.
    pub(crate) fn preview_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.changes
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "path": c.data_relative,
                        "outcome": c.outcome.as_str(),
                    })
                })
                .collect(),
        )
    }

    /// Re-read each edited file and compare it to the blob recorded at staging
    /// time. A mismatch means the file changed after the panel was reviewed.
    /// The confirm then refuses, rather than writing a merge computed against
    /// content that is gone. Returns the first drifted path.
    ///
    /// Over every edited path, listed or not. A path dropped for already
    /// matching upstream is silent in the panel, never in here.
    pub(crate) fn detect_drift(&self, workspace_path: &Path) -> Option<String> {
        let data_dir = workspace_path.join(DATA_DIR);
        self.changes
            .iter()
            .map(|c| (&c.data_relative, c.ours_oid))
            .chain(self.unchanged.iter().map(|(path, oid)| (path, Some(*oid))))
            .find_map(|(path, staged)| {
                (hash_on_disk(&data_dir.join(path)) != staged).then(|| path.clone())
            })
    }

    /// Resolve the plan against the user's keep-or-replace choice.
    /// `keep_local_changes == false` collapses every outcome to `Replaced`, so
    /// the update lands clean upstream and every edit is saved aside.
    pub(crate) fn resolve(&self, keep_local_changes: bool) -> Vec<ResolvedChange<'_>> {
        self.changes
            .iter()
            .map(|c| ResolvedChange {
                change: c,
                // A restore is untouched by the keep control: the user
                // deleted the file, so there is no edit to keep or drop.
                outcome: match (keep_local_changes, c.outcome) {
                    (true, outcome) => outcome,
                    (false, LocalChangeOutcome::Restored) => LocalChangeOutcome::Restored,
                    (false, _) => LocalChangeOutcome::Replaced,
                },
            })
            .collect()
    }
}

/// A [`LocalChange`] with the panel's keep control applied.
pub(crate) struct ResolvedChange<'a> {
    change: &'a LocalChange,
    pub(crate) outcome: LocalChangeOutcome,
}

impl ResolvedChange<'_> {
    pub(crate) fn data_relative(&self) -> &str {
        &self.change.data_relative
    }

    /// Merged bytes to write instead of upstream's, when this change survives
    /// as a merge.
    pub(crate) fn merged_bytes(&self) -> Option<&[u8]> {
        (self.outcome == LocalChangeOutcome::Merged)
            .then_some(self.change.merged.as_deref())
            .flatten()
    }

    /// The patch to file beside the saved copy, when confirming discards this
    /// edit. `Some(None)` means save the copy but write no patch, which is the
    /// binary and oversized case.
    pub(crate) fn to_save_aside(&self) -> Option<Option<&str>> {
        // Nothing on disk to copy: the user had deleted the file.
        if !self.outcome.discards_local_work() || self.change.ours_oid.is_none() {
            return None;
        }
        Some(self.change.patch.as_deref())
    }
}

/// Where a discarded local edit is kept: under `data/artifacts/`, which is
/// git-tracked and never auto-deleted. Not `.lucidos/`, which is rebuildable
/// scratch and the wrong home for the only copy of someone's work.
pub(crate) const SAVED_CHANGES_ROOT: &str = "artifacts/plugin-local-changes";

/// Write every local edit this confirm is about to discard, before anything is
/// overwritten. Each one is saved twice: the whole file, and a patch against
/// the version it was written on. Returns the `data/`-relative paths written,
/// for the caller to commit and announce.
///
/// A conflict, an unmergeable file and a switched-off keep control all land
/// here. A clean merge does not: the edit survives in the file itself, so a
/// copy would be clutter.
pub(crate) fn save_discarded(
    workspace_path: &Path,
    plugin_id: &str,
    version: &str,
    resolved: &[ResolvedChange<'_>],
) -> Result<Vec<String>, String> {
    let data_dir = workspace_path.join(DATA_DIR);
    let dir = free_save_dir(&data_dir, plugin_id, version);
    let mut written: Vec<String> = Vec::new();
    for change in resolved {
        let Some(patch) = change.to_save_aside() else {
            continue;
        };
        let rel = change.data_relative();
        if crate::core::is_path_traversal(rel) {
            continue;
        }
        // Copied off disk rather than from memory. The caller has just run the
        // drift check, so the live file is what the plan described. An
        // arbitrarily large one then never sits in the pending entry.
        let copy = format!("{}/{}", dir, rel);
        super::copy_atomic(&data_dir.join(rel), &data_dir.join(&copy))?;
        written.push(copy);
        if let Some(text) = patch {
            let patch_path = format!("{}/{}.patch", dir, rel);
            super::write_atomic(text.as_bytes(), &data_dir.join(&patch_path))?;
            written.push(patch_path);
        }
    }
    if !written.is_empty() {
        let readme = format!("{}/README.md", dir);
        super::write_atomic(
            saved_changes_readme(plugin_id, version, &dir).as_bytes(),
            &data_dir.join(&readme),
        )?;
        written.push(readme);
    }
    Ok(written)
}

/// A directory for this save that does not already hold one.
///
/// The same version can be installed more than once: `install_plugin` on the
/// recorded source reinstalls whatever is there. A fixed `v<version>` folder
/// would then write the second batch of discarded edits over the first. That
/// copy is the only place the first batch still exists in the working tree,
/// since the confirm has just overwritten the live files.
///
/// The first save of a version gets the plain name, so the common path stays
/// predictable and matches what the README tells the user. A later one is
/// suffixed, so nothing is ever replaced.
fn free_save_dir(data_dir: &Path, plugin_id: &str, version: &str) -> String {
    let base = format!("{}/{}/v{}", SAVED_CHANGES_ROOT, plugin_id, version);
    if !data_dir.join(&base).exists() {
        return base;
    }
    // Unbounded on purpose: it lands on `-2` in practice, and any cap would
    // need a fallback that overwrites, which is the thing being prevented.
    (2u32..)
        .map(|n| format!("{}-{}", base, n))
        .find(|candidate| !data_dir.join(candidate).exists())
        .unwrap_or(base)
}

/// The note that sits beside a saved-aside edit, so the folder explains itself
/// months later without the user having to remember this feature exists.
fn saved_changes_readme(plugin_id: &str, version: &str, dir: &str) -> String {
    format!(
        "# Local changes to {plugin_id}, kept from the update to v{version}\n\
         \n\
         Updating the {plugin_id} plugin could not keep these edits, so Lucidos \
         saved them here instead of losing them.\n\
         \n\
         Each file appears twice. One copy is your version exactly as it stood. \
         The matching `.patch` is the same edit as a diff against the version \
         you had installed.\n\
         \n\
         To re-apply one by hand, from the workspace root:\n\
         \n\
         ```\n\
         git apply data/{dir}/<path>.patch\n\
         ```\n\
         \n\
         If the new version moved that code too far for the patch to land, \
         copy the edit across from the whole-file copy instead.\n\
         \n\
         Nothing here is read by the plugin. Delete the folder once you are \
         done with it.\n",
        plugin_id = plugin_id,
        version = version,
        dir = dir,
    )
}

/// The user's current local patch for a plugin: one unified diff covering every
/// shipped path they have edited, from the install commit to the working tree.
/// `None` when they have edited nothing a diff can express.
///
/// Derived on read, like the Modified badge, and for the same reason. An update
/// need save nothing for this to work later, because the install commit is kept
/// a pristine copy of the shipped version. So the diff is against the version
/// they are actually on, which is what makes it worth offering upstream.
pub(crate) fn local_patch(workspace_path: &Path, baseline: &PluginBaseline) -> Option<String> {
    let status = modification_status_for(workspace_path, &baseline.commit, &baseline.files);
    if !status.modified {
        return None;
    }
    let repo = git2::Repository::open(workspace_path).ok()?;
    let tree = repo
        .revparse_single(&baseline.commit)
        .and_then(|obj| obj.peel_to_commit())
        .and_then(|c| c.tree())
        .ok()?;
    let data_dir = workspace_path.join(DATA_DIR);

    let mut out = String::new();
    for rel in &status.modified_paths {
        // A trigger definition is engine-generated, so its diff describes the
        // serializer rather than anything the user wrote.
        if super::plugin_trigger_slug(rel).is_some() {
            continue;
        }
        let Some(ours) = read_mergeable(&data_dir.join(rel)) else {
            continue;
        };
        if !is_mergeable_text(&ours) {
            continue;
        }
        let base = tree
            .get_path(Path::new(&format!("data/{}", rel)))
            .ok()
            .and_then(|entry| repo.find_blob(entry.id()).ok())
            .map(|blob| blob.content().to_vec());
        if base.as_deref().is_some_and(|b| !is_mergeable_text(b)) {
            continue;
        }
        if let Some(diff) = unified_diff(rel, base.as_deref(), &ours) {
            out.push_str(&diff);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Classify every path the update would overwrite, merging the ones the user
/// has edited. `baseline` is the version they are on; `planned` is what the
/// staged version ships.
///
/// Two questions per path, not one. "Did this change since the install
/// commit?" makes it a candidate; "is ours already theirs?" drops it again. A
/// user who published their own edit upstream meets the second on every file
/// they sent.
///
/// Best-effort by construction: an unopenable repo, an unresolvable commit or
/// an unreadable file all yield an empty plan, which is exactly today's
/// behaviour (replace everything). A merge improves on overwriting. It is never
/// a reason to fail an install.
pub(crate) fn plan_local_changes(
    workspace_path: &Path,
    baseline: &PluginBaseline,
    planned: &[PlannedFile],
) -> MergePlan {
    let status = modification_status_for(workspace_path, &baseline.commit, &baseline.files);
    if !status.modified {
        return MergePlan::default();
    }
    let modified: BTreeSet<&str> = status.modified_paths.iter().map(String::as_str).collect();

    let Ok(repo) = git2::Repository::open(workspace_path) else {
        return MergePlan::default();
    };
    let Ok(tree) = repo
        .revparse_single(&baseline.commit)
        .and_then(|obj| obj.peel_to_commit())
        .and_then(|c| c.tree())
    else {
        return MergePlan::default();
    };

    let data_dir = workspace_path.join(DATA_DIR);
    let mut changes = Vec::new();
    let mut unchanged = Vec::new();
    for pf in planned {
        if !modified.contains(pf.data_relative.as_str()) {
            continue;
        }
        let live = data_dir.join(&pf.data_relative);
        let ours_oid = hash_on_disk(&live);
        // The user's copy already IS what the new version ships, which is what
        // publishing your own edit upstream leaves behind. The merge writes
        // these bytes whichever way it goes, so there is nothing to decide. A
        // row for it inflates the count and hands the keep control a file it
        // cannot affect.
        //
        // Compared by content hash, never by the buffers below. `read_mergeable`
        // yields `None` for every oversized file, so two DIFFERENT ones would
        // read as equal. That would drop a real edit in silence.
        //
        // It keeps its drift key. The panel going quiet about a path must not
        // stop the confirm seeing an edit that lands after it.
        if let Some(ours) = ours_oid {
            if Some(ours) == hash_on_disk(&pf.source) {
                unchanged.push((pf.data_relative.clone(), ours));
                continue;
            }
        }
        let base = tree
            .get_path(Path::new(&format!("data/{}", pf.data_relative)))
            .ok()
            .and_then(|entry| repo.find_blob(entry.id()).ok())
            .map(|blob| blob.content().to_vec());
        let ours = read_mergeable(&live);
        let theirs = read_mergeable(&pf.source);

        let (outcome, merged) = classify(
            &repo,
            pf,
            base.as_deref(),
            ours.as_deref(),
            theirs.as_deref(),
            ours_oid.is_some(),
        );
        changes.push(LocalChange {
            data_relative: pf.data_relative.clone(),
            outcome,
            merged,
            patch: ours
                .as_deref()
                .filter(|o| diffable(o, base.as_deref()))
                .and_then(|o| unified_diff(&pf.data_relative, base.as_deref(), o)),
            ours_oid,
        });
    }
    MergePlan { changes, unchanged }
}

/// Decide one path's outcome, and produce the merged bytes when it merges.
fn classify(
    repo: &git2::Repository,
    pf: &PlannedFile,
    base: Option<&[u8]>,
    ours: Option<&[u8]>,
    theirs: Option<&[u8]>,
    live_exists: bool,
) -> (LocalChangeOutcome, Option<Vec<u8>>) {
    // A trigger definition is a re-serialized projection the engine rewrites
    // after every install (ADR 0019), so its bytes never match the shipped
    // authored bytes. Text-merging it would report a conflict for every plugin
    // trigger on every update. `resync_plugin_triggers` regenerates the file
    // from the event anyway, so a merged one would not survive the install.
    if super::plugin_trigger_slug(&pf.data_relative).is_some() {
        return (LocalChangeOutcome::Replaced, None);
    }
    // Gone from disk: the user deleted it and upstream still ships it, so it
    // comes back. Told apart from the unreadable and oversized cases below,
    // because there is no content to save aside for a deletion.
    if !live_exists {
        return (LocalChangeOutcome::Restored, None);
    }
    // Present, but no side to merge: unreadable, or too big to hold
    // (`read_mergeable` refuses those without reading them). Upstream's copy
    // lands, which is what an update has always done.
    let (Some(ours), Some(theirs)) = (ours, theirs) else {
        return (LocalChangeOutcome::Replaced, None);
    };
    if [Some(ours), Some(theirs), base]
        .iter()
        .flatten()
        .any(|side| !is_mergeable_text(side))
    {
        return (LocalChangeOutcome::Replaced, None);
    }

    // libgit2 merges blobs, so each side is written to the object database
    // first. An abandoned staging leaves those unreferenced, which is ordinary
    // loose-object garbage that `git gc` reclaims.
    let empty: &[u8] = b"";
    let Ok(ids) = [base.unwrap_or(empty), ours, theirs]
        .iter()
        .map(|side| repo.blob(side))
        .collect::<Result<Vec<_>, _>>()
    else {
        return (LocalChangeOutcome::Replaced, None);
    };

    let entries: Vec<git2::IndexEntry> = ids
        .iter()
        .map(|id| crate::core::blob_index_entry(&pf.data_relative, *id, 0o100644))
        .collect();
    let mut opts = git2::MergeFileOptions::new();
    opts.style_standard(true);
    match repo.merge_file_from_index(&entries[0], &entries[1], &entries[2], Some(&mut opts)) {
        Ok(result) if result.is_automergeable() => {
            (LocalChangeOutcome::Merged, Some(result.content().to_vec()))
        }
        Ok(_) => (LocalChangeOutcome::Conflict, None),
        // A merge libgit2 refused to attempt is not one to guess at.
        Err(e) => {
            log!(@Plugins, "merge {} failed, replacing instead: {}", pf.data_relative, e);
            (LocalChangeOutcome::Replaced, None)
        }
    }
}

/// Text small enough to merge. A NUL byte is git's own binary heuristic, and a
/// binary three-way merge is meaningless whatever the size.
fn is_mergeable_text(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_MERGE_BYTES && !bytes.contains(&0)
}

/// Whether a unified diff between these two sides would say anything useful.
/// Both must be text: a diff of binary content prints "Binary files differ",
/// which cannot be re-applied and only pads the saved-aside folder.
fn diffable(ours: &[u8], base: Option<&[u8]>) -> bool {
    is_mergeable_text(ours) && base.is_none_or(is_mergeable_text)
}

/// Read a file only when it is small enough to be worth merging.
///
/// The size is checked by `metadata` first, so an oversized file is never read
/// into memory at all. That matters because the caller runs over paths the user
/// may have replaced with anything, including a large binary asset.
fn read_mergeable(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > MAX_MERGE_BYTES as u64 {
        return None;
    }
    std::fs::read(path).ok()
}

/// The blob id of the file at `path`, streamed rather than read into memory,
/// and `None` when there is no file there. The staleness key, so a confirm
/// compares content rather than mtime.
fn hash_on_disk(path: &Path) -> Option<git2::Oid> {
    git2::Oid::hash_file(git2::ObjectType::Blob, path).ok()
}

/// A unified diff from the installed version to the user's copy.
///
/// Labelled `data/<path>`, i.e. relative to the workspace repo root, not to
/// `data/`. That is the root `git apply` resolves against, so the patch lands
/// with no `-p` juggling. An absent `base` is an add: the diff is then against
/// nothing, which is still the right patch to hand back.
fn unified_diff(data_relative: &str, base: Option<&[u8]>, ours: &[u8]) -> Option<String> {
    let repo_relative = format!("{}/{}", DATA_DIR, data_relative);
    let path = Path::new(&repo_relative);
    let mut patch =
        git2::Patch::from_buffers(base.unwrap_or(b""), Some(path), ours, Some(path), None).ok()?;
    let buf = patch.to_buf().ok()?;
    buf.as_str().map(str::to_string)
}

#[cfg(test)]
#[path = "../plugins_tests/merge.rs"]
mod tests;
