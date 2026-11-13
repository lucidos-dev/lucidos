//! Agent-created drafts (ADR 0665). An agent hands the owner a ready message
//! as a new composing thread with a seeded draft. Every surface calls
//! [`AgentDraftService::create`]: `lucidos threads create-draft` and
//! `POST /api/v1/threads/drafts`.
//!
//! Creating a draft emits no navigation and no message, writes no queue row and
//! starts no session. The owner finds it in the drafts list and sends it, or
//! not. The draft keeps the exact shape the saved-draft send accepts (ADR 0651).

use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

use crate::core::draft_submission::{SavedDraftScope, SavedDraftSelection, SavedDraftSnapshot};
use crate::core::preference_catalog as prefs;
use crate::core::store::{
    ComposeReport, ComposeReportRow, DraftSummary, EventStore, COMPOSE_REPORT_COLS,
    MAX_COMPOSE_IMAGES, MAX_COMPOSE_SELECTION_BYTES, MAX_COMPOSE_TEXT_BYTES,
};
use crate::core::ModelDefaults;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AgentDraft, EventMeta, MessageOrigin, ThreadEvent};
use crate::llm::model_registry::ProviderKind;
use crate::runtime::CodingAgent;

/// The longest retry key a caller may send.
const MAX_RETRY_KEY_CHARS: usize = 200;

/// What a caller asks for. Unknown fields fail closed. The engine maps the
/// picks onto the composer's selection, so no caller writes that shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDraftRequest {
    pub text: String,
    /// Workspace paths under `data/`, such as `artifacts/chart.png`, in order.
    #[serde(default)]
    pub images: Vec<String>,
    /// `lucidos` or `claude-code`.
    pub mode: String,
    /// A registered external repository, for a coding-agent draft.
    #[serde(default)]
    pub repo_id: Option<String>,
    /// An app folder under `data/apps/`, for a coding-agent draft.
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub coding_agent: Option<CodingAgent>,
    #[serde(default)]
    pub model: Option<String>,
    /// A chat draft's provider. A coding-agent draft takes none.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Names this draft for the calling thread. Sending the same key again
    /// returns the first draft as it now stands.
    pub retry_key: String,
}

/// The draft as the drafts read reports it, and whether this call created it.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedDraft {
    pub created: bool,
    /// The key that names this draft, for a retry.
    pub retry_key: String,
    #[serde(flatten)]
    pub draft: DraftSummary,
}

/// A request that passed every check, ready to write.
struct ValidatedDraft {
    text: String,
    images: Vec<Vec<u8>>,
    image_hashes: Vec<String>,
    mode: &'static str,
    selection: serde_json::Value,
}

pub struct AgentDraftService<'a> {
    pub bus: &'a EventBus,
    pub store: &'a EventStore,
    pub pool: &'a sqlx::PgPool,
    pub workspace: &'a Path,
    pub workspace_name: &'a str,
    pub defaults: ModelDefaults<'a>,
    pub configured: &'a [ProviderKind],
}

/// The draft's thread id: the same caller and key always name the same thread.
pub fn draft_thread_id(caller_thread_id: Uuid, retry_key: &str) -> Uuid {
    Uuid::new_v5(&caller_thread_id, retry_key.as_bytes())
}

impl AgentDraftService<'_> {
    /// Validate everything, then write blobs and one `ThreadStarted`. A
    /// refusal writes nothing. A retry returns the existing draft untouched.
    /// `actor` must come from the caller's verified origin token.
    pub async fn create(
        &self,
        request: CreateDraftRequest,
        caller_thread_id: Uuid,
        actor: MessageOrigin,
    ) -> Result<CreatedDraft, DraftRefusal> {
        let retry_key = request.retry_key.trim().to_string();
        if retry_key.is_empty() || retry_key.chars().count() > MAX_RETRY_KEY_CHARS {
            return Err(DraftRefusal::invalid("draft_retry_key_invalid"));
        }
        let thread_id = draft_thread_id(caller_thread_id, &retry_key);
        if let Some(existing) = self.existing(thread_id, &retry_key).await? {
            return Ok(existing);
        }
        let draft = self.validate(request).await?;
        let workspace = self.workspace.to_path_buf();
        let images = draft.images;
        tokio::task::spawn_blocking(move || {
            images
                .iter()
                .try_for_each(|bytes| crate::core::blobs::write_blob(&workspace, bytes).map(drop))
        })
        .await
        .map_err(DraftRefusal::internal)?
        .map_err(DraftRefusal::internal)?;
        let event = BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadStarted {
                recovered_compose: None,
                agent_draft: Some(AgentDraft {
                    text: draft.text,
                    image_hashes: draft.image_hashes,
                    selection: Some(draft.selection),
                    drafted_by_thread_id: caller_thread_id,
                }),
                mode: draft.mode.to_string(),
                actor: Some(actor.clone()),
            },
            meta: EventMeta::with_actor(Some(actor)),
        };
        let created = self
            .bus
            .emit(event)
            .await
            .map_err(DraftRefusal::internal)?
            .is_some();
        if !created {
            // A racing retry created it first. Report that one.
            return self
                .existing(thread_id, &retry_key)
                .await?
                .ok_or_else(|| DraftRefusal::internal("the draft's thread vanished"));
        }
        let report: ComposeReportRow = sqlx::query_as(&format!(
            "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id = $1"
        ))
        .bind(thread_id)
        .fetch_one(self.pool)
        .await
        .map_err(DraftRefusal::internal)?;
        self.bus
            .emit(ComposeReport::from(report).broadcast(thread_id, None))
            .await
            .map_err(DraftRefusal::internal)?;
        Ok(CreatedDraft {
            created: true,
            retry_key,
            draft: self.read(thread_id).await?,
        })
    }

    /// A retry's answer: the draft as it stands now, never rewritten.
    async fn existing(
        &self,
        thread_id: Uuid,
        retry_key: &str,
    ) -> Result<Option<CreatedDraft>, DraftRefusal> {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(self.pool)
                .await
                .map_err(DraftRefusal::internal)?;
        match state.as_deref() {
            None => Ok(None),
            Some("discarded") => Err(DraftRefusal::invalid("draft_discarded")),
            Some(_) => match self
                .store
                .get_draft(self.workspace_name, thread_id)
                .await
                .map_err(DraftRefusal::internal)?
            {
                Some(draft) => Ok(Some(CreatedDraft {
                    created: false,
                    retry_key: retry_key.to_string(),
                    draft,
                })),
                // The owner sent or cleared it. The key is spent either way.
                None => Err(DraftRefusal::invalid("draft_already_used")),
            },
        }
    }

    async fn read(&self, thread_id: Uuid) -> Result<DraftSummary, DraftRefusal> {
        self.store
            .get_draft(self.workspace_name, thread_id)
            .await
            .map_err(DraftRefusal::internal)?
            .ok_or_else(|| DraftRefusal::internal("the created draft is not readable"))
    }

    async fn validate(&self, request: CreateDraftRequest) -> Result<ValidatedDraft, DraftRefusal> {
        if request.text.len() > MAX_COMPOSE_TEXT_BYTES {
            return Err(DraftRefusal::invalid("draft_text_too_long"));
        }
        if request.images.len() > MAX_COMPOSE_IMAGES {
            return Err(DraftRefusal::invalid("draft_too_many_images"));
        }
        let mode = match request.mode.as_str() {
            "lucidos" => "lucidos",
            "claude-code" | "claude_code" => "claude_code",
            _ => return Err(DraftRefusal::invalid("draft_mode_unsupported")),
        };
        let selection = self.selection(&request, mode).await?;
        let selection_json = serde_json::to_value(&selection).map_err(DraftRefusal::internal)?;
        let selection_json = without_nulls(selection_json);
        if selection_json.to_string().len() > MAX_COMPOSE_SELECTION_BYTES {
            return Err(DraftRefusal::invalid("draft_selection_too_large"));
        }
        let workspace = self.workspace.to_path_buf();
        let paths = request.images.clone();
        let images = tokio::task::spawn_blocking(move || read_images(&workspace, &paths))
            .await
            .map_err(DraftRefusal::internal)??;
        let image_hashes: Vec<String> = images
            .iter()
            .map(|bytes| crate::core::blobs::compute_hash(bytes))
            .collect();
        // The send reads the row through this parser, so the draft must pass it.
        let snapshot = SavedDraftSnapshot::try_from((
            0,
            0,
            request.text.clone(),
            serde_json::json!(image_hashes),
            Some(mode.to_string()),
            Some(selection_json.clone()),
        ))
        .map_err(|e| DraftRefusal::invalid(e.to_string()))?;
        if mode == "claude_code" {
            let backend = selection
                .coding_agent
                .ok_or_else(|| DraftRefusal::internal("the coding agent was not pinned"))?;
            snapshot
                .resolve_coding_selection(None, backend)
                .map_err(|e| DraftRefusal::invalid(e.to_string()))?;
        } else {
            snapshot
                .resolve_chat_selection(
                    self.pool,
                    ModelDefaults {
                        registry: self.defaults.registry,
                        fallback_model: self.defaults.fallback_model,
                    },
                    None,
                    |kind| self.configured.contains(&kind),
                )
                .await
                .map_err(|e| DraftRefusal::invalid(e.to_string()))?;
        }
        Ok(ValidatedDraft {
            text: request.text,
            images,
            image_hashes,
            mode,
            selection: selection_json,
        })
    }

    /// The composer's selection for these picks. A coding-agent draft pins
    /// its backend now, as the composer does for a new draft.
    async fn selection(
        &self,
        request: &CreateDraftRequest,
        mode: &str,
    ) -> Result<SavedDraftSelection, DraftRefusal> {
        if mode == "lucidos" {
            if request.repo_id.is_some() || request.app_id.is_some() {
                return Err(DraftRefusal::invalid("draft_destination_needs_coding_mode"));
            }
            if request.coding_agent.is_some() {
                return Err(DraftRefusal::invalid(
                    "draft_coding_agent_needs_coding_mode",
                ));
            }
            return Ok(SavedDraftSelection {
                model: request.model.clone(),
                provider: request.provider.clone(),
                reasoning_effort: request.effort.clone(),
                ..SavedDraftSelection::default()
            });
        }
        if request.provider.is_some() {
            return Err(DraftRefusal::invalid("draft_provider_needs_chat_mode"));
        }
        let scope = match (&request.repo_id, &request.app_id) {
            (Some(_), Some(_)) => return Err(DraftRefusal::invalid("draft_destination_ambiguous")),
            (Some(repo_id), None) => Some(self.external_scope(repo_id).await?),
            (None, Some(app_id)) => Some(self.app_scope(app_id)?),
            (None, None) => None,
        };
        let coding_agent = match request.coding_agent {
            Some(backend) => backend,
            None => {
                let stored = prefs::CODING_AGENT_DEFAULT.read(self.pool).await;
                CodingAgent::ALL
                    .into_iter()
                    .find(|backend| backend.as_str() == stored)
                    .ok_or_else(|| DraftRefusal::invalid("draft_coding_backend_unsupported"))?
            }
        };
        Ok(SavedDraftSelection {
            scope,
            coding_agent: Some(coding_agent),
            cc_model: request.model.clone(),
            cc_reasoning_effort: request.effort.clone(),
            ..SavedDraftSelection::default()
        })
    }

    async fn external_scope(&self, repo_id: &str) -> Result<SavedDraftScope, DraftRefusal> {
        let id =
            Uuid::parse_str(repo_id).map_err(|_| DraftRefusal::invalid("draft_repo_invalid"))?;
        let repos = crate::core::repositories::RepositoryStore::list(self.pool)
            .await
            .map_err(DraftRefusal::internal)?;
        let repo = repos
            .iter()
            .find(|repo| repo.id == id)
            .ok_or_else(|| DraftRefusal::invalid("draft_repo_missing"))?;
        // The source checkout is the default destination, never an external one.
        if repo
            .name
            .eq_ignore_ascii_case(super::LucidosEngine::DEFAULT_REPO_NAME)
        {
            return Err(DraftRefusal::invalid("draft_repo_is_lucidos_source"));
        }
        Ok(SavedDraftScope::External {
            repo_id: id.to_string(),
        })
    }

    fn app_scope(&self, app_id: &str) -> Result<SavedDraftScope, DraftRefusal> {
        if !super::agent_session::coding_agent_kind::is_valid_app_id(app_id) {
            return Err(DraftRefusal::invalid("draft_app_invalid"));
        }
        let folder = self
            .workspace
            .join(crate::core::DATA_DIR)
            .join("apps")
            .join(app_id);
        if !folder.is_dir() {
            return Err(DraftRefusal::invalid("draft_app_missing"));
        }
        Ok(SavedDraftScope::App {
            app_id: app_id.to_string(),
        })
    }
}

/// Read every image before any is written, so a bad one writes nothing.
fn read_images(workspace: &Path, paths: &[String]) -> Result<Vec<Vec<u8>>, DraftRefusal> {
    let files = crate::core::email::EmailAttachment::read_from_workspace(workspace, paths)
        .map_err(DraftRefusal::invalid)?;
    files
        .into_iter()
        .zip(paths)
        .map(|(file, path)| {
            if crate::core::blobs::sniff_image_mime(&file.data).is_none() {
                return Err(DraftRefusal::invalid(format!(
                    "draft_image_unsupported: '{path}' is not an image the composer takes"
                )));
            }
            Ok(file.data)
        })
        .collect()
}

/// The selection the composer stores names only the picks it holds.
fn without_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            serde_json::Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).collect())
        }
        other => other,
    }
}

/// Why a draft was not created. The route answers `Invalid` with 400 and its
/// slug, and `Internal` with 500.
#[derive(Debug)]
pub enum DraftRefusal {
    Invalid(String),
    Internal(String),
}

impl DraftRefusal {
    fn invalid(reason: impl Into<String>) -> Self {
        Self::Invalid(reason.into())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

impl std::fmt::Display for DraftRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) | Self::Internal(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for DraftRefusal {}

impl super::LucidosEngine {
    /// The one entry point every draft surface calls.
    pub async fn create_agent_draft(
        &self,
        request: CreateDraftRequest,
        caller_thread_id: Uuid,
        actor: MessageOrigin,
    ) -> Result<CreatedDraft, DraftRefusal> {
        let provider = self.current_provider();
        let configured = self.configured_provider_kinds();
        let workspace_name = self.workspace_name();
        AgentDraftService {
            bus: &self.event_bus,
            store: self.event_store(),
            pool: self.pool(),
            workspace: self.workspace_path(),
            workspace_name: &workspace_name,
            defaults: ModelDefaults {
                registry: self.model_registry(),
                fallback_model: provider.default_model(),
            },
            configured: &configured,
        }
        .create(request, caller_thread_id, actor)
        .await
    }
}

#[cfg(test)]
#[path = "agent_drafts_tests.rs"]
mod tests;
