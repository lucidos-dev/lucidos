use chrono::{DateTime, Utc};
use git2::{ObjectType, Repository};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ARTIFACTS_DIR;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

/// Subdirectories under `data/` that browseable workspace tools (list_files,
/// glob_files, grep_files) walk. Anything outside these — postgres event
/// store, .lucidos/ runtime cache, etc. — is intentionally excluded.
pub const BROWSEABLE_DATA_SUBDIRS: &[&str] = &["artifacts", "apps", "knowhow", "triggers"];

/// Directory names holding vendored dependencies or build output. **The single
/// source of truth for that list**: anything that needs to tell the user's own
/// files apart from machine-generated ones reads this, so a second copy
/// somewhere else is a drift bug rather than a convenience.
///
/// There was nothing to reuse when this was written. The workspace has no
/// `.gitignore` parsing at all, and `WORKSPACE_GITIGNORE_ENTRIES` describes
/// what the workspace repo tracks (`.lucidos/`, `data/postgres/`,
/// `data/blobs/`, `data/.env`), not what a walk returns.
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

/// True when any DIRECTORY segment of `data_relative` (a path in the shape
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
/// answer: see `engine::chat::process::workspace_payload`.
pub fn is_vendored_path(data_relative: &str) -> bool {
    let Some((dirs, _file)) = data_relative.rsplit_once('/') else {
        return false;
    };
    dirs.split('/').any(|seg| VENDORED_DIR_NAMES.contains(&seg))
}

/// Walk the four browseable data/ subdirs and return each file as
/// `(data-relative path, absolute path)`, sorted lexicographically.
/// Shared by `ArtifactManager::list_artifacts` and the search tools so
/// the ignore policy stays consistent. Symlinks are intentionally skipped:
/// `entry.file_type()` doesn't follow them, which prevents both leaks
/// outside data/ and infinite recursion on symlink loops.
pub fn list_searchable_data_files(
    workspace_path: &Path,
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
        out: &mut Vec<(String, PathBuf)>,
    ) -> Result<(), std::io::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let path = entry.path();
            if ft.is_dir() {
                walk(&path, base, prefix, out)?;
            } else if ft.is_file() {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push((format!("{}/{}", prefix, rel.to_string_lossy()), path));
                }
            }
            // symlinks (and other special entries) are intentionally ignored
        }
        Ok(())
    }
    for sub in BROWSEABLE_DATA_SUBDIRS {
        let dir = data_path.join(sub);
        if dir.exists() {
            walk(&dir, &dir, sub, &mut out)?;
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

/// `Result`-returning wrapper around the canonical `super::is_path_traversal`
/// guard so the commit helpers can use `?` directly; `delete_data_path_and_commit`'s
/// `Box<dyn Error>` accepts the `git2::Error` via `From`.
fn reject_path_traversal(p: &str) -> Result<(), git2::Error> {
    if super::is_path_traversal(p) {
        return Err(git2::Error::from_str(&format!(
            "Path traversal not allowed: {}",
            p
        )));
    }
    Ok(())
}

/// What a `data/` write announces.
///
/// Every writer has to pick one. There is no "no event" option that is not
/// spelled out, which is the whole point: the failure this guards against is a
/// write whose announcement was simply forgotten, and an omission is not
/// expressible here.
pub enum WriteAnnouncement {
    /// The default. `ArtifactCreated` or `ArtifactUpdated`, decided from whether
    /// the file existed before this write. `source` labels who wrote it
    /// (`"http_request"`, `"run_python"`, …) or `None` for a plain write.
    Entity { source: Option<String> },
    /// The caller emits a RICHER event that stands in for the entity event on
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

/// The `data/` git store: writes a file under the workspace `data/` tree and
/// commits it.
///
/// **The write path owns the announcement.** [`Self::write_and_commit`] and
/// [`Self::delete_data_path_and_commit`] emit the matching entity event
/// themselves, so a caller cannot land a file in `data/artifacts/` that no list
/// refreshes on and the memory index never sees. That was not hypothetical: the
/// image tool wrote generated and saved images through the raw writer and
/// emitted nothing, so they appeared in no artifact list until a reload and
/// were never indexed.
///
/// It also moves the Created-vs-Updated decision to the one place that can get
/// it right. Every caller used to capture `artifact_exists` BEFORE its own
/// write and pass the flag to `SystemEvent::artifact_change`; that helper exists
/// only because the decision was duplicated five times.
///
/// See `core::announced_surfaces` for the registry entry and the exemptions.
pub struct ArtifactManager {
    workspace_path: PathBuf,
    repo: Arc<Mutex<Repository>>,
}

impl ArtifactManager {
    pub fn new(workspace_path: PathBuf) -> Result<Self, git2::Error> {
        let artifacts_path = workspace_path.join(ARTIFACTS_DIR);
        if let Err(e) = fs::create_dir_all(&artifacts_path) {
            log!("[Artifacts] Failed to create artifacts directory: {}", e);
        }

        let repo = match Repository::open(&workspace_path) {
            Ok(repo) => repo,
            Err(_) => {
                let mut opts = git2::RepositoryInitOptions::new();
                opts.initial_head("main");
                Repository::init_opts(&workspace_path, &opts)?
            }
        };

        // Ensure .gitignore exists and contains all engine-managed entries.
        // Idempotent across boots — appends new entries when the engine adds
        // them (e.g. data/blobs/) without rewriting historical lines. When
        // the file is newly created or modified, also commit it to the
        // artifacts repo so the workspace's git history records the change.
        //
        // Deliberately NOT routed through `retry_while_repo_contended`: this is
        // the constructor, so it runs at startup before this manager is shared
        // and before any timer or agent session exists to move HEAD under it.
        // It also stages outside any closure, and it logs-and-continues rather
        // than failing a request, so losing a race here costs a .gitignore
        // update that the next boot redoes rather than a 500.
        match super::ensure_workspace_gitignore_entries(&workspace_path) {
            Ok(true) => {
                let mut index = repo.index()?;
                index.add_path(Path::new(".gitignore"))?;
                index.write()?;
                if let Err(e) = super::commit_index(&repo, "chore: update .gitignore") {
                    log!("[Artifacts] Failed to commit .gitignore: {}", e);
                }
            }
            Ok(false) => {}
            Err(e) => log!("[Artifacts] Failed to ensure .gitignore entries: {}", e),
        }

        Ok(Self {
            workspace_path,
            repo: Arc::new(Mutex::new(repo)),
        })
    }

    /// The shared repo handle, for tests that need to stall a commit in flight.
    /// Every commit method below locks it, so holding it parks an in-flight
    /// snapshot exactly where two concurrent artifact commits park each other,
    /// with no test-only branch in the production path. Used by
    /// `a_timed_out_auto_commit_snapshot_still_excludes_a_concurrent_publish`
    /// to keep [`Self::commit_all_dirty`]'s blocking closure running after its
    /// caller has walked away.
    #[cfg(test)]
    pub(crate) fn repo_handle_for_test(&self) -> Arc<Mutex<Repository>> {
        self.repo.clone()
    }

    fn write_artifact(
        &self,
        relative_path: &str,
        content: impl AsRef<[u8]>,
    ) -> Result<PathBuf, std::io::Error> {
        // Defence-in-depth: API/data_api validates already, but artifact writes
        // also reach here from LLM tool handlers (`engine/tools/import.rs`,
        // `image.rs`, `http.rs`, `email.rs`, memory consumer) that pass paths
        // assembled from LLM-provided arguments. A `..` segment would otherwise
        // let the joined path escape `data/artifacts/`.
        reject_path_traversal(relative_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
        let full_path = self.workspace_path.join(ARTIFACTS_DIR).join(relative_path);

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&full_path, content)?;
        Ok(full_path)
    }

    /// Write an artifact, commit it, and announce it.
    ///
    /// Created-vs-Updated is decided from whether the file existed BEFORE this
    /// write, checked here so no caller has to sequence that itself. See
    /// [`WriteAnnouncement`] for the one case that suppresses the entity event.
    pub async fn write_and_commit(
        &self,
        event_bus: &EventBus,
        relative_path: &str,
        content: impl AsRef<[u8]>,
        message: &str,
        announcement: WriteAnnouncement,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let existed = self.artifact_exists(relative_path);
        self.write_artifact(relative_path, content)?;
        let commit_sha = self.commit(relative_path, message).await?;
        if let WriteAnnouncement::Entity { source } = announcement {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::artifact_change(
                        existed,
                        relative_path.to_string(),
                        commit_sha.clone(),
                        source,
                    )),
                    "[Artifacts] artifact write",
                )
                .await;
        }
        Ok(commit_sha)
    }

    /// Write multiple artifacts and commit them all in one operation
    pub async fn write_batch_and_commit(
        &self,
        files: &[(String, String)],
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        for (path, content) in files {
            self.write_artifact(path, content)?;
        }
        self.commit_batch(
            &files.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            message,
        )
        .await
        .map_err(Into::into)
    }

    /// Stage a single artifact file and commit.
    /// `artifact_path` is relative to data/artifacts/ (e.g., "user_profile.md").
    pub async fn commit(&self, artifact_path: &str, message: &str) -> Result<String, git2::Error> {
        reject_path_traversal(artifact_path)?;
        let repo_path = format!("{}/{}", ARTIFACTS_DIR, artifact_path);
        self.commit_repo_path(&repo_path, message).await
    }

    /// Stage multiple artifact files and commit in one operation (for batch imports).
    /// Each path is relative to data/artifacts/.
    pub async fn commit_batch(
        &self,
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
                super::reset_index_to_head(&repo, &mut index)?;

                for artifact_path in &paths {
                    let repo_path = format!("{}/{}", ARTIFACTS_DIR, artifact_path);
                    super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                }
                index.write()?;

                super::commit_index(&repo, &message)
            })
        })
        .await
        .unwrap()
    }

    /// Stage a data-relative path (e.g., "scripts/foo.py", "knowhow/bar.md") and commit.
    /// Path is relative to `data/` — any known subdirectory works.
    pub async fn commit_data_path(
        &self,
        data_relative_path: &str,
        message: &str,
    ) -> Result<String, git2::Error> {
        reject_path_traversal(data_relative_path)?;
        let repo_path = format!("data/{}", data_relative_path);
        self.commit_repo_path(&repo_path, message).await
    }

    /// Stage multiple data-relative paths and commit in one operation.
    /// Each path is relative to `data/` (e.g., "artifacts/report.csv", "knowhow/guide.md").
    pub async fn commit_data_paths(
        &self,
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
                super::reset_index_to_head(&repo, &mut index)?;

                for data_path in &paths {
                    let repo_path = format!("data/{}", data_path);
                    super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                }
                index.write()?;

                super::commit_index(&repo, &message)
            })
        })
        .await
        .unwrap()
    }

    /// Delete a data-relative file, commit the deletion, and announce it.
    ///
    /// Emits `ArtifactDeleted` for a path under `artifacts/`, which is the only
    /// `data/` subtree with a list that reloads on an entity event. Other
    /// subtrees commit silently, matching their registry classification.
    pub async fn delete_data_path_and_commit(
        &self,
        event_bus: &EventBus,
        data_relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(data_relative_path)?;
        let full_path = self.workspace_path.join("data").join(data_relative_path);
        fs::remove_file(&full_path)?;

        let repo = self.repo.clone();
        let repo_path = format!("data/{}", data_relative_path);
        let message = message.to_string();
        // The file removal above stays outside the retry closure, so a retried
        // attempt only re-stages an already-absent path onto the winner's head.
        // Staging is tolerant and the commit goes through
        // `commit_index_unless_unchanged`, because the writer that won the race
        // may have committed this deletion already.
        let commit_id = tokio::task::spawn_blocking(move || {
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                super::reset_index_to_head(&repo, &mut index)?;

                let _ = index.remove_path(std::path::Path::new(&repo_path));
                index.write()?;

                super::commit_index_unless_unchanged(&repo, &message)
            })
        })
        .await
        .unwrap()?;
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

    /// Stage a single repo-relative path (e.g., "data/artifacts/X" or "data/apps/Y") and commit.
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
                super::reset_index_to_head(&repo, &mut index)?;

                super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
                index.write()?;

                super::commit_index(&repo, &message)
            })
        })
        .await
        .unwrap()
    }

    /// Stage and commit all dirty files under data/ (artifacts, apps, etc.).
    /// Returns Some(commit_sha) if there were changes, None if clean.
    ///
    /// Holds `REPO_WORKTREE_MUTEX` for the real lifetime of the snapshot.
    /// Staging all of `data/` records anything missing from the working tree as a
    /// deletion, so a snapshot taken while a merge has published `main` but not
    /// yet synced the repo root commits the just-merged files as deleted, onto
    /// main. The guard is MOVED INTO the blocking closure rather than held around
    /// the `await`, because `spawn_blocking` cannot be cancelled: a caller whose
    /// future is dropped (the 30s ceiling in `Engine::commit_dirty_logged`, or
    /// any other cancellation) would otherwise free the exclusion with this
    /// closure still writing the index.
    ///
    /// The lock lives here rather than at the call site so no caller can forget
    /// it or scope it too narrowly, and for that reason a caller must NOT hold
    /// `REPO_WORKTREE_MUTEX` across this call. The single-path commit helpers
    /// above deliberately take no lock at all: they stage one named path onto the
    /// current head, so they cannot record an unrelated deletion.
    pub async fn commit_all_dirty(&self, message: &str) -> Result<Option<String>, git2::Error> {
        let worktree_guard = crate::engine::git_ops::lock_repo_worktree_owned().await;
        let repo = self.repo.clone();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            // Rebound so it is captured by the closure and dropped when the
            // closure returns, on every path out of it. That drop point is the
            // whole guarantee.
            let _worktree_guard = worktree_guard;
            // The worktree guard excludes the engine's OWN git-CLI paths, not a
            // git process outside this engine, so the index can still be locked
            // under us here, and HEAD can still move under us between the parent
            // read and the commit; see `retry_while_repo_contended`.
            super::retry_while_repo_contended(|| {
                let repo = repo.lock().unwrap();
                let mut index = repo.index()?;
                super::reset_index_to_head(&repo, &mut index)?;

                // Add all changes under data/ (artifacts, apps, and any other data subdirs)
                index.add_all([super::DATA_DIR], git2::IndexAddOption::DEFAULT, None)?;
                index.write()?;

                // Check if there's anything to commit
                let tree_id = index.write_tree()?;
                if let Ok(head) = repo.head() {
                    if let Ok(head_commit) = head.peel_to_commit() {
                        if head_commit.tree()?.id() == tree_id {
                            return Ok(None); // No changes
                        }
                    }
                }

                super::commit_index(&repo, &message).map(Some)
            })
        })
        .await
        .unwrap()
    }

    pub fn read_artifact(&self, relative_path: &str) -> Result<String, std::io::Error> {
        reject_path_traversal(relative_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
        let full_path = self.workspace_path.join(ARTIFACTS_DIR).join(relative_path);
        fs::read_to_string(full_path)
    }

    pub fn artifact_exists(&self, relative_path: &str) -> bool {
        if reject_path_traversal(relative_path).is_err() {
            return false;
        }
        let full_path = self.workspace_path.join(ARTIFACTS_DIR).join(relative_path);
        full_path.exists()
    }

    /// Resolve `relative_path` to the next non-colliding artifact path,
    /// Finder/browser style: if a file already exists at that path, insert a
    /// numeric suffix before the final extension (`Brev.pdf` → `Brev (1).pdf`
    /// → `Brev (2).pdf`; `README` → `README (1)`). Only the final extension is
    /// preserved, matching `Path::file_stem`/`Path::extension`
    /// (`archive.tar.gz` → `archive.tar (1).gz`). Returns the original path
    /// unchanged when there is no collision; never overwrites an existing file.
    ///
    /// Existence is checked against the real on-disk artifact path (the same
    /// root `write_artifact` writes to), so both import entry points stay
    /// consistent. The loop is bounded so a pathological directory can't spin
    /// forever — it errors instead of looping past `MAX_COLLISION_SUFFIX`.
    pub fn resolve_collision_free_path(
        &self,
        relative_path: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        /// Upper bound on suffix attempts. Far beyond any realistic number of
        /// same-name imports; exists only to make the search terminate.
        const MAX_COLLISION_SUFFIX: usize = 10_000;

        if !self.artifact_exists(relative_path) {
            return Ok(relative_path.to_string());
        }

        // Keep the directory prefix (with trailing '/') and split the leaf with
        // Path semantics so only the final extension is preserved.
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

    /// Delete an artifact and commit the deletion. Announces through
    /// [`Self::delete_data_path_and_commit`].
    pub async fn delete_and_commit(
        &self,
        event_bus: &EventBus,
        relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let data_relative = format!("artifacts/{}", relative_path);
        self.delete_data_path_and_commit(event_bus, &data_relative, message)
            .await
    }

    pub fn list_artifacts(&self) -> Result<Vec<String>, std::io::Error> {
        Ok(list_searchable_data_files(&self.workspace_path)?
            .into_iter()
            .map(|(rel, _)| rel)
            .collect())
    }

    /// Read an artifact file at a specific git commit
    /// Returns the file content as bytes, or an error if not found
    pub fn read_artifact_at_commit(
        &self,
        relative_path: &str,
        commit_hash: &str,
    ) -> Result<Vec<u8>, git2::Error> {
        reject_path_traversal(relative_path)?;
        let repo = self.repo.lock().unwrap();

        // Parse the commit hash
        let oid = git2::Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let tree = commit.tree()?;

        // Build the path within the repo (data/artifacts/relative_path)
        let full_path = format!("{}/{}", ARTIFACTS_DIR, relative_path);

        // Find the entry in the tree
        let entry = tree.get_path(Path::new(&full_path))?;

        // Get the blob
        let blob = repo.find_blob(entry.id())?;

        Ok(blob.content().to_vec())
    }

    /// Read an artifact file at a specific git commit as a string
    pub fn read_artifact_at_commit_string(
        &self,
        relative_path: &str,
        commit_hash: &str,
    ) -> Result<String, git2::Error> {
        let bytes = self.read_artifact_at_commit(relative_path, commit_hash)?;
        String::from_utf8(bytes).map_err(|_| git2::Error::from_str("File is not valid UTF-8"))
    }

    /// List artifacts at a specific git commit
    pub fn list_artifacts_at_commit(&self, commit_hash: &str) -> Result<Vec<String>, git2::Error> {
        let repo = self.repo.lock().unwrap();

        // Parse the commit hash
        let oid = git2::Oid::from_str(commit_hash)?;
        let commit = repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let mut paths = Vec::new();

        // Try to find the artifacts subtree
        if let Ok(artifacts_entry) = tree.get_path(Path::new(ARTIFACTS_DIR)) {
            if let Ok(artifacts_tree) = repo.find_tree(artifacts_entry.id()) {
                // Walk the artifacts tree recursively
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

    /// Find the most recent commit at or before a given Unix timestamp
    pub fn find_commit_at_timestamp(&self, timestamp: i64) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();

        // Get the HEAD commit and walk backwards
        let head = repo.head()?.peel_to_commit()?;
        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        for oid in revwalk {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let commit_time = commit.time().seconds();

            // Find the first commit that's at or before the target timestamp
            if commit_time <= timestamp {
                return Ok(oid.to_string());
            }
        }

        // If no commit found before timestamp, return the oldest commit
        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?;

        if let Some(Ok(oid)) = revwalk.next() {
            return Ok(oid.to_string());
        }

        Err(git2::Error::from_str("No commits found"))
    }

    /// Walk git history and collect all artifact file changes (adds/modifies) in chronological order.
    pub fn walk_artifact_history(&self) -> Result<Vec<ArtifactChange>, git2::Error> {
        let repo = self.repo.lock().unwrap();

        let head = match repo.head() {
            Ok(h) => h.peel_to_commit()?,
            Err(_) => return Ok(Vec::new()), // No commits yet
        };

        let mut revwalk = repo.revwalk()?;
        revwalk.push(head.id())?;
        revwalk.set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)?; // oldest first

        // Collect OIDs first to avoid borrow conflict with repo
        let oids: Vec<git2::Oid> = revwalk.filter_map(|r| r.ok()).collect();

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
                        return true; // skip deletes, renames, etc.
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

    #[test]
    fn test_write_and_read_artifact() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager.write_artifact("test.txt", "hello world").unwrap();
        let content = manager.read_artifact("test.txt").unwrap();

        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_commit_artifact() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager.write_artifact("test.txt", "hello").unwrap();
        let commit_sha = manager.commit("test.txt", "Add test file").await.unwrap();

        assert!(!commit_sha.is_empty());
    }

    #[test]
    fn test_resolve_collision_free_path_no_collision() {
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        // Nothing on disk yet — the path is returned unchanged.
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

        // First collision → "(1)".
        manager
            .write_artifact("imported/Brev.pdf", "first")
            .unwrap();
        assert_eq!(
            manager
                .resolve_collision_free_path("imported/Brev.pdf")
                .unwrap(),
            "imported/Brev (1).pdf"
        );

        // "(1)" also taken → "(2)".
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

        // Only the final extension is preserved, per Path semantics.
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

    /// Mirrors what `import_file_from_path` does: resolve a collision-free dest,
    /// then write+commit to it. Three same-name imports must land at the base
    /// name, "(1)", and "(2)" with every earlier file preserved untouched.
    /// The load-bearing guarantee for the `data/` half: a write announces, and
    /// Created-vs-Updated is decided from the state BEFORE the write.
    ///
    /// The second half is why the decision moved in here. Every caller used to
    /// capture `artifact_exists` itself and pass the flag along, which is easy
    /// to sequence wrong and easy to skip entirely: the image tool skipped it,
    /// so a generated image reached no artifact list and no memory index.
    #[tokio::test]
    async fn write_and_commit_announces_created_then_updated() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let dir = tempdir().unwrap();
        let manager = ArtifactManager::new(dir.path().to_path_buf()).unwrap();

        manager
            .write_and_commit(
                &bus,
                "notes.md",
                "one",
                "Write notes",
                WriteAnnouncement::Entity { source: None },
            )
            .await
            .unwrap();
        manager
            .write_and_commit(
                &bus,
                "notes.md",
                "two",
                "Rewrite notes",
                WriteAnnouncement::Entity {
                    source: Some("run_python".to_string()),
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

        // A caller with a richer event of its own suppresses the entity event
        // rather than adding to it, so an import is not double-announced.
        manager
            .write_and_commit(
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
            .delete_and_commit(&bus, "notes.md", "Delete notes")
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

        // First import: lands at the base name.
        let dest1 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest1, "imported/Brev.pdf");
        // The subject is collision-free naming, so the raw writer is enough:
        // the next `resolve_collision_free_path` only needs the file on disk.
        manager.write_artifact(&dest1, "first").unwrap();

        // Second import, same source name: auto-suffixed to "(1)".
        let dest2 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest2, "imported/Brev (1).pdf");
        manager.write_artifact(&dest2, "second").unwrap();

        // Third import: "(2)".
        let dest3 = manager
            .resolve_collision_free_path("imported/Brev.pdf")
            .unwrap();
        assert_eq!(dest3, "imported/Brev (2).pdf");
        manager.write_artifact(&dest3, "third").unwrap();

        // All three exist; earlier files were never overwritten.
        assert_eq!(manager.read_artifact("imported/Brev.pdf").unwrap(), "first");
        assert_eq!(
            manager.read_artifact("imported/Brev (1).pdf").unwrap(),
            "second"
        );
        assert_eq!(
            manager.read_artifact("imported/Brev (2).pdf").unwrap(),
            "third"
        );
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
                &["artifacts/a.csv".to_string(), "knowhow/b.md".to_string()],
                "test: commit two data paths",
            )
            .await
            .unwrap();
        assert!(!sha.is_empty());
    }

    /// `index.add_path` stages whatever it is handed, ignore rules included.
    /// So a `run_python` script writing `data/.env`, the documented route for
    /// a loose data-root file, put the workspace secrets into the tracked
    /// repo. The real output beside it must still commit.
    #[tokio::test]
    async fn committing_a_gitignored_data_path_stages_nothing() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let am = ArtifactManager::new(ws.to_path_buf()).unwrap();

        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/.env"), "OPENAI_API_KEY=sk-secret").unwrap();
        std::fs::write(ws.join("data/artifacts/report.csv"), "col1\n1").unwrap();

        am.commit_data_paths(
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
