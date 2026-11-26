//! State for plugin installations and removals awaiting confirmation.

use std::path::{Path, PathBuf};

use crate::core::{plugins::PluginManifest, DATA_DIR};

/// One plugin uninstall awaiting user confirmation.
pub struct PendingUninstall {
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_name: String,
    /// The installed hook scope when the uninstall was staged.
    pub hooks: Vec<PluginHook>,
    /// Recorded files that exist on disk at prepare time. Confirm attempts
    /// to delete each; any that disappear before confirm fold into the
    /// resulting event's `files_missing`.
    pub files_present: Vec<String>,
    /// Recorded files already gone at prepare time. Shown in the panel for
    /// transparency but never re-deleted.
    pub files_missing: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One plugin install awaiting user confirmation.
pub struct PendingInstall {
    /// Owns the staging root and removes it on drop.
    pub staging: tempfile::TempDir,
    /// Plugin root inside the staged tree (handles GitHub-tree subpaths).
    pub plugin_root: PathBuf,
    pub source_type: SourceType,
    pub source_string: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_name: String,
    /// The setup the staged version asks for. On confirm, `setup_occasion`
    /// decides from it whether a Lucidos Agent thread walks the user through
    /// the author's setup instructions.
    pub setup: StagedSetup,
    pub hooks: Vec<PluginHook>,
    pub prior_hooks: Vec<PluginHook>,
    /// What this install would do to the user's edited files, and which files
    /// it would remove. Decided at staging time, so the confirm panel can state
    /// it per file. Empty for a fresh install.
    pub plan: UpdatePlan,
    /// Every installed plugin's recorded paths as the plan saw them, `None`
    /// when they could not be read. See `check_records_still_match`.
    pub recorded_files: Option<RecordedFiles>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginHook {
    pub slug: String,
    pub name: String,
    pub event_type: String,
    pub content_hash: String,
}

/// Recorded paths per installed plugin id: `InstalledPlugins::files`.
pub type RecordedFiles = std::collections::BTreeMap<String, Vec<String>>;

pub type PendingInstallsMap = std::sync::Mutex<std::collections::HashMap<String, PendingInstall>>;

/// Type alias kept symmetric with `PendingInstallsMap` — the engine field
/// uses the inner type, but module helpers take this for readability.
pub type PendingUninstallsMap =
    std::sync::Mutex<std::collections::HashMap<String, PendingUninstall>>;

/// The setup a staged plugin version asks for, as the manifest states it.
/// `text` is normalized; `revision` is 0 when the manifest has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSetup {
    pub text: Option<String>,
    pub revision: u64,
    pub reason: Option<String>,
}

/// How a successfully-fetched plugin was obtained. Serialized into
/// `PluginInstalled.source_type` via `as_str()` -- the wire format is
/// `"git"` or `"archive"` and downstream consumers (events table,
/// future projections) match on those literals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceType {
    Git,
    Archive,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Archive => "archive",
        }
    }
}

/// What the confirm will do with one path the user has locally edited. A path
/// the user has NOT edited never becomes a [`LocalChange`] at all, and neither
/// does one whose copy already equals the staged version. Both are overwritten,
/// or removed, exactly as an unedited file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalChangeOutcome {
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
    /// The new version no longer ships the file. It is removed, so a stale
    /// copy cannot keep loading, and the user's version is saved aside first.
    Removed,
}

impl LocalChangeOutcome {
    /// Wire value for the staged preview and the confirm response.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Conflict => "conflict",
            Self::Replaced => "replaced",
            Self::Restored => "restored",
            Self::Removed => "removed",
        }
    }

    /// Whether confirming discards content that must be copied aside first.
    /// `Merged` keeps the edit in the file, and `Restored` has no content to
    /// keep: the user deleted it.
    fn discards_local_work(self) -> bool {
        matches!(self, Self::Conflict | Self::Replaced | Self::Removed)
    }
}

/// One path the user edited, and what the update will do with it.
///
/// A plan sits in the pending-install map for up to an hour, so it holds file
/// content only where it must. The merged bytes and the patch are needed at
/// confirm time and are bounded by `MAX_MERGE_BYTES`. The user's own copy is
/// NOT retained: saving it aside copies it off disk, which the drift check has
/// just proved unchanged. Retaining it would mean holding an arbitrarily large
/// modified binary in memory for the whole staging window.
pub struct LocalChange {
    pub data_relative: String,
    pub outcome: LocalChangeOutcome,
    /// The merged bytes, for [`LocalChangeOutcome::Merged`] only.
    pub merged: Option<Vec<u8>>,
    /// A unified diff from the install commit to the user's copy, saving a
    /// discarded edit as something re-appliable rather than a whole file alone.
    /// Generated for every change, since the user may still switch the panel's
    /// keep control off and discard one that would have merged. `None` for
    /// content no diff can express, such as a binary or an oversized file.
    pub patch: Option<String>,
    /// The user's copy as it hashed when the plan was made, streamed off disk.
    /// Re-read at confirm so an edit landing inside the staging window cannot
    /// be silently clobbered. `None` when they had deleted the file.
    pub ours_oid: Option<git2::Oid>,
}

/// Every locally-edited path an update would overwrite or remove, and every
/// unedited path it would remove. Empty for a fresh install, and for an update
/// that drops nothing over a plugin nobody has touched.
#[derive(Default)]
pub struct UpdatePlan {
    pub changes: Vec<LocalChange>,
    /// Edited paths whose copy already equalled the staged version, with the
    /// hash that made them equal. No decision, so the panel never lists them
    /// and the confirm never resolves them. Still watched for drift: the user
    /// edits these files, and one edited inside the staging window is real
    /// work the confirm would otherwise overwrite unseen.
    pub unchanged: Vec<(String, git2::Oid)>,
    /// Unedited paths the new version dropped, with the hash they had at
    /// staging. Removed with no copy, since the install commit holds them.
    pub removals: Vec<(String, git2::Oid)>,
}

/// Trim the manifest's setup instructions and map blank text to `None`. Staging
/// and confirmation normalize the same way before comparing the pending and
/// installed instructions.
pub fn normalize_setup(setup: Option<&str>) -> Option<String> {
    setup
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl StagedSetup {
    pub fn from_manifest(manifest: &PluginManifest) -> Self {
        Self {
            text: normalize_setup(manifest.setup.as_deref()),
            revision: manifest.setup_revision,
            reason: manifest.setup_reason.clone(),
        }
    }
}

impl UpdatePlan {
    /// Every path the confirm will delete, edited or not: the staged preview's
    /// `removes`, and the writer's list.
    pub fn removes(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .removals
            .iter()
            .map(|(path, _)| path.clone())
            .chain(
                self.changes
                    .iter()
                    .filter(|c| c.outcome == LocalChangeOutcome::Removed)
                    .map(|c| c.data_relative.clone()),
            )
            .collect();
        paths.sort();
        paths
    }

    /// The `local_changes` block of the staged preview: what the panel lists
    /// before the user confirms.
    pub fn preview_json(&self) -> serde_json::Value {
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
    /// Over every edited path, listed or not, and every path it would remove.
    /// A path dropped for already matching upstream is silent in the panel,
    /// never in here. Nothing is deleted on a judgment the user did not see.
    pub fn detect_drift(&self, workspace_path: &Path) -> Option<String> {
        let data_dir = workspace_path.join(DATA_DIR);
        self.changes
            .iter()
            .map(|c| (&c.data_relative, c.ours_oid))
            .chain(
                self.unchanged
                    .iter()
                    .chain(&self.removals)
                    .map(|(path, oid)| (path, Some(*oid))),
            )
            .find_map(|(path, staged)| {
                (hash_on_disk(&data_dir.join(path)) != staged).then(|| path.clone())
            })
    }

    /// Resolve the plan against the user's keep-or-replace choice.
    /// `keep_local_changes == false` collapses every outcome to `Replaced`, so
    /// the update lands clean upstream and every edit is saved aside.
    pub fn resolve(&self, keep_local_changes: bool) -> Vec<ResolvedChange<'_>> {
        self.changes
            .iter()
            .map(|c| ResolvedChange {
                change: c,
                // The keep control cannot touch a restore or a removal. A
                // restore has no edit to keep, and keeping a dropped file in
                // place is the stale copy the removal exists to prevent.
                outcome: match (keep_local_changes, c.outcome) {
                    (true, outcome) => outcome,
                    (
                        false,
                        outcome @ (LocalChangeOutcome::Restored | LocalChangeOutcome::Removed),
                    ) => outcome,
                    (false, _) => LocalChangeOutcome::Replaced,
                },
            })
            .collect()
    }
}

/// A [`LocalChange`] with the panel's keep control applied.
pub struct ResolvedChange<'a> {
    change: &'a LocalChange,
    pub outcome: LocalChangeOutcome,
}

impl ResolvedChange<'_> {
    pub fn data_relative(&self) -> &str {
        &self.change.data_relative
    }

    /// Merged bytes to write instead of upstream's, when this change survives
    /// as a merge.
    pub fn merged_bytes(&self) -> Option<&[u8]> {
        (self.outcome == LocalChangeOutcome::Merged)
            .then_some(self.change.merged.as_deref())
            .flatten()
    }

    /// The patch to file beside the saved copy, when confirming discards this
    /// edit. `Some(None)` means save the copy but write no patch, which is the
    /// binary and oversized case.
    pub fn to_save_aside(&self) -> Option<Option<&str>> {
        // Nothing on disk to copy: the user had deleted the file.
        if !self.outcome.discards_local_work() || self.change.ours_oid.is_none() {
            return None;
        }
        Some(self.change.patch.as_deref())
    }
}

/// The blob id of the file at `path`, streamed rather than read into memory,
/// and `None` when there is no file there. The staleness key, so a confirm
/// compares content rather than mtime.
pub fn hash_on_disk(path: &Path) -> Option<git2::Oid> {
    git2::Oid::hash_file(git2::ObjectType::Blob, path).ok()
}
