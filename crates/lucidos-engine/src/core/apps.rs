use git2::Repository;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::memory::EmbeddingProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowhow: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowhow: Vec<String>,
}

pub struct AppManager {
    workspace_path: PathBuf,
    apps_path: PathBuf,
    repo: Mutex<Repository>,
}

impl AppManager {
    pub fn new(workspace_path: &Path) -> Result<Self, git2::Error> {
        let apps_path = workspace_path.join("data/apps");
        if let Err(e) = std::fs::create_dir_all(&apps_path) {
            log!(
                "[Apps] Failed to create apps directory {}: {}",
                apps_path.display(),
                e
            );
        }

        let repo = match Repository::open(workspace_path) {
            Ok(repo) => repo,
            Err(_) => Repository::init(workspace_path)?,
        };

        Ok(Self {
            workspace_path: workspace_path.to_path_buf(),
            apps_path,
            repo: Mutex::new(repo),
        })
    }

    /// Reset the index to match HEAD tree, avoiding stale staged entries
    /// from previous operations polluting this commit.
    fn reset_index_to_head(repo: &Repository, index: &mut git2::Index) -> Result<(), git2::Error> {
        if let Ok(head) = repo.head() {
            if let Ok(tree) = head.peel_to_tree() {
                index.read_tree(&tree)?;
            }
        }
        Ok(())
    }

    /// Stage a single app file and commit.
    /// `app_path` is relative to data/apps/ (e.g., "my-app/index.html").
    pub fn commit(&self, app_path: &str, message: &str) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        Self::reset_index_to_head(&repo, &mut index)?;
        let repo_path = format!("data/apps/{}", app_path);
        index.add_path(Path::new(&repo_path))?;
        index.write()?;
        super::commit_index(&repo, message)
    }

    /// Stage multiple app files and commit in one operation.
    pub fn commit_batch(&self, app_paths: &[String], message: &str) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        Self::reset_index_to_head(&repo, &mut index)?;
        for p in app_paths {
            let repo_path = format!("data/apps/{}", p);
            index.add_path(Path::new(&repo_path))?;
        }
        index.write()?;
        super::commit_index(&repo, message)
    }

    /// Load an App from its manifest.json on disk.
    fn load_app(&self, app_dir: &Path) -> Result<App, std::io::Error> {
        let id = app_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let manifest_path = app_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No manifest.json in app: {}", id),
            ));
        }

        let content = std::fs::read_to_string(&manifest_path)?;
        let manifest: AppManifest = serde_json::from_str(&content).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid manifest.json in app {}: {}", id, e),
            )
        })?;

        Ok(App {
            id,
            name: manifest.name,
            description: manifest.description,
            icon: manifest.icon,
            knowhow: manifest.knowhow,
        })
    }

    /// List all apps in the workspace.
    pub fn list_apps(&self) -> Result<Vec<App>, std::io::Error> {
        let mut apps = Vec::new();

        if !self.apps_path.exists() {
            return Ok(apps);
        }

        for entry in std::fs::read_dir(&self.apps_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                match self.load_app(&path) {
                    Ok(app) => apps.push(app),
                    Err(e) => {
                        log!("[Apps] Skipping {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(apps)
    }

    /// Get a specific app by ID.
    pub fn get_app(&self, app_id: &str) -> Result<App, std::io::Error> {
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("App not found: {}", app_id),
            ));
        }
        self.load_app(&app_dir)
    }

    /// Create a new app directory with manifest.json + index.html and commit to git.
    pub fn create_app(
        &self,
        app_id: &str,
        name: &str,
        description: &str,
        html_content: &str,
    ) -> Result<(PathBuf, String), Box<dyn std::error::Error + Send + Sync>> {
        let app_dir = self.apps_path.join(app_id);
        std::fs::create_dir_all(&app_dir)?;

        let manifest = AppManifest {
            name: name.to_string(),
            description: description.to_string(),
            icon: None,
            knowhow: Vec::new(),
        };
        std::fs::write(
            app_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;
        std::fs::write(app_dir.join("index.html"), html_content)?;

        let commit = self.commit_batch(
            &[
                format!("{}/manifest.json", app_id),
                format!("{}/index.html", app_id),
            ],
            &format!("Create app: {}", name),
        )?;
        Ok((app_dir, commit))
    }

    /// Delete an app directory and commit to git.
    pub fn delete_app(
        &self,
        app_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        std::fs::remove_dir_all(&app_dir)?;

        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        Self::reset_index_to_head(&repo, &mut index)?;
        index.remove_dir(Path::new(&format!("data/apps/{}", app_id)), 0)?;
        index.write()?;

        let message = format!("Delete app: {}", app_id);
        Ok(super::commit_index(&repo, &message)?)
    }

    /// Update an app's name and description in manifest.json, preserving icon and knowhow.
    pub fn update_app_metadata(
        &self,
        app_id: &str,
        name: &str,
        description: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let manifest_path = self.apps_path.join(app_id).join("manifest.json");
        if !manifest_path.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        let existing: AppManifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
        let manifest = AppManifest {
            name: name.to_string(),
            description: description.to_string(),
            icon: existing.icon,
            knowhow: existing.knowhow,
        };
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

        let commit = self.commit(
            &format!("{}/manifest.json", app_id),
            &format!("Update app metadata: {}", name),
        )?;
        Ok(commit)
    }

    /// Read all editable text files in an app, returning (name, content) pairs.
    /// Skips manifest.json (metadata) and binary files. Sorts with index.html first.
    pub fn read_app_source(
        &self,
        app_id: &str,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        let file_names = self.list_app_files(app_id);
        let mut result = Vec::new();
        for name in file_names {
            if name == "manifest.json" {
                continue;
            }
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !matches!(
                ext,
                "html" | "htm" | "css" | "js" | "ts" | "json" | "md" | "txt" | "svg"
            ) {
                continue;
            }
            let file_path = app_dir.join(&name);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                result.push((name, content));
            }
        }
        result.sort_by(|a, b| {
            if a.0 == "index.html" {
                std::cmp::Ordering::Less
            } else if b.0 == "index.html" {
                std::cmp::Ordering::Greater
            } else {
                a.0.cmp(&b.0)
            }
        });
        Ok(result)
    }

    /// Write app source files and commit to git.
    /// Validates each filename to reject path traversal and absolute paths.
    pub fn write_app_source(
        &self,
        app_id: &str,
        files: &[(String, String)],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        let mut git_paths = Vec::new();
        for (name, content) in files {
            if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
                return Err(format!("Invalid filename: {}", name).into());
            }
            let file_path = app_dir.join(name);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file_path, content)?;
            git_paths.push(format!("{}/{}", app_id, name));
        }

        let commit = self.commit_batch(&git_paths, &format!("Edit app: {}", app_id))?;
        Ok(commit)
    }

    /// Get the path to an app's index.html.
    pub fn get_app_path(&self, app_id: &str) -> PathBuf {
        self.apps_path.join(app_id).join("index.html")
    }

    /// Recursively list all files in an app directory.
    pub fn list_app_files(&self, app_id: &str) -> Vec<String> {
        let app_dir = self.apps_path.join(app_id);
        let mut files = Vec::new();

        if !app_dir.exists() {
            return files;
        }

        fn walk(dir: &Path, base: &Path, files: &mut Vec<String>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, base, files);
                    } else if let Ok(relative) = path.strip_prefix(base) {
                        files.push(relative.to_string_lossy().to_string());
                    }
                }
            }
        }

        walk(&app_dir, &app_dir, &mut files);
        files
    }

    /// Get the workspace path.
    pub fn workspace_path(&self) -> &Path {
        &self.workspace_path
    }

    /// Get the apps directory path.
    pub fn apps_path(&self) -> &Path {
        &self.apps_path
    }

    /// Delete a single file from an app and commit.
    /// `app_path` is relative to data/apps/ (e.g., "my-app/old-file.js").
    pub fn delete_file_and_commit(
        &self,
        app_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let full_path = self.apps_path.join(app_path);
        std::fs::remove_file(&full_path)?;

        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        Self::reset_index_to_head(&repo, &mut index)?;
        let repo_path = format!("data/apps/{}", app_path);
        index.remove_path(Path::new(&repo_path))?;
        index.write()?;
        Ok(super::commit_index(&repo, message)?)
    }

    /// Stamp know-how references into an app's manifest.json.
    /// Discovers relevant know-how from the app's HTML content and explicit references
    /// using semantic similarity, then writes the IDs into the manifest's `knowhow` field.
    pub async fn stamp_knowhow(
        &self,
        app_id: &str,
        summaries: &[super::knowhow::KnowhowSummary],
        explicit_refs: &[String],
        embedder: &dyn EmbeddingProvider,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let manifest_path = self.apps_path.join(app_id).join("manifest.json");
        if !manifest_path.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        // Read current manifest
        let mut manifest: AppManifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;

        // Read app HTML content for semantic discovery
        let index_path = self.apps_path.join(app_id).join("index.html");
        let html_content = std::fs::read_to_string(&index_path).unwrap_or_default();

        // Discover know-how semantically from content + explicit refs
        let discovered = super::knowhow::discover_knowhow(
            &format!("{} {}", manifest.name, html_content),
            summaries,
            explicit_refs,
            embedder,
        )
        .await;

        if discovered == manifest.knowhow {
            return Ok(()); // No change needed
        }

        manifest.knowhow = discovered;
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

        // Commit the updated manifest
        self.commit(
            &format!("{}/manifest.json", app_id),
            &format!("Stamp know-how refs for app: {}", app_id),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_provider() -> &'static dyn EmbeddingProvider {
        crate::test_util::shared_embedder()
    }

    #[test]
    fn app_manifest_deserializes() {
        let json = r#"{
            "name": "Varmepumpe Dashboard",
            "description": "Heat pump monitoring and control",
            "icon": "thermometer",
            "knowhow": ["heatpump"]
        }"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "Varmepumpe Dashboard");
        assert_eq!(manifest.description, "Heat pump monitoring and control");
        assert_eq!(manifest.icon.as_deref(), Some("thermometer"));
        assert_eq!(manifest.knowhow, vec!["heatpump"]);
    }

    #[test]
    fn app_manifest_defaults() {
        let json = r#"{"name": "Minimal App"}"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "Minimal App");
        assert_eq!(manifest.description, "");
        assert!(manifest.icon.is_none());
        assert!(manifest.knowhow.is_empty());
    }

    #[test]
    fn app_manifest_round_trip() {
        let manifest = AppManifest {
            name: "Test App".to_string(),
            description: "A test application".to_string(),
            icon: Some("star".to_string()),
            knowhow: vec!["topic-a".to_string(), "topic-b".to_string()],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: AppManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.description, manifest.description);
        assert_eq!(deserialized.icon, manifest.icon);
        assert_eq!(deserialized.knowhow, manifest.knowhow);
    }

    #[test]
    fn path_validation_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();

        // Create the app directory so write_app_source doesn't fail on "not found"
        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();

        let files = vec![("../etc/passwd".to_string(), "bad".to_string())];
        let result = manager.write_app_source("test-app", &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));

        let files = vec![("foo/../../etc/passwd".to_string(), "bad".to_string())];
        let result = manager.write_app_source("test-app", &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));
    }

    #[tokio::test]
    async fn stamp_knowhow_discovers_from_content() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();
        let embedder = shared_provider();

        // Create a test app with heatpump-related HTML
        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let manifest = AppManifest {
            name: "Heatpump Dashboard".to_string(),
            description: "Monitor heatpump".to_string(),
            icon: None,
            knowhow: vec![],
        };
        std::fs::write(
            app_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(app_dir.join("index.html"), "<h1>Heatpump Status</h1>").unwrap();

        // Create a know-how file with heatpump description
        let kh_dir = ws.join("data/knowhow");
        std::fs::create_dir_all(&kh_dir).unwrap();
        std::fs::write(
            kh_dir.join("heatpump.md"),
            "---\nname: Panasonic Heatpump\ndescription: Controls and monitors Panasonic heatpumps via Comfort Cloud API\n---\nAPI docs...",
        ).unwrap();

        // Stamp know-how
        let summaries = crate::core::KnowhowStore::load_summaries(&kh_dir);
        let result = manager
            .stamp_knowhow("test-app", &summaries, &[], embedder)
            .await;
        assert!(result.is_ok());

        // Verify manifest was updated
        let updated: AppManifest =
            serde_json::from_str(&std::fs::read_to_string(app_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert!(updated.knowhow.contains(&"heatpump".to_string()));
    }

    #[tokio::test]
    async fn stamp_knowhow_includes_explicit_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();
        let embedder = shared_provider();

        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let manifest = AppManifest {
            name: "Generic App".to_string(),
            description: "".to_string(),
            icon: None,
            knowhow: vec![],
        };
        std::fs::write(
            app_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(app_dir.join("index.html"), "<h1>Hello</h1>").unwrap();

        let kh_dir = ws.join("data/knowhow");
        std::fs::create_dir_all(&kh_dir).unwrap();
        std::fs::write(
            kh_dir.join("oura.md"),
            "---\nname: Oura Ring\ndescription: Oura Ring sleep and activity tracking API\n---\nOura API...",
        ).unwrap();

        let summaries = crate::core::KnowhowStore::load_summaries(&kh_dir);
        let result = manager
            .stamp_knowhow("test-app", &summaries, &["oura".to_string()], embedder)
            .await;
        assert!(result.is_ok());

        let updated: AppManifest =
            serde_json::from_str(&std::fs::read_to_string(app_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(updated.knowhow, vec!["oura"]);
    }

    #[test]
    fn path_validation_rejects_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();

        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();

        let files = vec![("/etc/passwd".to_string(), "bad".to_string())];
        let result = manager.write_app_source("test-app", &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));

        let files = vec![(
            "\\Windows\\System32\\evil.dll".to_string(),
            "bad".to_string(),
        )];
        let result = manager.write_app_source("test-app", &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));
    }
}
