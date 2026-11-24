//! What a plugin update does to the files on disk: a three-way merge against
//! the user's local edits, and the removal of files the new version dropped.
//!
//! An update overwrites every file the plugin ships. When the user has edited
//! one of those files, the three inputs a merge needs are all in hand at
//! staging time: **base** is the file at the install commit, **ours** is what
//! is on disk now, and **theirs** is the staged new version.
//!
//! A file the previous version recorded, and the new one no longer ships, is
//! removed. A moved file then leaves no stale copy behind. An edited one is
//! saved aside first.
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
use crate::engine::pending_plugins::{
    hash_on_disk, LocalChange, LocalChangeOutcome, ResolvedChange, UpdatePlan,
};

use super::registry::{modification_status_for, PluginBaseline};

/// Above this, on any side, a file is replaced rather than merged. A plugin
/// ships prose, markup and small scripts. Anything at this scale was not
/// hand-edited, and holding three copies of it for the staging TTL is not worth
/// the merge.
const MAX_MERGE_BYTES: usize = 1024 * 1024;

pub(crate) const SAVED_CHANGES_ROOT: &str = "artifacts/plugin-local-changes";

/// Write every local edit this confirm is about to discard, before anything is
/// overwritten. Each one is saved twice: the whole file, and a patch against
/// the version it was written on. Returns the `data/`-relative paths written,
/// for the caller to commit and announce.
///
/// A conflict, an unmergeable file, a switched-off keep control and an edited
/// file the new version dropped all land here. A clean merge does not: the edit
/// survives in the file itself, so a copy would be clutter.
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
         saved them here instead of losing them. A file the new version no \
         longer ships was removed from where it lived, and its copy is here.\n\
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

pub(crate) fn local_patch(workspace_path: &Path, baseline: &PluginBaseline) -> Option<String> {
    let status = modification_status_for(workspace_path, &baseline.commit, &baseline.files);
    if !status.modified {
        return None;
    }
    let repo = git2::Repository::open(workspace_path).ok()?;
    let tree = install_tree(&repo, &baseline.commit)?;
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
        let base = blob_at(&repo, &tree, rel);
        if base.as_deref().is_some_and(|b| !is_mergeable_text(b)) {
            continue;
        }
        if let Some(diff) = unified_diff(rel, base.as_deref(), &ours) {
            out.push_str(&diff);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The version on disk, opened for reading: its install commit's tree, and the
/// paths the user has changed since.
struct OpenedBaseline<'r> {
    repo: &'r git2::Repository,
    tree: git2::Tree<'r>,
    modified: BTreeSet<String>,
}

pub(crate) fn plan_update(
    workspace_path: &Path,
    recorded: &[String],
    baseline: Option<&PluginBaseline>,
    planned: &[PlannedFile],
    recorded_by_others: &BTreeSet<&str>,
) -> UpdatePlan {
    let repo = git2::Repository::open(workspace_path).ok();
    let opened = baseline.zip(repo.as_ref()).and_then(|(baseline, repo)| {
        let tree = install_tree(repo, &baseline.commit)?;
        let status = modification_status_for(workspace_path, &baseline.commit, &baseline.files);
        Some(OpenedBaseline {
            repo,
            tree,
            modified: status.modified_paths.into_iter().collect(),
        })
    });

    let mut plan = UpdatePlan::default();
    if let Some(opened) = &opened {
        plan_shipped(workspace_path, opened, planned, &mut plan);
    }
    plan_dropped(
        workspace_path,
        opened.as_ref(),
        recorded,
        planned,
        recorded_by_others,
        &mut plan,
    );
    plan
}

/// Classify every path the update would overwrite, merging the ones the user
/// has edited.
///
/// Two questions per path, not one. "Did this change since the install
/// commit?" makes it a candidate; "is ours already theirs?" drops it again. A
/// user who published their own edit upstream meets the second on every file
/// they sent.
fn plan_shipped(
    workspace_path: &Path,
    baseline: &OpenedBaseline<'_>,
    planned: &[PlannedFile],
    plan: &mut UpdatePlan,
) {
    let data_dir = workspace_path.join(DATA_DIR);
    for pf in planned {
        if !baseline.modified.contains(&pf.data_relative) {
            continue;
        }
        let live = data_dir.join(&pf.data_relative);
        let ours_oid = hash_on_disk(&live);
        if let Some(ours) = ours_oid {
            if Some(ours) == hash_on_disk(&pf.source) {
                plan.unchanged.push((pf.data_relative.clone(), ours));
                continue;
            }
        }
        let base = blob_at(baseline.repo, &baseline.tree, &pf.data_relative);
        let ours = read_mergeable(&live);
        let theirs = read_mergeable(&pf.source);

        let (outcome, merged) = classify(
            baseline.repo,
            pf,
            base.as_deref(),
            ours.as_deref(),
            theirs.as_deref(),
            ours_oid.is_some(),
        );
        plan.changes.push(LocalChange {
            data_relative: pf.data_relative.clone(),
            outcome,
            merged,
            patch: patch_of(&pf.data_relative, base.as_deref(), ours.as_deref()),
            ours_oid,
        });
    }
}

fn plan_dropped(
    workspace_path: &Path,
    baseline: Option<&OpenedBaseline<'_>>,
    recorded: &[String],
    planned: &[PlannedFile],
    recorded_by_others: &BTreeSet<&str>,
    plan: &mut UpdatePlan,
) {
    let data_dir = workspace_path.join(DATA_DIR);
    let shipped: BTreeSet<String> = planned
        .iter()
        .map(|pf| pf.data_relative.to_lowercase())
        .collect();
    let dropped = recorded.iter().filter(|rel| {
        !shipped.contains(&rel.to_lowercase())
            && !recorded_by_others.contains(rel.as_str())
            && super::update_may_remove(rel)
    });
    for rel in dropped {
        let live = data_dir.join(rel);
        let Some(ours_oid) = hash_on_disk(&live) else {
            continue;
        };
        // Only a committed copy makes a removal undoable. So the bytes on disk
        // must be the install commit's, not merely absent from the Modified set.
        if baseline.is_some_and(|b| oid_at(&b.tree, rel) == Some(ours_oid)) {
            plan.removals.push((rel.clone(), ours_oid));
            continue;
        }
        // No baseline means no diff, so the whole-file copy is the only one.
        let patch = baseline.and_then(|b| {
            let base = blob_at(b.repo, &b.tree, rel);
            patch_of(rel, base.as_deref(), read_mergeable(&live).as_deref())
        });
        plan.changes.push(LocalChange {
            data_relative: rel.clone(),
            outcome: LocalChangeOutcome::Removed,
            merged: None,
            patch,
            ours_oid: Some(ours_oid),
        });
    }
}

/// The install commit's tree, or `None` when the commit no longer resolves.
fn install_tree<'r>(repo: &'r git2::Repository, commit: &str) -> Option<git2::Tree<'r>> {
    repo.revparse_single(commit)
        .and_then(|obj| obj.peel_to_commit())
        .and_then(|c| c.tree())
        .ok()
}

/// The blob id the install commit recorded for a `data/`-relative path.
fn oid_at(tree: &git2::Tree<'_>, data_relative: &str) -> Option<git2::Oid> {
    tree.get_path(Path::new(&format!("{}/{}", DATA_DIR, data_relative)))
        .ok()
        .map(|entry| entry.id())
}

/// The bytes the install commit recorded for a `data/`-relative path.
fn blob_at(repo: &git2::Repository, tree: &git2::Tree<'_>, data_relative: &str) -> Option<Vec<u8>> {
    oid_at(tree, data_relative)
        .and_then(|id| repo.find_blob(id).ok())
        .map(|blob| blob.content().to_vec())
}

/// The patch filed beside a saved-aside copy, when both sides are text.
fn patch_of(data_relative: &str, base: Option<&[u8]>, ours: Option<&[u8]>) -> Option<String> {
    ours.filter(|o| diffable(o, base))
        .and_then(|o| unified_diff(data_relative, base, o))
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
            log_legacy!(@Plugins, "merge {} failed, replacing instead: {}", pf.data_relative, e);
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
