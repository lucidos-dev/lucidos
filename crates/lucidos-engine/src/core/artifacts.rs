use chrono::{DateTime, Utc};
use git2::{ObjectType, Repository};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ARTIFACTS_DIR;

/// Subdirectories under `data/` that browseable workspace tools (list_files,
/// glob_files, grep_files) walk. Anything outside these — postgres event
/// store, .lucidos/ runtime cache, etc. — is intentionally excluded.
pub const BROWSEABLE_DATA_SUBDIRS: &[&str] = &["artifacts", "apps", "knowhow", "triggers"];

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

pub struct ArtifactManager {
    workspace_path: PathBuf,
    repo: Arc<Mutex<Repository>>,
}

impl ArtifactManager {
    /// Resolve a relative artifact path to its full filesystem path.
    pub fn artifact_path(&self, relative_path: &str) -> PathBuf {
        self.workspace_path.join(ARTIFACTS_DIR).join(relative_path)
    }

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

    /// Write artifact and commit in one operation
    pub async fn write_and_commit(
        &self,
        relative_path: &str,
        content: impl AsRef<[u8]>,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.write_artifact(relative_path, content)?;
        let commit_sha = self.commit(relative_path, message).await?;
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
            let repo = repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;

            for artifact_path in &paths {
                let repo_path = format!("{}/{}", ARTIFACTS_DIR, artifact_path);
                index.add_path(Path::new(&repo_path))?;
            }
            index.write()?;

            super::commit_index(&repo, &message)
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
            let repo = repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;

            for data_path in &paths {
                let repo_path = format!("data/{}", data_path);
                index.add_path(Path::new(&repo_path))?;
            }
            index.write()?;

            super::commit_index(&repo, &message)
        })
        .await
        .unwrap()
    }

    /// Delete a data-relative file and commit the deletion.
    pub async fn delete_data_path_and_commit(
        &self,
        data_relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(data_relative_path)?;
        let full_path = self.workspace_path.join("data").join(data_relative_path);
        fs::remove_file(&full_path)?;

        let repo = self.repo.clone();
        let repo_path = format!("data/{}", data_relative_path);
        let message = message.to_string();
        let commit_id = tokio::task::spawn_blocking(move || -> Result<String, git2::Error> {
            let repo = repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;

            index.remove_path(std::path::Path::new(&repo_path))?;
            index.write()?;

            super::commit_index(&repo, &message)
        })
        .await
        .unwrap()?;
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
            let repo = repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;

            index.add_path(Path::new(&repo_path))?;
            index.write()?;

            super::commit_index(&repo, &message)
        })
        .await
        .unwrap()
    }

    /// Stage and commit all dirty files under data/ (artifacts, apps, etc.).
    /// Returns Some(commit_sha) if there were changes, None if clean.
    pub async fn commit_all_dirty(&self, message: &str) -> Result<Option<String>, git2::Error> {
        let repo = self.repo.clone();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
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

    /// Delete artifact and commit the deletion
    pub async fn delete_and_commit(
        &self,
        relative_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let data_relative = format!("artifacts/{}", relative_path);
        self.delete_data_path_and_commit(&data_relative, message)
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
}
