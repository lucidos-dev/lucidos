use std::path::{Path, PathBuf};

use crate::core::{plugins::PluginManifest, DATA_DIR};

pub struct PendingUninstall {
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_name: String,
    pub hooks: Vec<PluginHook>,
    pub files_present: Vec<String>,
    pub files_missing: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct PendingInstall {
    pub staging: tempfile::TempDir,
    pub plugin_root: PathBuf,
    pub source_type: SourceType,
    pub source_string: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_name: String,
    pub setup: StagedSetup,
    pub hooks: Vec<PluginHook>,
    pub prior_hooks: Vec<PluginHook>,
    pub plan: UpdatePlan,
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

pub type RecordedFiles = std::collections::BTreeMap<String, Vec<String>>;

pub type PendingInstallsMap = std::sync::Mutex<std::collections::HashMap<String, PendingInstall>>;

pub type PendingUninstallsMap =
    std::sync::Mutex<std::collections::HashMap<String, PendingUninstall>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSetup {
    pub text: Option<String>,
    pub revision: u64,
    pub reason: Option<String>,
}

/// INVARIANT: How a successfully-fetched plugin was obtained. Serialized into
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalChangeOutcome {
    Merged,
    /// INVARIANT: Both sides changed the same region. Upstream wins on disk and the user's
    /// version is saved aside. These files are LLM context, so conflict markers
    /// would become an instruction the engine acts on.
    Conflict,
    /// INVARIANT: Locally edited but not mergeable: a trigger projection, a binary, or an
    /// oversized file. Upstream wins on disk and the user's version is saved
    /// aside, so `Replaced` always implies a saved copy exists.
    Replaced,
    Restored,
    Removed,
}

impl LocalChangeOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Conflict => "conflict",
            Self::Replaced => "replaced",
            Self::Restored => "restored",
            Self::Removed => "removed",
        }
    }

    fn discards_local_work(self) -> bool {
        matches!(self, Self::Conflict | Self::Replaced | Self::Removed)
    }
}

pub struct LocalChange {
    pub data_relative: String,
    pub outcome: LocalChangeOutcome,
    pub merged: Option<Vec<u8>>,
    /// INVARIANT: A unified diff from the install commit to the user's copy, saving a
    /// discarded edit as something re-appliable rather than a whole file alone.
    /// Generated for every change, since the user may still switch the panel's
    /// keep control off and discard one that would have merged. `None` for
    /// content no diff can express, such as a binary or an oversized file.
    pub patch: Option<String>,
    /// INVARIANT: The user's copy as it hashed when the plan was made, streamed off disk.
    /// Re-read at confirm so an edit landing inside the staging window cannot
    /// be silently clobbered. `None` when they had deleted the file.
    pub ours_oid: Option<git2::Oid>,
}

#[derive(Default)]
pub struct UpdatePlan {
    pub changes: Vec<LocalChange>,
    /// INVARIANT: Edited paths whose copy already equalled the staged version, with the
    /// hash that made them equal. No decision, so the panel never lists them
    /// and the confirm never resolves them. Still watched for drift: the user
    /// edits these files, and one edited inside the staging window is real
    /// work the confirm would otherwise overwrite unseen.
    pub unchanged: Vec<(String, git2::Oid)>,
    pub removals: Vec<(String, git2::Oid)>,
}

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

    /// INVARIANT: Re-read each edited file and compare it to the blob recorded at staging
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

    pub fn resolve(&self, keep_local_changes: bool) -> Vec<ResolvedChange<'_>> {
        self.changes
            .iter()
            .map(|c| ResolvedChange {
                change: c,
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

pub struct ResolvedChange<'a> {
    change: &'a LocalChange,
    pub outcome: LocalChangeOutcome,
}

impl ResolvedChange<'_> {
    pub fn data_relative(&self) -> &str {
        &self.change.data_relative
    }

    pub fn merged_bytes(&self) -> Option<&[u8]> {
        (self.outcome == LocalChangeOutcome::Merged)
            .then_some(self.change.merged.as_deref())
            .flatten()
    }

    pub fn to_save_aside(&self) -> Option<Option<&str>> {
        if !self.outcome.discards_local_work() || self.change.ours_oid.is_none() {
            return None;
        }
        Some(self.change.patch.as_deref())
    }
}

pub fn hash_on_disk(path: &Path) -> Option<git2::Oid> {
    git2::Oid::hash_file(git2::ObjectType::Blob, path).ok()
}
