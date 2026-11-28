use chrono::{DateTime, Utc};
use git2::{ObjectType, Repository};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ARTIFACTS_DIR;
use crate::core::repository_lock::WorkspaceRepoLock;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

pub const BROWSEABLE_DATA_SUBDIRS: &[&str] = &["artifacts", "apps", "knowhow", "triggers"];

pub const VENDORED_DIR_NAMES: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    "__pycache__",
    ".venv",
    "venv",
    ".pytest_cache",
    "out",
];

/// INVARIANT: True when any DIRECTORY segment of `data_relative` (a path in the shape
/// [`list_searchable_data_files`] returns, e.g.
/// `apps/demo/remotion/node_modules/x.js`) names a vendored or build
/// directory. Every segment is tested, not just the first: a workspace's
/// vendored tree is typically buried several levels down, which a root-only
/// check misses entirely. The file name itself is never tested, so a file the
/// user called `build` or `out` is still theirs.
///
/// Deliberately NOT applied by [`list_searchable_data_files`]. That walk backs
/// the `list_files`, `glob_files` and `grep_files` tools, where filtering would
/// remove the agent's only way to reach a vendored file on purpose. It is for
/// code that SHAPES a payload, where a listing is a sample rather than an
/// answer: see `engine::chat::process::workspace_prompt_footprint`.
pub fn is_vendored_path(data_relative: &str) -> bool {
    let Some((dirs, _file)) = data_relative.rsplit_once('/') else {
        return false;
    };
    dirs.split('/').any(|seg| VENDORED_DIR_NAMES.contains(&seg))
}

const BUILD_OUTPUT_FILE_EXTENSIONS: &[&str] = &["pyc", "pyo"];

pub fn is_build_output_file(file_name: &str) -> bool {
    Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| BUILD_OUTPUT_FILE_EXTENSIONS.contains(&ext))
}

pub fn is_build_output_path(data_relative: &str) -> bool {
    let file_name = data_relative.rsplit('/').next().unwrap_or(data_relative);
    is_vendored_path(data_relative) || is_build_output_file(file_name)
}

/// INVARIANT: Walk the four browseable data/ subdirs and return each file as
/// `(data-relative path, absolute path)`, sorted lexicographically.
/// Shared by `ArtifactManager::list_artifacts` and the search tools so
/// the ignore policy stays consistent. Symlinks are intentionally skipped:
/// `entry.file_type()` doesn't follow them, which prevents both leaks
/// outside data/ and infinite recursion on symlink loops.
pub fn list_searchable_data_files(
    workspace_path: &Path,
) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    walk_data_files(workspace_path, false)
}

pub fn list_user_data_files(
    workspace_path: &Path,
) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    walk_data_files(workspace_path, true)
}

fn walk_data_files(
    workspace_path: &Path,
    skip_build_output: bool,
) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    let data_path = workspace_path.join("data");
    let mut out = Vec::new();
    if !data_path.exists() {
        return Ok(out);
    }
    fn walk(
        dir: &Path,
        base: &Path,
        prefix: &str,
        skip_build_output: bool,
        out: &mut Vec<(String, PathBuf)>,
    ) -> Result<(), std::io::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() {
                if !(skip_build_output && VENDORED_DIR_NAMES.contains(&name.as_ref())) {
                    walk(&path, base, prefix, skip_build_output, out)?;
                }
            } else if ft.is_file() && !(skip_build_output && is_build_output_file(&name)) {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push((format!("{}/{}", prefix, rel.to_string_lossy()), path));
                }
            }
        }
        Ok(())
    }
    for sub in BROWSEABLE_DATA_SUBDIRS {
        let dir = data_path.join(sub);
        if dir.exists() {
            walk(&dir, &dir, sub, skip_build_output, &mut out)?;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub struct ArtifactChange {
    pub path: String,
    pub commit_hash: String,
    pub timestamp: DateTime<Utc>,
}

fn reject_path_traversal(p: &str) -> Result<(), git2::Error> {
    if crate::core::is_path_traversal(p) {
        return Err(git2::Error::from_str(&format!(
            "Path traversal not allowed: {}",
            p
        )));
    }
    Ok(())
}

pub enum WriteAnnouncement {
    Entity {
        source: Option<String>,
        writer: crate::core::artifact_writer::ArtifactWriterOrigin,
        writer_thread_id: Option<uuid::Uuid>,
    },
    /// INVARIANT: The caller emits a RICHER event that stands in for the entity event on
    /// this write, and the entity event must be suppressed rather than added.
    ///
    /// The case this exists for is `ArtifactImported`: it carries the source and
    /// a summary, the frontend reloads the artifact list on it, and
    /// `memory_consumer` indexes on it. Emitting a paired `ArtifactCreated`
    /// would put two rows on the timeline for one import and index the same
    /// file into memory twice.
    ///
    /// Names the event so a reader can check the claim.
    SupersededBy(&'static str),
}

pub struct ArtifactManager {
    workspace_path: PathBuf,
    repo: Arc<Mutex<Repository>>,
}

struct PreparedDirtyIndex {
    base: super::IndexBase,
    entries: Vec<git2::IndexEntry>,
}

impl ArtifactManager {
    pub fn new(workspace_path: PathBuf) -> Result<Self, git2::Error> {
        let artifacts_path = workspace_path.join(ARTIFACTS_DIR);
        if let Err(e) = fs::create_dir_all(&artifacts_path) {
            log_legacy!("[Artifacts] Failed to create artifacts directory: {}", e);
        }

        let repo = match Repository::open(&workspace_path) {
            Ok(repo) => repo,
            Err(_) => {
                let mut opts = git2::RepositoryInitOptions::new();
                opts.initial_head("main");
                Repository::init_opts(&workspace_path, &opts)?
            }
        };

        // INVARIANT: Ensure .gitignore exists and contains all engine-managed entries.
        // Idempotent across boots — appends new entries when the engine adds
        // them (e.g. data/blobs/) without rewriting historical lines. When
        // the file is newly created or modified, also commit it to the
        // artifacts repo so the workspace's git history records the change.
        //
        // Request handlers construct managers too, so this commit races other
        // writers like any other and retries. The next construction finds the
        // entries present and never commits, so a lost race would leave the
        // edit uncommitted for good.
        match super::ensure_workspace_gitignore_entries(&workspace_path) {
            Ok(true) => {
                let committed = super::retry_while_repo_contended(|| {
                    let mut index = repo.index()?;
                    let base = super::reset_index_to_head(&repo, &mut index)?;
                    index.add_path(Path::new(".gitignore"))?;
                    index.write()?;
                    super::commit_index(&repo, "chore: update .gitignore", base)
                });
                if let Err(e) = committed {
                    log_legacy!("[Artifacts] Failed to commit .gitignore: {}", e);
                }
            }
            Ok(false) => {}
            Err(e) => log_legacy!("[Artifacts] Failed to ensure .gitignore entries: {}", e),
        }

        Ok(Self {
            workspace_path,
            repo: Arc::new(Mutex::new(repo)),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn repo_handle_for_test(&self) -> Arc<Mutex<Repository>> {
        self.repo.clone()
    }

    fn write_artifact(
        &self,
        relative_path: &str,
        content: impl AsRef<[u8]>,
    ) -> Result<PathBuf, std::io::Error> {
        // INVARIANT: Defence-in-depth: API/data_api validates already, but artifact writes
        // also reach here from LLM tool handlers (`engine/tools/import.rs`,
        // `image.rs`, `http.rs`, `email.rs`, memory consumer) that pass paths
        // assembled from LLM-provided arguments. A `..` segment would otherwise
        // let the joined path escape `data/artifacts/`.
        reject_path_traversal(relative_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
        let full_path = crate::core::contained_path::join_contained(
            &self.workspace_path.join(ARTIFACTS_DIR),
            relative_path,
        )
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("'{relative_path}' resolves outside artifacts/ through a symlink"),
            )
        })?;

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&full_path, content)?;
        Ok(full_path)
    }

    pub async fn write_and_commit(
        &self,
        held: &WorkspaceRepoLock,
        event_bus: &EventBus,
        relative_path: &str,
        content: impl AsRef<[u8]>,
        message: &str,
        announcement: WriteAnnouncement,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let writer = match &announcement {
            WriteAnnouncement::Entity { writer, .. } => {
                Some(writer.clone().resolve(event_bus.pool()).await?)
            }
            WriteAnnouncement::SupersededBy(_) => None,
        };
        let existed = self.artifact_exists(relative_path);
        self.write_artifact(relative_path, content)?;
        let commit_sha = self.commit(held, relative_path, message).await?;
        if let WriteAnnouncement::Entity {
            source,
            writer_thread_id,
            ..
        } = announcement
        {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::artifact_change(
                        existed,
                        relative_path.to_string(),
                        commit_sha.clone(),
                        source,
                        writer_thread_id,
                        writer.expect("entity writer was resolved before writing"),
                    )),
                    "[Artifacts] artifact write",
                )
                .await;
        }
        Ok(commit_sha)
    }

    pub async fn write_batch_and_commit(
        &self,
        held: &WorkspaceRepoLock,
        files: &[(String, String)],
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        for (path, content) in files {
            self.write_artifact(path, content)?;
        }
        self.commit_batch(
            held,
            &files.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            message,
        )
        .await
        .map_err(Into::into)
    }

    pub async fn commit(
        &self,
        _held: &WorkspaceRepoLock,
        artifact_path: &str,
        message: &str,
    ) -> Result<String, git2::Error> {
        reject_path_traversal(artifact_path)?;
        let repo_path = format!("{}/{}", ARTIFACTS_DIR, artifact_path);
        self.commit_repo_path(&repo_path, message).await
    }

    pub(crate) async fn commit_batch(
        &self,
        _held: &WorkspaceRepoLock,
        artifact_paths: &[String],
        message: &str,
    ) -> Result<String, git2::Error> {
        for p in artifact_paths {
            reject_path_traversal(p)?;
        }
        let repo = self.repo.clone();
        let paths: Vec<String> = artifact_paths.to_vec();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                let base = super::reset_index_to_head(&repo, &mut index)?;

                for artifact_path in &paths {
                    let repo_path = format!("{}/{}", ARTIFACTS_DIR, artifact_path);
                    super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                }
                index.write()?;

                super::commit_index(&repo, &message, base)
            })
        })
        .await
        .unwrap()
    }

    pub async fn commit_data_path(
        &self,
        _held: &WorkspaceRepoLock,
        data_relative_path: &str,
        message: &str,
    ) -> Result<String, git2::Error> {
        reject_path_traversal(data_relative_path)?;
        let repo_path = format!("data/{}", data_relative_path);
        self.commit_repo_path(&repo_path, message).await
    }

    pub async fn commit_data_paths(
        &self,
        _held: &WorkspaceRepoLock,
        data_relative_paths: &[String],
        message: &str,
    ) -> Result<String, git2::Error> {
        for p in data_relative_paths {
            reject_path_traversal(p)?;
        }
        let repo = self.repo.clone();
        let paths: Vec<String> = data_relative_paths.to_vec();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                let base = super::reset_index_to_head(&repo, &mut index)?;

                for data_path in &paths {
                    let repo_path = format!("data/{}", data_path);
                    super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                }
                index.write()?;

                super::commit_index(&repo, &message, base)
            })
        })
        .await
        .unwrap()
    }

    pub async fn delete_data_path_and_commit(
        &self,
        _held: &WorkspaceRepoLock,
        event_bus: &EventBus,
        data_relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(data_relative_path)?;
        let full_path = self.workspace_path.join("data").join(data_relative_path);
        fs::remove_file(&full_path)?;
        let commit_id = self
            .commit_removal(data_relative_path, false, message)
            .await?;
        if let Some(artifact_path) = data_relative_path.strip_prefix("artifacts/") {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ArtifactDeleted {
                        artifact_path: artifact_path.to_string(),
                        commit: commit_id.clone(),
                    }),
                    "[Artifacts] ArtifactDeleted",
                )
                .await;
        }
        Ok(commit_id)
    }

    /// INVARIANT: Delete a data-relative folder, commit the deletion and emit `announce`,
    /// the event that names what the folder was. A link in its place is
    /// removed itself, never what it points at.
    pub async fn delete_data_dir_and_commit(
        &self,
        _held: &WorkspaceRepoLock,
        event_bus: &EventBus,
        data_relative_dir: &str,
        message: &str,
        announce: SystemEvent,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(data_relative_dir)?;
        let full_path = self.workspace_path.join("data").join(data_relative_dir);
        let is_dir = fs::symlink_metadata(&full_path)?.is_dir();
        if is_dir {
            fs::remove_dir_all(&full_path)?;
        } else {
            fs::remove_file(&full_path)?;
        }
        let commit_id = self
            .commit_removal(data_relative_dir, is_dir, message)
            .await?;
        event_bus.emit(BusEvent::System(announce)).await?;
        Ok(commit_id)
    }

    /// INVARIANT: Commit the removal of a data-relative path already gone from disk.
    ///
    /// The removal stays outside the retry closure, so a retried attempt only
    /// re-stages an already-absent path onto the winner's head. Staging is
    /// tolerant and the commit goes through `commit_index_unless_unchanged`,
    /// because the writer that won the race may have committed it already.
    async fn commit_removal(
        &self,
        data_relative_path: &str,
        dir: bool,
        message: &str,
    ) -> Result<String, git2::Error> {
        let repo = self.repo.clone();
        let repo_path = format!("data/{}", data_relative_path);
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                let base = super::reset_index_to_head(&repo, &mut index)?;
                let path = std::path::Path::new(&repo_path);
                let _ = if dir {
                    index.remove_dir(path, 0)
                } else {
                    index.remove_path(path)
                };
                index.write()?;
                super::commit_index_unless_unchanged(&repo, &message, base)
            })
        })
        .await
        .unwrap()
    }

    async fn commit_repo_path(
        &self,
        repo_path: &str,
        message: &str,
    ) -> Result<String, git2::Error> {
        let repo = self.repo.clone();
        let repo_path = repo_path.to_string();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                let base = super::reset_index_to_head(&repo, &mut index)?;

                super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                index.write()?;

                super::commit_index(&repo, &message, base)
            })
        })
        .await
        .unwrap()
    }

    /// INVARIANT: Prepare the data/ index without the repo lock, then rescan and
    /// commit under the lock. The rescan sees the finished merge tree.
    ///
    /// Acquire the guard between blocking tasks so cancelling preparation cannot
    /// queue a lock acquisition. Once acquired, the blocking writer owns the
    /// guard until it returns, even if the caller stops waiting.
    pub async fn commit_all_dirty(&self, message: &str) -> Result<Option<String>, git2::Error> {
        let workspace = self.workspace_path.clone();
        let prepared = tokio::task::spawn_blocking(move || Self::prepare_dirty_index(&workspace))
            .await
            .unwrap();

        let held = crate::core::repository_lock::lock_workspace_repo().await;
        let repo = self.repo.clone();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            let repo = repo.lock().unwrap();
            // Preparation can race a writer. The locked scan remains authoritative.
            Self::commit_dirty_index(&repo, prepared.ok(), &message, &held)
        })
        .await
        .unwrap()
    }

    fn prepare_dirty_index(workspace: &Path) -> Result<PreparedDirtyIndex, git2::Error> {
        let repo = Repository::open(workspace)?;
        let real_index = repo.index()?;
        let real_path = real_index
            .path()
            .ok_or_else(|| git2::Error::from_str("workspace has no index path"))?;

        // INVARIANT: libgit2's stat refresh writes the supplied index to disk.
        // Use a private copy so preparation can never publish a workspace index.
        let scratch = tempfile::tempdir().map_err(|e| {
            git2::Error::from_str(&format!("create temporary workspace index: {e}"))
        })?;
        let scratch_path = scratch.path().join("index");
        match fs::copy(real_path, &scratch_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(git2::Error::from_str(&format!("copy workspace index: {e}"))),
        }
        let mut index = git2::Index::open(&scratch_path)?;
        repo.set_index(&mut index)?;
        let base = super::reset_index_to_head(&repo, &mut index)?;
        repo.diff_index_to_workdir(
            Some(&index),
            Some(
                git2::DiffOptions::new()
                    .pathspec(super::DATA_DIR)
                    .update_index(true),
            ),
        )?;
        let entries = index
            .iter()
            .filter(|entry| entry.path.starts_with(b"data/"))
            .collect();
        Ok(PreparedDirtyIndex { base, entries })
    }

    fn commit_dirty_index(
        repo: &Repository,
        mut prepared: Option<PreparedDirtyIndex>,
        message: &str,
        _held: &WorkspaceRepoLock,
    ) -> Result<Option<String>, git2::Error> {
        super::retry_while_repo_contended(|| {
            let mut index = repo.index()?;
            index.read(true)?;
            let base = super::reset_index_to_head(repo, &mut index)?;
            if let Some(prepared) = prepared
                .take()
                .filter(|prepared| base.0.is_some() && prepared.base == base)
            {
                // INVARIANT: Reuse only the stats that libgit2 refreshed without
                // changing the tree. The locked scan stages edits and additions.
                for entry in prepared.entries {
                    index.add(&entry)?;
                }
            }

            index.add_all([super::DATA_DIR], git2::IndexAddOption::DEFAULT, None)?;
            index.write()?;

            let tree_id = index.write_tree()?;
            if let Ok(head) = repo.head() {
                if let Ok(head_commit) = head.peel_to_commit() {
                    if head_commit.tree()?.id() == tree_id {
                        return Ok(None);
                    }
                }
            }

            super::commit_index(repo, message, base).map(Some)
        })
    }

    pub fn artifact_exists(&self, relative_path: &str) -> bool {
        if reject_path_traversal(relative_path).is_err() {
            return false;
        }
        let full_path = self.workspace_path.join(ARTIFACTS_DIR).join(relative_path);
        full_path.exists()
    }

    pub fn resolve_collision_free_path(
        &self,
        relative_path: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        const MAX_COLLISION_SUFFIX: usize = 10_000;

        if !self.artifact_exists(relative_path) {
            return Ok(relative_path.to_string());
        }

        let (dir_prefix, leaf) = match relative_path.rfind('/') {
            Some(idx) => (&relative_path[..=idx], &relative_path[idx + 1..]),
            None => ("", relative_path),
        };
        let leaf_path = Path::new(leaf);
        let stem = leaf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(leaf);
        let ext = leaf_path.extension().and_then(|e| e.to_str());

        for n in 1..=MAX_COLLISION_SUFFIX {
            let candidate_leaf = match ext {
                Some(ext) => format!("{} ({}).{}", stem, n, ext),
                None => format!("{} ({})", stem, n),
            };
            let candidate = format!("{}{}", dir_prefix, candidate_leaf);
            if !self.artifact_exists(&candidate) {
                return Ok(candidate);
            }
        }

        Err(format!(
            "Could not find a free name for {} after {} attempts",
            relative_path, MAX_COLLISION_SUFFIX
        )
        .into())
    }

    pub async fn delete_and_commit(
        &self,
        held: &WorkspaceRepoLock,
        event_bus: &EventBus,
        relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let data_relative = format!("artifacts/{}", relative_path);
        self.delete_data_path_and_commit(held, event_bus, &data_relative, message)
            .await
    }

    pub fn list_artifacts(&self) -> Result<Vec<String>, std::io::Error> {
        Ok(list_searchable_data_files(&self.workspace_path)?
            .into_iter()
            .map(|(rel, _)| rel)
            .collect())
    }

    pub fn read_artifact_at_commit(
        &self,
        relative_path: &str,
        commit_hash: &str,
    ) -> Result<Vec<u8>, git2::Error> {
        reject_path_traversal(relative_path)?;
        let repo = self.repo.lock().unwrap();

        let oid = git2::Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let full_path = format!("{}/{}", ARTIFACTS_DIR, relative_path);

        let entry = tree.get_path(Path::new(&full_path))?;

        let blob = repo.find_blob(entry.id())?;

        Ok(blob.content().to_vec())
    }

    pub fn read_artifact_at_commit_string(
        &self,
        relative_path: &str,
        commit_hash: &str,
    ) -> Result<String, git2::Error> {
        let bytes = self.read_artifact_at_commit(relative_path, commit_hash)?;
        String::from_utf8(bytes).map_err(|_| git2::Error::from_str("File is not valid UTF-8"))
    }

    pub fn list_artifacts_at_commit(&self, commit_hash: &str) -> Result<Vec<String>, git2::Error> {
        let repo = self.repo.lock().unwrap();

        let oid = git2::Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let mut paths = Vec::new();

        if let Ok(artifacts_entry) = tree.get_path(Path::new(ARTIFACTS_DIR)) {
            if let Ok(artifacts_tree) = repo.find_tree(artifacts_entry.id()) {
                fn walk_tree(
                    repo: &Repository,
                    tree: &git2::Tree,
                    prefix: &str,
                    paths: &mut Vec<String>,
                ) {
                    for entry in tree.iter() {
                        let name = entry.name().unwrap_or("");
                        let full_path = if prefix.is_empty() {
                            name.to_string()
                        } else {
                            format!("{}/{}", prefix, name)
                        };

                        match entry.kind() {
                            Some(ObjectType::Tree) => {
                                if let Ok(subtree) = repo.find_tree(entry.id()) {
                                    walk_tree(repo, &subtree, &full_path, paths);
                                }
                            }
                            Some(ObjectType::Blob) => {
                                paths.push(full_path);
                            }
                            _ => {}
                        }
                    }
                }

                walk_tree(&repo, &artifacts_tree, "", &mut paths);
            }
        }

        Ok(paths)
    }

    pub fn find_commit_at_timestamp(&self, timestamp: i64) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();

        let head = repo.head()?.peel_to_commit()?;
        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        for oid in revwalk {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let commit_time = commit.time().seconds();

            if commit_time <= timestamp {
                return Ok(oid.to_string());
            }
        }

        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;

        if let Some(Ok(oid)) = revwalk.next() {
            return Ok(oid.to_string());
        }

        Err(git2::Error::from_str("No commits found"))
    }

    pub fn walk_artifact_history(&self) -> Result<Vec<ArtifactChange>, git2::Error> {
        let repo = self.repo.lock().unwrap();

        let head = match repo.head() {
            Ok(h) => h.peel_to_commit()?,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;

        // INVARIANT: Collect OIDs first to avoid borrow conflict with repo
        // One unreadable commit is skipped, not fatal: the caller rebuilds
        // memory from this list, and an error here would drop every artifact.
        let oids: Vec<git2::Oid> = revwalk
            .filter_map(|r| {
                r.map_err(|e| {
                    log_legacy!("[Artifacts] Skipping unreadable commit in history: {}", e)
                })
                .ok()
            })
            .collect();

        let artifacts_prefix = format!("{}/", super::ARTIFACTS_DIR);
        let mut changes = Vec::new();

        for oid in &oids {
            let commit = repo.find_commit(*oid)?;
            let commit_tree = commit.tree()?;

            let parent_tree = if commit.parent_count() > 0 {
                Some(commit.parent(0)?.tree()?)
            } else {
                None
            };

            let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None)?;

            let commit_hash = oid.to_string();
            let timestamp =
                DateTime::<Utc>::from_timestamp(commit.time().seconds(), 0).unwrap_or_default();

            diff.foreach(
                &mut |delta, _progress| {
                    let is_content_change =
                        matches!(delta.status(), git2::Delta::Added | git2::Delta::Modified);
                    if !is_content_change {
                        return true;
                    }

                    if let Some(path_str) = delta.new_file().path().and_then(|p| p.to_str()) {
                        if let Some(relative) = path_str.strip_prefix(&artifacts_prefix) {
                            changes.push(ArtifactChange {
                                path: relative.to_string(),
                                commit_hash: commit_hash.clone(),
                                timestamp,
                            });
                        }
                    }
                    true
                },
                None,
                None,
                None,
            )?;
        }

        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn index_mtime(repo: &Repository, path: &Path) -> i32 {
        repo.index()
            .unwrap()
            .get_path(path, 0)
            .unwrap()
            .mtime
            .seconds()
    }

    #[tokio::test]
    async fn auto_commit_persists_the_stat_refresh_without_creating_a_commit() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        let path = Path::new("data/artifacts/touched.txt");
        manager.write_artifact("touched.txt", "same bytes").unwrap();
        assert!(manager.commit_all_dirty("seed").await.unwrap().is_some());
        let before = index_mtime(&Repository::open(dir.path()).unwrap(), path);
        touch_without_editing(&dir.path().join(path));

        assert!(manager.commit_all_dirty("refresh").await.unwrap().is_none());
        assert_eq!(
            index_mtime(&Repository::open(dir.path()).unwrap(), path),
            before + 60
        );
    }

    fn touch_without_editing(path: &Path) {
        let file = fs::File::open(path).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        file.set_modified(modified + std::time::Duration::from_secs(60))
            .unwrap();
    }

    #[tokio::test]
    async fn auto_commit_prepares_under_contention_without_writing_the_workspace_index() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        manager.write_artifact("touched.txt", "same bytes").unwrap();
        manager.commit_all_dirty("seed").await.unwrap();
        touch_without_editing(&dir.path().join(ARTIFACTS_DIR).join("touched.txt"));
        let index_path = Repository::open(dir.path()).unwrap().path().join("index");
        let before = fs::read(&index_path).unwrap();

        let _held = super::super::repository_lock::lock_workspace_repo().await;
        let path = dir.path().to_path_buf();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || ArtifactManager::prepare_dirty_index(&path)),
        )
        .await
        .expect("preparation must finish without the repo lock")
        .unwrap()
        .unwrap();

        assert_eq!(fs::read(&index_path).unwrap(), before);
    }

    #[tokio::test]
    async fn auto_commit_restages_after_the_merge_tree_sync() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let path = Path::new("data/artifacts/merged.txt");
        let held = super::super::repository_lock::lock_workspace_repo().await;
        let mut index = repo.index().unwrap();
        let base = super::super::reset_index_to_head(&repo, &mut index).unwrap();
        fs::write(dir.path().join(path), "merged").unwrap();
        index.add_path(path).unwrap();
        index.write().unwrap();
        super::super::commit_index(&repo, "merge advances HEAD", base).unwrap();
        fs::remove_file(dir.path().join(path)).unwrap();

        manager.write_artifact("dirty.txt", "dirty").unwrap();
        let prepared = ArtifactManager::prepare_dirty_index(dir.path()).unwrap();
        fs::write(dir.path().join(path), "merged").unwrap();
        let commit = ArtifactManager::commit_dirty_index(&repo, Some(prepared), "auto", &held)
            .unwrap()
            .unwrap();

        let tree = repo
            .find_commit(commit.parse().unwrap())
            .unwrap()
            .tree()
            .unwrap();
        assert!(tree.get_path(path).is_ok());
        assert!(tree.get_path(Path::new("data/artifacts/dirty.txt")).is_ok());
    }

    #[tokio::test]
    async fn auto_commit_keeps_a_single_path_deletion_between_phases() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        let path = Path::new("data/artifacts/doomed.txt");
        manager.write_artifact("doomed.txt", "seed").unwrap();
        manager.commit_all_dirty("seed").await.unwrap();
        manager.write_artifact("doomed.txt", "edited").unwrap();
        manager.write_artifact("dirty.txt", "dirty").unwrap();

        let prepared = ArtifactManager::prepare_dirty_index(dir.path()).unwrap();
        let held = super::super::repository_lock::lock_workspace_repo().await;
        let repo = Repository::open(dir.path()).unwrap();
        fs::remove_file(dir.path().join(path)).unwrap();
        let deletion = manager
            .commit_removal("artifacts/doomed.txt", false, "delete doomed")
            .await
            .unwrap();
        let commit = ArtifactManager::commit_dirty_index(&repo, Some(prepared), "auto", &held)
            .unwrap()
            .unwrap();

        let head = repo.find_commit(commit.parse().unwrap()).unwrap();
        assert_eq!(head.parent_id(0).unwrap().to_string(), deletion);
        let tree = head.tree().unwrap();
        assert!(tree.get_path(path).is_err());
        assert!(tree.get_path(Path::new("data/artifacts/dirty.txt")).is_ok());
    }

    #[tokio::test]
    async fn auto_commit_applies_current_ignore_rules_to_new_paths() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        manager
            .commit_all_dirty("warm up ignore cache")
            .await
            .unwrap();
        manager
            .write_artifact("temporary.txt", "temporary")
            .unwrap();
        let prepared = ArtifactManager::prepare_dirty_index(dir.path()).unwrap();
        fs::write(
            dir.path().join(ARTIFACTS_DIR).join(".gitignore"),
            "temporary.txt\n",
        )
        .unwrap();
        let held = super::super::repository_lock::lock_workspace_repo().await;
        let repo = manager.repo.lock().unwrap();
        let commit = ArtifactManager::commit_dirty_index(&repo, Some(prepared), "auto", &held)
            .unwrap()
            .unwrap();

        let tree = repo
            .find_commit(commit.parse().unwrap())
            .unwrap()
            .tree()
            .unwrap();
        assert!(tree
            .get_path(Path::new("data/artifacts/temporary.txt"))
            .is_err());
    }

    #[tokio::test]
    async fn auto_commit_applies_current_attributes_to_tracked_edits() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        manager.write_artifact("example.txt", "seed\n").unwrap();
        manager.commit_all_dirty("seed").await.unwrap();
        manager.write_artifact("example.txt", "edited\r\n").unwrap();

        let prepared = ArtifactManager::prepare_dirty_index(dir.path()).unwrap();
        fs::write(
            dir.path().join(ARTIFACTS_DIR).join(".gitattributes"),
            "example.txt text eol=lf\n",
        )
        .unwrap();
        let held = super::super::repository_lock::lock_workspace_repo().await;
        let repo = manager.repo.lock().unwrap();
        let commit = ArtifactManager::commit_dirty_index(&repo, Some(prepared), "auto", &held)
            .unwrap()
            .unwrap();
        drop(repo);
        assert_eq!(
            manager
                .read_artifact_at_commit("example.txt", &commit)
                .unwrap(),
            b"edited\n"
        );
    }

    fn read_back(manager: &ArtifactManager, relative_path: &str) -> String {
        fs::read_to_string(
            manager
                .workspace_path
                .join(ARTIFACTS_DIR)
                .join(relative_path),
        )
        .unwrap()
    }

    #[test]
    fn the_user_walk_is_the_full_walk_without_build_output() {
        let dir = tempdir().unwrap();
        for rel in [
            "artifacts/notes.md",
            "artifacts/build/report.md",
            "apps/demo/index.html",
            "apps/demo/node_modules/pkg/index.js",
            "apps/demo/web/dist/bundle.js",
            "apps/demo/scripts/tool.py",
            "apps/demo/scripts/tool.cpython-312.pyc",
            "knowhow/out",
            "triggers/t/trigger.json",
            "config/apis.json",
        ] {
            let path = dir.path().join("data").join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let filtered: Vec<_> = list_searchable_data_files(dir.path())
            .unwrap()
            .into_iter()
            .filter(|(rel, _)| !is_build_output_path(rel))
            .collect();
        let pruned = list_user_data_files(dir.path()).unwrap();
        assert_eq!(pruned, filtered);
        let paths: Vec<_> = pruned.iter().map(|(rel, _)| rel.as_str()).collect();
        assert_eq!(
            paths,
            [
                "apps/demo/index.html",
                "apps/demo/scripts/tool.py",
                "artifacts/notes.md",
                "knowhow/out",
                "triggers/t/trigger.json",
            ]
        );
    }

    #[test]
    fn test_write_and_read_artifact() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager.write_artifact("test.txt", "hello world").unwrap();

        assert_eq!(read_back(&manager, "test.txt"), "hello world");
    }

    #[cfg(unix)]
    #[test]
    fn an_artifact_write_never_follows_a_symlink_out_of_artifacts() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        let artifacts = dir.path().join(ARTIFACTS_DIR);
        fs::create_dir_all(&artifacts).unwrap();
        fs::write(outside.path().join("rc"), "original").unwrap();
        std::os::unix::fs::symlink(outside.path().join("rc"), artifacts.join("notes.md")).unwrap();
        std::os::unix::fs::symlink(outside.path(), artifacts.join("dir")).unwrap();

        assert!(manager.write_artifact("notes.md", "changed").is_err());
        assert!(manager.write_artifact("dir/new.md", "x").is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("rc")).unwrap(),
            "original"
        );
        assert!(!outside.path().join("new.md").exists());
    }

    #[tokio::test]
    async fn test_commit_artifact() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager.write_artifact("test.txt", "hello").unwrap();
        let commit_sha = manager
            .commit(
                &crate::core::repository_lock::lock_workspace_repo().await,
                "test.txt",
                "Add test file",
            )
            .await
            .unwrap();

        assert!(!commit_sha.is_empty());
    }

    #[test]
    fn test_resolve_collision_free_path_no_collision() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        assert_eq!(
            manager
                .resolve_collision_free_path("imported/Brev.pdf")
                .unwrap(),
            "imported/Brev.pdf"
        );
    }

    #[test]
    fn test_resolve_collision_free_path_suffixes_in_sequence() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager
            .write_artifact("imported/Brev.pdf", "first")
            .unwrap();
        assert_eq!(
            manager
                .resolve_collision_free_path("imported/Brev.pdf")
                .unwrap(),
            "imported/Brev (1).pdf"
        );

        manager
            .write_artifact("imported/Brev (1).pdf", "second")
            .unwrap();
        assert_eq!(
            manager
                .resolve_collision_free_path("imported/Brev.pdf")
                .unwrap(),
            "imported/Brev (2).pdf"
        );
    }

    #[test]
    fn test_resolve_collision_free_path_no_extension() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager.write_artifact("imported/README", "x").unwrap();
        assert_eq!(
            manager
                .resolve_collision_free_path("imported/README")
                .unwrap(),
            "imported/README (1)"
        );
    }

    #[test]
    fn test_resolve_collision_free_path_multi_dot_keeps_final_ext() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager
            .write_artifact("imported/archive.tar.gz", "x")
            .unwrap();
        assert_eq!(
            manager
                .resolve_collision_free_path("imported/archive.tar.gz")
                .unwrap(),
            "imported/archive.tar (1).gz"
        );
    }

    #[tokio::test]
    async fn write_and_commit_announces_created_then_updated() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager
            .write_and_commit(
                &crate::core::repository_lock::lock_workspace_repo().await,
                &bus,
                "notes.md",
                "one",
                "Write notes",
                WriteAnnouncement::Entity {
                    writer: crate::core::artifact_writer::ArtifactWriterOrigin::Known(
                        crate::engine::event_bus::ArtifactWriter::Person,
                    ),
                    source: None,
                    writer_thread_id: None,
                },
            )
            .await
            .unwrap();
        manager
            .write_and_commit(
                &crate::core::repository_lock::lock_workspace_repo().await,
                &bus,
                "notes.md",
                "two",
                "Rewrite notes",
                WriteAnnouncement::Entity {
                    writer: crate::core::artifact_writer::ArtifactWriterOrigin::Known(
                        crate::engine::event_bus::ArtifactWriter::Person,
                    ),
                    source: Some("run_python".to_string()),
                    writer_thread_id: None,
                },
            )
            .await
            .unwrap();

        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM events \
             WHERE event_type IN ('ArtifactCreated', 'ArtifactUpdated') ORDER BY sequence",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(kinds, vec!["ArtifactCreated", "ArtifactUpdated"]);
        let writers: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT COALESCE(payload->'data', payload)->'writer' FROM events WHERE event_type IN ('ArtifactCreated', 'ArtifactUpdated') ORDER BY sequence",
        ).fetch_all(&pool).await.unwrap();
        let expected =
            serde_json::to_value(crate::engine::event_bus::ArtifactWriter::Person).unwrap();
        assert_eq!(writers, vec![expected.clone(), expected]);

        manager
            .write_and_commit(
                &crate::core::repository_lock::lock_workspace_repo().await,
                &bus,
                "imported/doc.txt",
                "x",
                "Import doc",
                WriteAnnouncement::SupersededBy("ArtifactImported"),
            )
            .await
            .unwrap();
        let after: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE event_type IN ('ArtifactCreated', 'ArtifactUpdated')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, 2, "a superseded write must not add an entity event");

        manager
            .delete_and_commit(
                &crate::core::repository_lock::lock_workspace_repo().await,
                &bus,
                "notes.md",
                "Delete notes",
            )
            .await
            .unwrap();
        let deleted: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'ArtifactDeleted'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(deleted, 1);

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn test_import_flow_auto_suffixes_and_preserves_existing() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        let dest1 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest1, "imported/Brev.pdf");
        manager.write_artifact(&dest1, "first").unwrap();

        let dest2 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest2, "imported/Brev (1).pdf");
        manager.write_artifact(&dest2, "second").unwrap();

        let dest3 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest3, "imported/Brev (2).pdf");
        manager.write_artifact(&dest3, "third").unwrap();

        assert_eq!(read_back(&manager, "imported/Brev.pdf"), "first");
        assert_eq!(read_back(&manager, "imported/Brev (1).pdf"), "second");
        assert_eq!(read_back(&manager, "imported/Brev (2).pdf"), "third");
    }

    #[tokio::test]
    async fn test_commit_data_paths_multiple_files() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let am = ArtifactManager::new(ws.to_path_buf()).unwrap();

        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::create_dir_all(ws.join("data/knowhow")).unwrap();
        std::fs::write(ws.join("data/artifacts/a.csv"), "col1\n1").unwrap();
        std::fs::write(ws.join("data/knowhow/b.md"), "# B").unwrap();

        let sha = am
            .commit_data_paths(
                &crate::core::repository_lock::lock_workspace_repo().await,
                &["artifacts/a.csv".to_string(), "knowhow/b.md".to_string()],
                "test: commit two data paths",
            )
            .await
            .unwrap();
        assert!(!sha.is_empty());
    }

    #[tokio::test]
    async fn deleting_a_data_folder_commits_every_file_in_it_gone() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let am = ArtifactManager::new(ws.to_path_buf()).unwrap();
        std::fs::create_dir_all(ws.join("data/hooks/gate/lib")).unwrap();
        std::fs::write(ws.join("data/hooks/gate/hook.toml"), "name = \"Gate\"").unwrap();
        std::fs::write(ws.join("data/hooks/gate/lib/util.py"), "").unwrap();
        std::fs::write(ws.join("data/hooks/keep.txt"), "").unwrap();
        let held = crate::core::repository_lock::lock_workspace_repo().await;
        let paths = [
            "hooks/gate/hook.toml",
            "hooks/gate/lib/util.py",
            "hooks/keep.txt",
        ];
        am.commit_data_paths(&held, &paths.map(String::from), "test: add a hook")
            .await
            .unwrap();
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        am.delete_data_dir_and_commit(
            &held,
            &bus,
            "hooks/gate",
            "test: remove a hook",
            hook_removed("gate"),
        )
        .await
        .unwrap();

        assert!(!ws.join("data/hooks/gate").exists());
        {
            let repo = am.repo.lock().unwrap();
            let tree = repo.head().unwrap().peel_to_tree().unwrap();
            assert!(tree.get_path(Path::new("data/hooks/gate")).is_err());
            assert!(tree.get_path(Path::new("data/hooks/keep.txt")).is_ok());
        }
        assert_eq!(hook_removals(&pool).await, 1);
        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    fn hook_removed(slug: &str) -> SystemEvent {
        SystemEvent::EventHookRemoved {
            slug: slug.to_string(),
            actor: None,
        }
    }

    async fn hook_removals(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'EventHookRemoved'")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deleting_a_file_or_link_in_place_of_a_data_folder_commits_it_gone() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let am = ArtifactManager::new(ws.to_path_buf()).unwrap();
        std::fs::create_dir_all(ws.join("data/hooks/real")).unwrap();
        std::fs::write(ws.join("data/hooks/real/hook.toml"), "").unwrap();
        std::fs::write(ws.join("data/hooks/notes.txt"), "").unwrap();
        std::os::unix::fs::symlink("real", ws.join("data/hooks/link")).unwrap();
        let held = crate::core::repository_lock::lock_workspace_repo().await;
        let paths = ["hooks/real/hook.toml", "hooks/notes.txt", "hooks/link"];
        am.commit_data_paths(&held, &paths.map(String::from), "test: add entries")
            .await
            .unwrap();
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        for (entry, slug) in [("hooks/notes.txt", "notes.txt"), ("hooks/link", "link")] {
            am.delete_data_dir_and_commit(
                &held,
                &bus,
                entry,
                "test: remove an entry",
                hook_removed(slug),
            )
            .await
            .unwrap();
        }

        assert!(ws.join("data/hooks/real/hook.toml").exists());
        {
            let repo = am.repo.lock().unwrap();
            let tree = repo.head().unwrap().peel_to_tree().unwrap();
            assert!(tree.get_path(Path::new("data/hooks/notes.txt")).is_err());
            assert!(tree.get_path(Path::new("data/hooks/link")).is_err());
            assert!(tree
                .get_path(Path::new("data/hooks/real/hook.toml"))
                .is_ok());
        }
        assert_eq!(hook_removals(&pool).await, 2);
        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn committing_a_gitignored_data_path_stages_nothing() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let am = ArtifactManager::new(ws.to_path_buf()).unwrap();

        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/.env"), "OPENAI_API_KEY=sk-secret").unwrap();
        std::fs::write(ws.join("data/artifacts/report.csv"), "col1\n1").unwrap();

        am.commit_data_paths(
            &crate::core::repository_lock::lock_workspace_repo().await,
            &[".env".to_string(), "artifacts/report.csv".to_string()],
            "test: a script's output beside a secret",
        )
        .await
        .unwrap();

        let repo = Repository::open(ws).unwrap();
        let tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(
            tree.get_path(Path::new("data/.env")).is_err(),
            "data/.env is gitignored and must never be committed"
        );
        assert!(
            tree.get_path(Path::new("data/artifacts/report.csv"))
                .is_ok(),
            "the script's real output must still commit"
        );
    }
}
