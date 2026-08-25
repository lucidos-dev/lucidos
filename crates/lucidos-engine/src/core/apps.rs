use git2::Repository;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

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
    ///
    /// This handle's `Mutex` excludes only the other `AppManager` writes. Every
    /// other writer of the same repo (`ArtifactManager`, the plugin helpers, a
    /// coding agent's `git` CLI) races it, so the whole staging plus commit runs
    /// inside `retry_while_repo_contended`. The repo guard is taken INSIDE the
    /// closure so a retry re-stages onto a freshly reset index.
    pub fn commit(&self, app_path: &str, message: &str) -> Result<String, git2::Error> {
        super::retry_while_repo_contended(|| {
            let repo = self.repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;
            let repo_path = format!("data/apps/{}", app_path);
            index.add_path(Path::new(&repo_path))?;
            index.write()?;
            super::commit_index(&repo, message)
        })
    }

    /// Stage multiple app files and commit in one operation.
    pub fn commit_batch(&self, app_paths: &[String], message: &str) -> Result<String, git2::Error> {
        super::retry_while_repo_contended(|| {
            let repo = self.repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;
            for p in app_paths {
                let repo_path = format!("data/apps/{}", p);
                index.add_path(Path::new(&repo_path))?;
            }
            index.write()?;
            super::commit_index(&repo, message)
        })
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
    /// honest "would this id appear in the list?" check. It decides AppCreated
    /// vs AppUpdated when the raw file tools touch an app, and it is what
    /// `create_app` refuses on.
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

    /// Create an app on disk (manifest.json + index.html), commit it to git, and
    /// announce it.
    ///
    /// `AppCreated` is what puts the app in every client's list; the emit lives
    /// here rather than at the call site so a second creation path cannot ship
    /// an app nothing can see.
    ///
    /// **Creating is not overwriting.** An id that already exists is refused,
    /// because this call writes exactly two files. A second `create_app` for a
    /// live app would rewrite `index.html` and orphan everything else it grew:
    /// the extra pages, the scripts, the knowhow. That loss is silent, and most
    /// users cannot reach git history to undo it. `ArtifactManager` makes the
    /// same decision in its write path.
    pub async fn create_app(
        &self,
        event_bus: &EventBus,
        app_id: &str,
        name: &str,
        description: &str,
        html_content: &str,
    ) -> Result<(PathBuf, String), Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_id)?;
        if self.app_exists(app_id) {
            return Err(format!(
                "App '{}' already exists. Creating it again would rewrite index.html and \
                 orphan every other file in it. To change the app, edit its files with \
                 edit_file or write_file under data/apps/{}/ instead.",
                app_id, app_id
            )
            .into());
        }
        let app_dir = self.apps_path.join(app_id);
        std::fs::create_dir_all(&app_dir)?;

        let manifest = AppManifest {
            name: name.to_string(),
            description: description.to_string(),
            icon: None,
        };
        // The manifest lands LAST, because its presence is what `app_exists`
        // reads and what the guard above refuses on. A create that dies partway
        // leaves a directory that is not an app yet. The next attempt then
        // finishes it, instead of hitting "already exists".
        std::fs::write(app_dir.join("index.html"), html_content)?;
        std::fs::write(
            app_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;

        let commit = self.commit_batch(
            &[
                format!("{}/manifest.json", app_id),
                format!("{}/index.html", app_id),
            ],
            &format!("Create app: {}", name),
        )?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppCreated {
                    app_id: app_id.to_string(),
                    name: Some(name.to_string()),
                    actor: None,
                }),
                "[Apps] AppCreated",
            )
            .await;
        Ok((app_dir, commit))
    }

    /// Delete an app directory, commit to git, and announce it.
    pub async fn delete_app(
        &self,
        event_bus: &EventBus,
        app_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_id)?;
        let app_dir = self.apps_path.join(app_id);
        if !app_dir.exists() {
            return Err(format!("App not found: {}", app_id).into());
        }

        std::fs::remove_dir_all(&app_dir)?;

        // The closure keeps the repo guard and the git2 index (neither of which
        // is Send) out of the await below; otherwise this future stops being
        // Send and axum refuses the handler. The directory removal above stays
        // outside it, so a retried attempt only re-stages an already-absent
        // path onto the winner's head. Staging is tolerant and the commit goes
        // through `commit_index_unless_unchanged`, because the writer that won
        // the race may have committed this deletion already.
        let commit = super::retry_while_repo_contended(|| {
            let repo = self.repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;
            let _ = index.remove_dir(Path::new(&format!("data/apps/{}", app_id)), 0);
            index.write()?;
            let message = format!("Delete app: {}", app_id);
            super::commit_index_unless_unchanged(&repo, &message)
        })?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppDeleted {
                    app_id: app_id.to_string(),
                    actor,
                }),
                "[Apps] AppDeleted",
            )
            .await;
        Ok(commit)
    }

    /// Update an app's name and description in manifest.json (preserving icon),
    /// commit, and announce it.
    pub async fn update_app_metadata(
        &self,
        event_bus: &EventBus,
        app_id: &str,
        name: &str,
        description: &str,
        actor: Option<MessageOrigin>,
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
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppUpdated {
                    app_id: app_id.to_string(),
                    name: Some(name.to_string()),
                    actor,
                }),
                "[Apps] AppUpdated",
            )
            .await;
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

    /// Write app source files, commit to git, and announce the app changed.
    /// Validates each filename to reject path traversal and absolute paths.
    ///
    /// One `AppUpdated` per save, not per file: this is the editor's save
    /// button, and the agent's per-file writes go through the file tools, which
    /// coalesce into a single end-of-turn `AppUpdated` instead (see
    /// `engine/tools/files.rs::app_lifecycle_event`).
    pub async fn write_app_source(
        &self,
        event_bus: &EventBus,
        app_id: &str,
        files: &[(String, String)],
        actor: Option<MessageOrigin>,
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
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppUpdated {
                    app_id: app_id.to_string(),
                    name: self.app_name(app_id),
                    actor,
                }),
                "[Apps] AppUpdated",
            )
            .await;
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
    ///
    /// Deliberately silent, and registered as an exemption in
    /// `core::announced_surfaces`: removing one file is not an app lifecycle
    /// change. The caller (`engine/tools/files.rs`) decides whether the deletion
    /// killed the app, by checking whether it took `manifest.json` with it.
    pub fn delete_file_and_commit(
        &self,
        app_path: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        reject_path_traversal(app_path)?;
        let full_path = self.apps_path.join(app_path);
        std::fs::remove_file(&full_path)?;

        // The file removal above stays outside the retry closure, so a retried
        // attempt only re-stages an already-absent path onto the winner's head.
        // Staging is tolerant and the commit goes through
        // `commit_index_unless_unchanged`, because the writer that won the race
        // may have committed this deletion already.
        Ok(super::retry_while_repo_contended(|| {
            let repo = self.repo.lock().unwrap();
            let mut index = repo.index()?;
            super::reset_index_to_head(&repo, &mut index)?;
            let repo_path = format!("data/apps/{}", app_path);
            let _ = index.remove_path(Path::new(&repo_path));
            index.write()?;
            super::commit_index_unless_unchanged(&repo, message)
        })?)
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

    #[tokio::test]
    async fn path_validation_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();

        // Create the app directory so write_app_source doesn't fail on "not found"
        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();

        let bus = crate::test_support::offline_event_bus();
        let files = vec![("../etc/passwd".to_string(), "bad".to_string())];
        let result = manager
            .write_app_source(&bus, "test-app", &files, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));

        let files = vec![("foo/../../etc/passwd".to_string(), "bad".to_string())];
        let result = manager
            .write_app_source(&bus, "test-app", &files, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));
    }

    /// The app id itself must also be traversal-guarded: `create_app` is
    /// reached from the LLM tool handler with a model-provided id, and an
    /// unchecked `../…` id would create (or, via delete_app, remove) files
    /// outside `data/apps/`.
    #[tokio::test]
    async fn path_validation_rejects_traversal_in_app_id() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();

        let bus = crate::test_support::offline_event_bus();
        let result = manager
            .create_app(&bus, "../escaped", "Evil", "", "<html></html>")
            .await;
        assert!(result.is_err());
        assert!(!tmp.path().join("data/escaped").exists());

        assert!(manager.delete_app(&bus, "../..", None).await.is_err());
        assert!(manager.get_app("../..").is_err());
        assert!(!manager.app_exists("../.."));
    }

    #[tokio::test]
    async fn path_validation_rejects_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let manager = AppManager::new(ws).unwrap();

        let app_dir = ws.join("data/apps/test-app");
        std::fs::create_dir_all(&app_dir).unwrap();

        let bus = crate::test_support::offline_event_bus();
        let files = vec![("/etc/passwd".to_string(), "bad".to_string())];
        let result = manager
            .write_app_source(&bus, "test-app", &files, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));

        let files = vec![(
            "\\Windows\\System32\\evil.dll".to_string(),
            "bad".to_string(),
        )];
        let result = manager
            .write_app_source(&bus, "test-app", &files, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid filename"));
    }

    // --- re-creating a live app (the silent truncation) -------------------

    /// `AppCreated` is a transient event, so it reaches subscribers and never
    /// the events table. Counting it means draining the bus.
    fn app_created_ids(
        rx: &mut tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
    ) -> Vec<String> {
        let mut ids = Vec::new();
        while let Ok(emitted) = rx.try_recv() {
            if let BusEvent::System(SystemEvent::AppCreated { app_id, .. }) = emitted.typed {
                ids.push(app_id);
            }
        }
        ids
    }

    /// A model re-issuing `create_app` for a live id used to rewrite
    /// `index.html` and orphan every other file the app had grown. The refusal
    /// keeps the app whole, and names the tool that changes it instead.
    #[tokio::test]
    async fn create_app_refuses_an_id_that_already_exists() {
        let bus = crate::test_support::offline_event_bus();
        let mut rx = bus.subscribe();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();

        manager
            .create_app(&bus, "habit-tracker", "Habit Tracker", "Habits", "<h1>v1")
            .await
            .expect("first create");

        // The app grows a second page the way a real one does.
        let app_dir = tmp.path().join("data/apps/habit-tracker");
        let sibling = app_dir.join("stats.html");
        std::fs::write(&sibling, "<h1>stats").unwrap();

        let err = manager
            .create_app(&bus, "habit-tracker", "Habit Tracker", "Habits", "<h1>v2")
            .await
            .expect_err("re-creating a live app must be refused");
        let err = err.to_string();
        assert!(err.contains("habit-tracker"), "names the id: {err}");
        assert!(err.contains("already exists"), "got: {err}");
        assert!(err.contains("edit_file"), "names the recovery: {err}");

        assert_eq!(
            std::fs::read_to_string(app_dir.join("index.html")).unwrap(),
            "<h1>v1",
            "the live index.html must survive"
        );
        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "<h1>stats",
            "the sibling page must survive"
        );
        assert_eq!(
            app_created_ids(&mut rx),
            vec!["habit-tracker".to_string()],
            "the refusal path must not announce a second creation"
        );
    }

    /// "Exists" means what the apps list means: a readable `manifest.json`. A
    /// bare directory laid down by `write_file` is not an app yet, so
    /// `create_app` still finishes it.
    #[tokio::test]
    async fn create_app_still_finishes_a_manifest_less_directory() {
        let bus = crate::test_support::offline_event_bus();
        let mut rx = bus.subscribe();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();

        let app_dir = tmp.path().join("data/apps/half-built");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("notes.md"), "scaffolding").unwrap();
        assert!(!manager.app_exists("half-built"));

        manager
            .create_app(&bus, "half-built", "Half Built", "", "<h1>done")
            .await
            .expect("a directory without a manifest is not an app yet");

        assert!(manager.app_exists("half-built"));
        assert_eq!(app_created_ids(&mut rx), vec!["half-built".to_string()]);
        assert!(app_dir.join("notes.md").exists(), "scaffolding survives");
    }

    /// A create that dies before the manifest stays retryable. The manifest is
    /// the marker the guard reads, so writing it last is what keeps a
    /// half-written app finishable.
    #[tokio::test]
    async fn create_app_can_be_retried_after_a_failed_attempt() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();

        // A directory sitting where index.html goes fails the first write.
        let app_dir = tmp.path().join("data/apps/habit-tracker");
        std::fs::create_dir_all(app_dir.join("index.html")).unwrap();

        manager
            .create_app(&bus, "habit-tracker", "Habit Tracker", "Habits", "<h1>v1")
            .await
            .expect_err("writing index.html over a directory must fail");
        assert!(
            !manager.app_exists("habit-tracker"),
            "a failed create must not mark the app as existing"
        );

        std::fs::remove_dir(app_dir.join("index.html")).unwrap();
        manager
            .create_app(&bus, "habit-tracker", "Habit Tracker", "Habits", "<h1>v1")
            .await
            .expect("the retry must finish the app");
        assert!(manager.app_exists("habit-tracker"));
    }
}
