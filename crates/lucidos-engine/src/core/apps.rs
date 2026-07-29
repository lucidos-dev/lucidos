use git2::Repository;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Defence-in-depth: API handlers validate app ids at the boundary
/// (`is_valid_id` in `api/apps.rs`), but `AppManager` is also reached from LLM
/// tool handlers (`engine/tools/apps.rs` passes the model-provided `id`
/// straight through). A `..` segment or absolute path would let the joined
/// path escape `data/apps/` — mirror of the guard in
/// `ArtifactManager::write_artifact`.
fn reject_path_traversal(p: &str) -> Result<(), std::io::Error> {
    if super::is_path_traversal(p) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Path traversal not allowed: {}", p),
        ));
    }
    Ok(())
}

pub struct AppManager {
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
            apps_path,
            repo: Mutex::new(repo),
        })
    }

    /// Stage a single app file and commit.
    /// `app_path` is relative to data/apps/ (e.g., "my-app/index.html").
    pub fn commit(&self, app_path: &str, message: &str) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        super::reset_index_to_head(&repo, &mut index)?;
        let repo_path = format!("data/apps/{}", app_path);
        index.add_path(Path::new(&repo_path))?;
        index.write()?;
        super::commit_index(&repo, message)
    }

    /// Stage multiple app files and commit in one operation.
    pub fn commit_batch(&self, app_paths: &[String], message: &str) -> Result<String, git2::Error> {
        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        super::reset_index_to_head(&repo, &mut index)?;
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
        reject_path_traversal(app_id)?;
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("App not found: {}", app_id),
            ));
        }
        self.load_app(&app_dir)
    }

    /// Whether the app exists as far as the apps list is concerned: a
    /// `manifest.json` is present under `data/apps/<id>/`. Matches `list_apps`,
    /// which skips directories without a readable manifest — so this is the
    /// honest "would this id appear in the list?" check used to decide
    /// AppCreated vs AppUpdated when the raw file tools touch an app.
    pub fn app_exists(&self, app_id: &str) -> bool {
        if reject_path_traversal(app_id).is_err() {
            return false;
        }
        self.apps_path.join(app_id).join("manifest.json").exists()
    }

    /// The app's display name from its manifest, or `None` if it can't be read.
    /// Used to populate the `name` field on `AppCreated`/`AppUpdated`.
    pub fn app_name(&self, app_id: &str) -> Option<String> {
        self.get_app(app_id).ok().map(|a| a.name)
    }

    /// Create a new app directory with manifest.json + index.html and commit to git.
    pub fn create_app(
        &self,
        app_id: &str,
        name: &str,
        description: &str,
        html_content: &str,
    ) -> Result<(PathBuf, String), Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_id)?;
        let app_dir = self.apps_path.join(app_id);
        std::fs::create_dir_all(&app_dir)?;

        let manifest = AppManifest {
            name: name.to_string(),
            description: description.to_string(),
            icon: None,
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
        reject_path_traversal(app_id)?;
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        std::fs::remove_dir_all(&app_dir)?;

        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        super::reset_index_to_head(&repo, &mut index)?;
        index.remove_dir(Path::new(&format!("data/apps/{}", app_id)), 0)?;
        index.write()?;

        let message = format!("Delete app: {}", app_id);
        Ok(super::commit_index(&repo, &message)?)
    }

    /// Update an app's name and description in manifest.json, preserving icon.
    pub fn update_app_metadata(
        &self,
        app_id: &str,
        name: &str,
        description: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_id)?;
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
        reject_path_traversal(app_id)?;
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
            let content = std::fs::read_to_string(&file_path)?;
            result.push((name, content));
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
        reject_path_traversal(app_id)?;
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        let mut git_paths = Vec::new();
        for (name, content) in files {
            if super::is_path_traversal(name) {
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

    /// Delete a single file from an app and commit.
    /// `app_path` is relative to data/apps/ (e.g., "my-app/old-file.js").
    pub fn delete_file_and_commit(
        &self,
        app_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_path)?;
        let full_path = self.apps_path.join(app_path);
        std::fs::remove_file(&full_path)?;

        let repo = self.repo.lock().unwrap();
        let mut index = repo.index()?;
        super::reset_index_to_head(&repo, &mut index)?;
        let repo_path = format!("data/apps/{}", app_path);
        index.remove_path(Path::new(&repo_path))?;
        index.write()?;
        Ok(super::commit_index(&repo, message)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_manifest_deserializes() {
        let json = r#"{
            "name": "Varmepumpe Dashboard",
            "description": "Heat pump monitoring and control",
            "icon": "thermometer"
        }"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "Varmepumpe Dashboard");
        assert_eq!(manifest.description, "Heat pump monitoring and control");
        assert_eq!(manifest.icon.as_deref(), Some("thermometer"));
    }

    #[test]
    fn app_manifest_ignores_legacy_knowhow_field() {
        // Manifests stamped by the old know-how pass still carry a `knowhow`
        // array on disk. The field is no longer part of AppManifest; serde must
        // ignore the unknown key rather than fail to deserialize, so existing
        // apps keep loading (and the stale field drops on the next rewrite).
        let json = r#"{
            "name": "Legacy App",
            "description": "Has a stamped knowhow array",
            "knowhow": ["oura/api-ref", "browser-learning/observation"]
        }"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "Legacy App");
        assert_eq!(manifest.description, "Has a stamped knowhow array");
    }

    #[test]
    fn app_manifest_defaults() {
        let json = r#"{"name": "Minimal App"}"#;
        let manifest: AppManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "Minimal App");
        assert_eq!(manifest.description, "");
        assert!(manifest.icon.is_none());
    }

    #[test]
    fn app_manifest_round_trip() {
        let manifest = AppManifest {
            name: "Test App".to_string(),
            description: "A test application".to_string(),
            icon: Some("star".to_string()),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: AppManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.description, manifest.description);
        assert_eq!(deserialized.icon, manifest.icon);
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

    /// The app id itself must also be traversal-guarded: `create_app` is
    /// reached from the LLM tool handler with a model-provided id, and an
    /// unchecked `../…` id would create (or, via delete_app, remove) files
    /// outside `data/apps/`.
    #[test]
    fn path_validation_rejects_traversal_in_app_id() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();

        let result = manager.create_app("../escaped", "Evil", "", "<html></html>");
        assert!(result.is_err());
        assert!(!tmp.path().join("data/escaped").exists());

        assert!(manager.delete_app("../..").is_err());
        assert!(manager.get_app("../..").is_err());
        assert!(!manager.app_exists("../.."));
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
