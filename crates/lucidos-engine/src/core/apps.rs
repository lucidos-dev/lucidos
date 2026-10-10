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
    /// Kept verbatim, so a metadata rewrite never alters what the author
    /// wrote. `AppReveal::from_manifest` reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<String>,
    /// Kept verbatim like `reveal`. `AppKind::from_manifest` reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The thread a widget was made in. Kept verbatim; `App` parses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_thread_id: Option<String>,
    /// A reusable widget is offered to every thread (ADR 0402).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reusable: bool,
}

/// What an app folder is. The one classifier every apps reader filters
/// through, so a widget never reaches the apps list (ADR 0402).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppKind {
    #[default]
    App,
    /// A visual answer that lives in a thread: an app that is not listed.
    Widget,
}

impl AppKind {
    /// An unrecognised value lists the app as an app rather than failing the
    /// manifest. Hiding a folder on a typo would lose it; the audit reports it.
    pub(crate) fn from_manifest(app_id: &str, value: Option<&str>) -> Self {
        match value {
            None | Some("app") => Self::App,
            Some("widget") => Self::Widget,
            Some(other) => {
                log!(
                    "[Apps] {}: unrecognised manifest kind {:?}, listing it as an app",
                    app_id,
                    other
                );
                Self::App
            }
        }
    }

    fn manifest_value(self) -> Option<String> {
        match self {
            Self::App => None,
            Self::Widget => Some("widget".to_string()),
        }
    }
}

/// What `create_app` makes. A widget always records the thread it was made
/// in, so a widget without an origin cannot be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewAppKind {
    App,
    Widget { origin_thread_id: uuid::Uuid },
}

/// When the host lifts its cover off an opening app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppReveal {
    /// On the frame's `load` event.
    #[default]
    OnLoad,
    /// When the app calls `lucidos.ui.ready()`.
    OnReady,
}

impl AppReveal {
    /// An unrecognised value opens the app as `on-load` rather than failing
    /// the manifest: a typo must not drop the app from the list. The workspace
    /// audit reports it instead.
    fn from_manifest(app_id: &str, value: Option<&str>) -> Self {
        match value {
            None | Some("on-load") => Self::OnLoad,
            Some("on-ready") => Self::OnReady,
            Some(other) => {
                log!(
                    "[Apps] {}: unrecognised manifest reveal {:?}, using on-load",
                    app_id,
                    other
                );
                Self::OnLoad
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub reveal: AppReveal,
    pub kind: AppKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_thread_id: Option<uuid::Uuid>,
    pub reusable: bool,
}

impl App {
    pub fn is_widget(&self) -> bool {
        self.kind == AppKind::Widget
    }
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
            let base = super::reset_index_to_head(&repo, &mut index)?;
            let repo_path = format!("data/apps/{}", app_path);
            super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
            index.write()?;
            super::commit_index(&repo, message, base)
        })
    }

    /// Stage multiple app files and commit in one operation.
    pub fn commit_batch(&self, app_paths: &[String], message: &str) -> Result<String, git2::Error> {
        super::retry_while_repo_contended(|| {
            let repo = self.repo.lock().unwrap();
            let mut index = repo.index()?;
            let base = super::reset_index_to_head(&repo, &mut index)?;
            for p in app_paths {
                let repo_path = format!("data/apps/{}", p);
                super::add_path_unless_ignored(&repo, &mut index, &repo_path)?;
            }
            index.write()?;
            super::commit_index(&repo, message, base)
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

        let origin_thread_id = manifest.origin_thread_id.as_deref().and_then(|raw| {
            let parsed = uuid::Uuid::parse_str(raw).ok();
            if parsed.is_none() {
                log!("[Apps] {}: origin_thread_id {:?} is not a uuid", id, raw);
            }
            parsed
        });
        Ok(App {
            reveal: AppReveal::from_manifest(&id, manifest.reveal.as_deref()),
            kind: AppKind::from_manifest(&id, manifest.kind.as_deref()),
            origin_thread_id,
            reusable: manifest.reusable,
            id,
            name: manifest.name,
            description: manifest.description,
            icon: manifest.icon,
        })
    }

    /// The apps list: every app folder that is not a widget. Every apps
    /// reader goes through here, so none of them shows a widget.
    pub fn list_apps(&self) -> Result<Vec<App>, std::io::Error> {
        Ok(self.apps_and_reusable_widgets()?.0)
    }

    /// The reusable widgets: the ones any thread may show.
    pub fn list_reusable_widgets(&self) -> Result<Vec<App>, std::io::Error> {
        Ok(self.apps_and_reusable_widgets()?.1)
    }

    /// The apps list and the reusable widgets from ONE scan, so a reader
    /// needing both cannot see a widget in both or in neither.
    pub fn apps_and_reusable_widgets(&self) -> Result<(Vec<App>, Vec<App>), std::io::Error> {
        let (widgets, apps): (Vec<App>, Vec<App>) =
            self.list_all()?.into_iter().partition(App::is_widget);
        let reusable: Vec<App> = widgets.into_iter().filter(|w| w.reusable).collect();
        Ok((apps, reusable))
    }

    /// The widgets these threads own: made in one of them and not reusable.
    /// These are what Delete (a thread) removes.
    pub fn widgets_owned_by(
        &self,
        thread_ids: &[uuid::Uuid],
    ) -> Result<Vec<String>, std::io::Error> {
        Ok(self
            .list_all()?
            .into_iter()
            .filter(|app| {
                app.is_widget()
                    && !app.reusable
                    && app
                        .origin_thread_id
                        .is_some_and(|t| thread_ids.contains(&t))
            })
            .map(|app| app.id)
            .collect())
    }

    /// Every app folder with a readable manifest, apps and widgets alike, by
    /// name and then id. `read_dir` promises no order, and the apps list rides
    /// the cached system prompt, so an unsorted scan could rewrite that cache.
    fn list_all(&self) -> Result<Vec<App>, std::io::Error> {
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

        apps.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
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

    /// The app's kind from its manifest, or `None` if it can't be read.
    /// Read before a delete, for `AppDeleted` (ADR 0404).
    pub fn app_kind(&self, app_id: &str) -> Option<AppKind> {
        self.get_app(app_id).ok().map(|a| a.kind)
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
        kind: NewAppKind,
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

        let (kind, origin_thread_id) = match kind {
            NewAppKind::App => (AppKind::App, None),
            NewAppKind::Widget { origin_thread_id } => {
                (AppKind::Widget, Some(origin_thread_id.to_string()))
            }
        };
        let manifest = AppManifest {
            name: name.to_string(),
            description: description.to_string(),
            icon: None,
            reveal: None,
            kind: kind.manifest_value(),
            origin_thread_id,
            reusable: false,
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
            &format!(
                "Create {}: {}",
                match kind {
                    AppKind::App => "app",
                    AppKind::Widget => "widget",
                },
                name
            ),
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
        // Read before the manifest goes: `AppDeleted` records it (ADR 0404).
        let kind = self.load_app(&app_dir).ok().map(|app| app.kind);

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
            let base = super::reset_index_to_head(&repo, &mut index)?;
            let _ = index.remove_dir(Path::new(&format!("data/apps/{}", app_id)), 0);
            index.write()?;
            let message = format!("Delete app: {}", app_id);
            super::commit_index_unless_unchanged(&repo, &message, base)
        })?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppDeleted {
                    app_id: app_id.to_string(),
                    kind,
                    actor,
                }),
                "[Apps] AppDeleted",
            )
            .await;
        Ok(commit)
    }

    /// Update an app's name and description in manifest.json (preserving
    /// every other field), commit, and announce it.
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
            ..existing
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

    /// Make a widget reusable, or stop reusing it. Only the flag changes: the
    /// origin thread stays recorded, so a widget that stops being reusable is
    /// owned by its origin thread again. Refuses anything that is not a widget.
    pub async fn set_widget_reusable(
        &self,
        event_bus: &EventBus,
        app_id: &str,
        reusable: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let app = self.get_app(app_id)?;
        if !app.is_widget() {
            return Err(format!("'{}' is an app, not a widget", app_id).into());
        }
        let manifest_path = self.apps_path.join(app_id).join("manifest.json");
        let existing: AppManifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
        if existing.reusable == reusable {
            return Err(format!(
                "Widget '{}' is already {}",
                app_id,
                if reusable { "reusable" } else { "not reusable" }
            )
            .into());
        }
        let manifest = AppManifest {
            reusable,
            ..existing
        };
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

        let commit = self.commit(
            &format!("{}/manifest.json", app_id),
            &format!(
                "{} widget: {}",
                if reusable { "Reuse" } else { "Stop reusing" },
                app.name
            ),
        )?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::AppUpdated {
                    app_id: app_id.to_string(),
                    name: Some(app.name),
                    actor,
                }),
                "[Apps] AppUpdated",
            )
            .await;
        Ok(commit)
    }

    /// Remove the widgets Delete (a thread) takes with it, in one commit, and
    /// announce each one. Returns the ids removed. Only widgets are accepted,
    /// so this can never remove an app. A widget made reusable since the caller
    /// listed it is kept. A folder that fails to go stops the run, but what
    /// already went is still committed and announced.
    pub async fn delete_widgets(
        &self,
        event_bus: &EventBus,
        app_ids: &[String],
        actor: Option<MessageOrigin>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let mut targets: Vec<&String> = Vec::new();
        for app_id in app_ids {
            let app = self.get_app(app_id)?;
            if !app.is_widget() {
                return Err(format!("'{}' is an app, not a widget", app_id).into());
            }
            if !app.reusable {
                targets.push(app_id);
            }
        }
        let mut removed: Vec<String> = Vec::new();
        let mut failure: Option<std::io::Error> = None;
        for app_id in targets {
            match std::fs::remove_dir_all(self.apps_path.join(app_id)) {
                Ok(()) => removed.push(app_id.clone()),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if !removed.is_empty() {
            // Same shape as `delete_app`: the removal stays outside the retry, and
            // the commit tolerates a winner that already committed it.
            let commit = super::retry_while_repo_contended(|| {
                let repo = self.repo.lock().unwrap();
                let mut index = repo.index()?;
                let base = super::reset_index_to_head(&repo, &mut index)?;
                for app_id in &removed {
                    let _ = index.remove_dir(Path::new(&format!("data/apps/{}", app_id)), 0);
                }
                index.write()?;
                let message = format!("Delete widgets with their thread: {}", removed.join(", "));
                super::commit_index_unless_unchanged(&repo, &message, base)
            })?;
            log!("[Apps] Removed widgets {:?} in {}", removed, commit);
            for app_id in &removed {
                event_bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::AppDeleted {
                            app_id: app_id.clone(),
                            kind: Some(AppKind::Widget),
                            actor: actor.clone(),
                        }),
                        "[Apps] AppDeleted",
                    )
                    .await;
            }
        }
        match failure {
            Some(e) => Err(format!(
                "removed {} of {} widgets, then failed: {}",
                removed.len(),
                app_ids.len(),
                e
            )
            .into()),
            None => Ok(removed),
        }
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

    /// Where an app's `manifest.json` lives. `app_id` must already be valid.
    pub fn manifest_path(&self, app_id: &str) -> PathBuf {
        self.apps_path.join(app_id).join("manifest.json")
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
            let base = super::reset_index_to_head(&repo, &mut index)?;
            let repo_path = format!("data/apps/{}", app_path);
            let _ = index.remove_path(Path::new(&repo_path));
            index.write()?;
            super::commit_index_unless_unchanged(&repo, message, base)
        })?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
            reveal: Some("on-ready".to_string()),
            kind: Some("widget".to_string()),
            origin_thread_id: Some(uuid::Uuid::nil().to_string()),
            reusable: true,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: AppManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, manifest.name);
        assert_eq!(deserialized.description, manifest.description);
        assert_eq!(deserialized.icon, manifest.icon);
        assert_eq!(deserialized.reveal, manifest.reveal);
        assert_eq!(deserialized.kind, manifest.kind);
        assert_eq!(deserialized.origin_thread_id, manifest.origin_thread_id);
        assert!(deserialized.reusable);
    }

    fn write_manifest(ws: &Path, app_id: &str, json: &str) {
        let dir = ws.join("data/apps").join(app_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), json).unwrap();
    }

    /// The apps list rides the cached system prompt, so its order must not
    /// depend on the order folders were created in.
    #[test]
    fn apps_list_by_name_whatever_order_their_folders_were_made_in() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        for (id, name) in [("zeta", "Zeta"), ("alpha", "Alpha"), ("mid", "Mid")] {
            write_manifest(tmp.path(), id, &format!(r#"{{"name": "{name}"}}"#));
        }
        let names: Vec<String> = manager
            .list_apps()
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["Alpha", "Mid", "Zeta"]);
    }

    #[test]
    fn reveal_reads_from_the_manifest_and_defaults_to_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        write_manifest(tmp.path(), "plain", r#"{"name": "Plain"}"#);
        write_manifest(
            tmp.path(),
            "waits",
            r#"{"name": "Waits", "reveal": "on-ready"}"#,
        );
        write_manifest(
            tmp.path(),
            "explicit",
            r#"{"name": "Explicit", "reveal": "on-load"}"#,
        );

        assert_eq!(manager.get_app("plain").unwrap().reveal, AppReveal::OnLoad);
        assert_eq!(manager.get_app("waits").unwrap().reveal, AppReveal::OnReady);
        assert_eq!(
            manager.get_app("explicit").unwrap().reveal,
            AppReveal::OnLoad
        );
    }

    /// A typo in `reveal` must not make the manifest invalid, which would drop
    /// the app from the list. It opens as on-load, and the audit reports it.
    #[test]
    fn an_unrecognised_reveal_still_lists_the_app_as_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        write_manifest(
            tmp.path(),
            "typo",
            r#"{"name": "Typo", "reveal": "onready"}"#,
        );

        let apps = manager.list_apps().unwrap();
        let typo = apps.iter().find(|a| a.id == "typo").expect("still listed");
        assert_eq!(typo.reveal, AppReveal::OnLoad);
    }

    #[test]
    fn the_api_shape_carries_reveal_in_kebab_case() {
        let app = App {
            id: "waits".to_string(),
            name: "Waits".to_string(),
            description: String::new(),
            icon: None,
            reveal: AppReveal::OnReady,
            kind: AppKind::App,
            origin_thread_id: None,
            reusable: false,
        };
        let json = serde_json::to_value(&app).unwrap();
        assert_eq!(json["reveal"], "on-ready");
    }

    /// The rewrite rebuilds the manifest from its known fields, so a field it
    /// forgot would be dropped by a rename.
    #[tokio::test]
    async fn a_metadata_update_keeps_the_reveal_opt_in() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        manager
            .create_app(&bus, "waits", "Waits", "", "<h1>hi", NewAppKind::App)
            .await
            .unwrap();
        write_manifest(
            tmp.path(),
            "waits",
            r#"{"name": "Waits", "reveal": "on-ready"}"#,
        );

        manager
            .update_app_metadata(&bus, "waits", "Waits Renamed", "New", None)
            .await
            .unwrap();

        let app = manager.get_app("waits").unwrap();
        assert_eq!(app.name, "Waits Renamed");
        assert_eq!(app.reveal, AppReveal::OnReady);
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
            .create_app(
                &bus,
                "../escaped",
                "Evil",
                "",
                "<html></html>",
                NewAppKind::App,
            )
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
            .create_app(
                &bus,
                "habit-tracker",
                "Habit Tracker",
                "Habits",
                "<h1>v1",
                NewAppKind::App,
            )
            .await
            .expect("first create");

        // The app grows a second page the way a real one does.
        let app_dir = tmp.path().join("data/apps/habit-tracker");
        let sibling = app_dir.join("stats.html");
        std::fs::write(&sibling, "<h1>stats").unwrap();

        let err = manager
            .create_app(
                &bus,
                "habit-tracker",
                "Habit Tracker",
                "Habits",
                "<h1>v2",
                NewAppKind::App,
            )
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
            .create_app(
                &bus,
                "half-built",
                "Half Built",
                "",
                "<h1>done",
                NewAppKind::App,
            )
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
            .create_app(
                &bus,
                "habit-tracker",
                "Habit Tracker",
                "Habits",
                "<h1>v1",
                NewAppKind::App,
            )
            .await
            .expect_err("writing index.html over a directory must fail");
        assert!(
            !manager.app_exists("habit-tracker"),
            "a failed create must not mark the app as existing"
        );

        std::fs::remove_dir(app_dir.join("index.html")).unwrap();
        manager
            .create_app(
                &bus,
                "habit-tracker",
                "Habit Tracker",
                "Habits",
                "<h1>v1",
                NewAppKind::App,
            )
            .await
            .expect("the retry must finish the app");
        assert!(manager.app_exists("habit-tracker"));
    }

    // --- widgets: the app kind (ADR 0402) ---------------------------------

    fn thread(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    /// The fixture every reader test shares: one app and one widget.
    pub(crate) async fn app_and_widget(manager: &AppManager, bus: &EventBus) -> uuid::Uuid {
        let origin = thread(1);
        manager
            .create_app(
                bus,
                "habit-tracker",
                "Habit Tracker",
                "Habits",
                "<h1>app",
                NewAppKind::App,
            )
            .await
            .unwrap();
        manager
            .create_app(
                bus,
                "fare-grid",
                "Fare grid",
                "Fares by date",
                "<h1>widget",
                NewAppKind::Widget {
                    origin_thread_id: origin,
                },
            )
            .await
            .unwrap();
        origin
    }

    #[tokio::test]
    async fn the_apps_list_never_holds_a_widget() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        let origin = app_and_widget(&manager, &bus).await;

        let ids: Vec<String> = manager
            .list_apps()
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, vec!["habit-tracker".to_string()]);

        let widget = manager
            .get_app("fare-grid")
            .expect("a widget is still an app by id");
        assert_eq!(widget.kind, AppKind::Widget);
        assert_eq!(widget.origin_thread_id, Some(origin));
        assert!(!widget.reusable);
    }

    /// A file-tool delete reads this before the manifest goes (ADR 0404).
    #[tokio::test]
    async fn app_kind_reads_the_manifest_and_is_none_once_gone() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        app_and_widget(&manager, &bus).await;
        assert_eq!(manager.app_kind("fare-grid"), Some(AppKind::Widget));
        assert_eq!(manager.app_kind("habit-tracker"), Some(AppKind::App));
        assert_eq!(manager.app_kind("never-made"), None);
    }

    #[test]
    fn an_unrecognised_kind_lists_the_folder_as_an_app() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        write_manifest(tmp.path(), "typo", r#"{"name": "Typo", "kind": "widgte"}"#);
        let apps = manager.list_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].kind, AppKind::App);
    }

    #[test]
    fn a_widget_manifest_with_a_bad_origin_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        write_manifest(
            tmp.path(),
            "odd",
            r#"{"name": "Odd", "kind": "widget", "origin_thread_id": "nope"}"#,
        );
        let app = manager.get_app("odd").unwrap();
        assert!(app.is_widget());
        assert_eq!(app.origin_thread_id, None);
    }

    #[tokio::test]
    async fn reusable_widgets_are_listed_and_reversible() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        let origin = app_and_widget(&manager, &bus).await;
        assert!(manager.list_reusable_widgets().unwrap().is_empty());
        assert_eq!(
            manager.widgets_owned_by(&[origin]).unwrap(),
            vec!["fare-grid"]
        );

        manager
            .set_widget_reusable(&bus, "fare-grid", true, None)
            .await
            .unwrap();
        let reusable = manager.list_reusable_widgets().unwrap();
        assert_eq!(reusable.len(), 1);
        assert_eq!(
            reusable[0].origin_thread_id,
            Some(origin),
            "origin stays recorded"
        );
        assert!(
            manager.widgets_owned_by(&[origin]).unwrap().is_empty(),
            "a reusable widget belongs to no thread"
        );
        assert!(manager
            .list_apps()
            .unwrap()
            .iter()
            .all(|a| a.id != "fare-grid"));

        manager
            .set_widget_reusable(&bus, "fare-grid", false, None)
            .await
            .unwrap();
        assert_eq!(
            manager.widgets_owned_by(&[origin]).unwrap(),
            vec!["fare-grid"]
        );
    }

    #[tokio::test]
    async fn only_a_widget_can_be_made_reusable() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        app_and_widget(&manager, &bus).await;
        let err = manager
            .set_widget_reusable(&bus, "habit-tracker", true, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a widget"), "got: {err}");
    }

    #[tokio::test]
    async fn a_metadata_update_keeps_the_widget_fields() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        let origin = app_and_widget(&manager, &bus).await;
        manager
            .update_app_metadata(&bus, "fare-grid", "Fares", "Renamed", None)
            .await
            .unwrap();
        let app = manager.get_app("fare-grid").unwrap();
        assert_eq!(app.name, "Fares");
        assert!(app.is_widget());
        assert_eq!(app.origin_thread_id, Some(origin));
    }

    #[tokio::test]
    async fn deleting_owned_widgets_refuses_an_app_and_removes_nothing() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        app_and_widget(&manager, &bus).await;
        let ids = vec!["fare-grid".to_string(), "habit-tracker".to_string()];
        assert!(manager.delete_widgets(&bus, &ids, None).await.is_err());
        assert!(
            manager.app_exists("fare-grid"),
            "nothing goes when one id is an app"
        );
        assert!(manager.app_exists("habit-tracker"));

        let removed = manager
            .delete_widgets(&bus, &["fare-grid".to_string()], None)
            .await
            .unwrap();
        assert_eq!(removed, vec!["fare-grid".to_string()]);
        assert!(!manager.app_exists("fare-grid"));
        assert!(manager.app_exists("habit-tracker"));
        assert!(manager
            .delete_widgets(&bus, &[], None)
            .await
            .unwrap()
            .is_empty());

        // A widget made reusable after the caller listed it is kept.
        manager
            .create_app(
                &bus,
                "shared",
                "Shared",
                "",
                "<h1>w",
                NewAppKind::Widget {
                    origin_thread_id: thread(1),
                },
            )
            .await
            .unwrap();
        manager
            .set_widget_reusable(&bus, "shared", true, None)
            .await
            .unwrap();
        let removed = manager
            .delete_widgets(&bus, &["shared".to_string()], None)
            .await
            .unwrap();
        assert!(removed.is_empty());
        assert!(manager.app_exists("shared"));
    }
}
