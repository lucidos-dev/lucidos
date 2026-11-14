//! Saved-owner-content submission. Transports supply authenticated provenance,
//! never a send payload. Every surface must use this service (ADR 0651).
use crate::core::draft_submission::{
    DraftSubmissionAuthority, DraftSubmissionReceipt, SavedDraftRow, SavedDraftScope,
    SavedDraftSnapshot, SubmitDraftRequest, SAVED_DRAFT_COLS,
};
use crate::core::preference_catalog as prefs;
use crate::core::{ModelDefaults, PreferenceStore};
use crate::engine::event_bus::{EventBus, PreparedDraftSubmission};
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::thread_queue::ThreadQueueRequest;
use crate::llm::model_registry::ProviderKind;
use crate::runtime::CodingAgent;
use std::path::Path;
use uuid::Uuid;

pub struct DraftSubmissionService<'a> {
    pub bus: &'a EventBus,
    pub pool: &'a sqlx::PgPool,
    pub workspace: &'a Path,
    pub defaults: ModelDefaults<'a>,
    pub configured: &'a [ProviderKind],
}

impl DraftSubmissionService<'_> {
    /// Submit for the agent in thread `caller`. The engine finds the owner's
    /// authority itself: a standing instruction, else a live permission to
    /// send to this target. A retry of an accepted request returns its receipt.
    /// A stale draft refuses before authority, so a sent draft never asks the
    /// owner for a second card.
    pub async fn submit_as_agent(
        &self,
        caller: Uuid,
        request: SubmitDraftRequest,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        request.validate()?;
        let actor = crate::engine::chat::agent_archive::agent_thread_actor(caller);
        if let Some(receipt) = self.bus.saved_draft_receipt(&request, &actor).await? {
            return Ok(receipt);
        }
        let raw = self.read_current(&request).await?;
        let mut conn = self.pool.acquire().await?;
        let authority = if let Some(instruction_event_id) =
            crate::api::standing_instruction::standing_instruction_event(&mut conn, caller).await?
        {
            DraftSubmissionAuthority::StandingInstruction {
                instruction_event_id,
            }
        } else {
            let permission = crate::engine::permission::live_permission(
                &mut *conn,
                &crate::api::permission_requests::thread_user(caller),
                crate::engine::permission::Action::SubmitDraft,
                Some(&request.thread_id.to_string()),
            )
            .await?
            .ok_or("draft_owner_authority_missing")?;
            DraftSubmissionAuthority::Permission {
                permission_id: permission.permission_id,
            }
        };
        drop(conn);
        self.accept_current(request, actor, authority, raw).await
    }

    /// Tests supply the actor and authority directly. Authority is rechecked
    /// and, when single-use, spent at atomic acceptance.
    #[cfg(test)]
    pub async fn submit(
        &self,
        request: SubmitDraftRequest,
        actor: MessageOrigin,
        authority: DraftSubmissionAuthority,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(receipt) = self.bus.saved_draft_receipt(&request, &actor).await? {
            return Ok(receipt);
        }
        let raw = self.read_current(&request).await?;
        self.accept_current(request, actor, authority, raw).await
    }

    /// The target's saved draft. It refuses as stale unless it is still the
    /// revision and epoch the request names. It refuses as missing when the
    /// drafts read would list nothing for it.
    async fn read_current(
        &self,
        request: &SubmitDraftRequest,
    ) -> Result<SavedDraftRow, Box<dyn std::error::Error + Send + Sync>> {
        let raw: SavedDraftRow = sqlx::query_as(&format!(
            "SELECT {SAVED_DRAFT_COLS} FROM thread_summaries WHERE thread_id=$1 AND state IN ('composing','active')"
        )).bind(request.thread_id).fetch_optional(self.pool).await?.ok_or("draft_missing")?;
        if !request.names_current(&raw) {
            return Err("draft_snapshot_stale".into());
        }
        if !crate::core::store::has_draft(&raw.report.2, &raw.report.3) {
            return Err("draft_missing".into());
        }
        Ok(raw)
    }

    /// Resolve the send for a current draft and accept it.
    async fn accept_current(
        &self,
        request: SubmitDraftRequest,
        actor: MessageOrigin,
        authority: DraftSubmissionAuthority,
        raw: SavedDraftRow,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        let snapshot = SavedDraftSnapshot::try_from(raw)?;
        let binding: Binding = sqlx::query_as(
            "SELECT state,source,coding_agent,cc_repo_id,coding_agent_kind,coding_agent_folder \
             FROM thread_summaries WHERE thread_id=$1",
        )
        .bind(request.thread_id)
        .fetch_one(self.pool)
        .await?;
        let active = binding.state == "active";
        let coding = if active {
            match binding.source.as_str() {
                "claude_code" => true,
                "chat" => false,
                _ => return Err("draft_channel_unsupported".into()),
            }
        } else {
            // A NULL composing mode depends on device-local inputMode. No
            // authenticated agent can infer that device's selection.
            match snapshot.report().mode.as_deref() {
                Some("claude_code") => true,
                Some("lucidos") => false,
                _ => return Err("draft_channel_unspecified".into()),
            }
        };
        let (repo_id, app_id) = if coding {
            self.resolve_destination(&snapshot, &binding, active)
                .await?
        } else {
            (None, None)
        };
        snapshot.resolve_images(self.workspace)?;
        let (model, provider, effort, cc_model, backend) = if coding {
            let bound = if active {
                Some(strict_backend(
                    binding.coding_agent.as_deref().unwrap_or("claude-code"),
                )?)
            } else {
                None
            };
            let default = match bound.or(snapshot.selection().coding_agent) {
                Some(backend) => backend,
                None => {
                    let stored = PreferenceStore::get_all(self.pool)
                        .await?
                        .remove(prefs::CODING_AGENT_DEFAULT.key())
                        .unwrap_or_else(|| prefs::CODING_AGENT_DEFAULT.resolve(None));
                    strict_backend(&stored)?
                }
            };
            let selected = snapshot.resolve_coding_selection(bound, default)?;
            (
                None,
                None,
                selected.effort,
                selected.model,
                Some(selected.backend),
            )
        } else {
            let selected = snapshot
                .resolve_chat_selection(
                    self.pool,
                    ModelDefaults {
                        registry: self.defaults.registry,
                        fallback_model: self.defaults.fallback_model,
                    },
                    active.then_some(request.thread_id),
                    |kind| self.configured.contains(&kind),
                )
                .await?;
            (
                selected.model,
                selected.provider,
                selected.reasoning_effort,
                None,
                None,
            )
        };
        let report = snapshot.report();
        let queue_request = ThreadQueueRequest::AgentChat {
            user_addressing: None,
            message: report.text.clone(),
            thread_id: request.thread_id,
            depth: 0,
            event_id: Some(Uuid::new_v4().to_string()),
            image_hashes: report.image_hashes.clone(),
            device_id: None,
            app_context: None,
            file_context: None,
            url_context: None,
            conflict_change_id: None,
            model,
            reasoning_effort: effort,
            provider,
            use_coding_agent: Some(coding),
            repo_id,
            cc_model,
            coding_agent: backend,
            title: None,
            mode: ActorMode::Human,
            origin: None,
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
            app_id,
            integrated: false,
            spawning_trigger_id: None,
        };
        self.bus
            .accept_saved_draft(PreparedDraftSubmission {
                request,
                snapshot,
                queue_request,
                actor,
                authority,
            })
            .await
    }

    async fn resolve_destination(
        &self,
        snapshot: &SavedDraftSnapshot,
        binding: &Binding,
        active: bool,
    ) -> Result<(Option<String>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
        use super::agent_session::coding_agent_kind::{
            classify_resolved_folder, FolderClassification,
        };
        use crate::core::repositories::RepositoryStore;
        let workspace = std::fs::canonicalize(self.workspace)?;
        let repos = RepositoryStore::list(self.pool).await?;
        let source = repos.iter().find(|repo| {
            repo.name
                .eq_ignore_ascii_case(super::LucidosEngine::DEFAULT_REPO_NAME)
        });
        // A stale source registration must not disable unrelated app or
        // external targets. Only absence is optional; other filesystem
        // failures remain explicit refusals. A source send still requires it.
        let source_path = match source.map(|repo| std::fs::canonicalize(&repo.path)) {
            Some(Ok(path)) => Some(path),
            Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Some(Err(error)) => return Err(error.into()),
            None => None,
        };
        let bound_to_source = source.is_some_and(|repo| {
            binding
                .cc_repo_id
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok())
                == Some(repo.id)
        });
        let scope = if active {
            match binding.coding_agent_kind.as_deref() {
                Some("app") => {
                    let folder = binding
                        .coding_agent_folder
                        .as_deref()
                        .ok_or("draft_app_binding_invalid")?;
                    let path = std::fs::canonicalize(folder)?;
                    match classify_resolved_folder(
                        &path,
                        &workspace,
                        source_path.as_deref(),
                        |_| None,
                    )? {
                        FolderClassification::App { app_id, .. } => SavedDraftScope::App { app_id },
                        _ => return Err("draft_app_binding_invalid".into()),
                    }
                }
                Some("external") => SavedDraftScope::External {
                    repo_id: binding
                        .cc_repo_id
                        .clone()
                        .ok_or("draft_repo_binding_invalid")?,
                },
                Some("lucidos") => {
                    if binding.cc_repo_id.is_some() && !bound_to_source {
                        return Err("draft_repo_binding_invalid".into());
                    }
                    SavedDraftScope::Lucidos {}
                }
                None if binding.cc_repo_id.is_none() || bound_to_source => {
                    SavedDraftScope::Lucidos {}
                }
                None => SavedDraftScope::External {
                    repo_id: binding
                        .cc_repo_id
                        .clone()
                        .ok_or("draft_repo_binding_invalid")?,
                },
                _ => return Err("draft_destination_unsupported".into()),
            }
        } else {
            snapshot
                .selection()
                .scope
                .clone()
                .unwrap_or(SavedDraftScope::Lucidos {})
        };
        if active
            && snapshot
                .selection()
                .scope
                .as_ref()
                .is_some_and(|saved| saved != &scope)
        {
            return Err("draft_destination_locked".into());
        }
        let (path, expected_repo, expected_app) = match scope {
            SavedDraftScope::Lucidos {} => (
                source_path.clone().ok_or("draft_lucidos_source_missing")?,
                None,
                None,
            ),
            SavedDraftScope::External { repo_id } => {
                let id = Uuid::parse_str(&repo_id).map_err(|_| "draft_repo_invalid")?;
                let repo = repos
                    .iter()
                    .find(|repo| repo.id == id)
                    .ok_or("draft_repo_missing")?;
                (
                    std::fs::canonicalize(&repo.path)?,
                    Some(id.to_string()),
                    None,
                )
            }
            SavedDraftScope::App { app_id } => {
                if !super::agent_session::coding_agent_kind::is_valid_app_id(&app_id) {
                    return Err("draft_app_invalid".into());
                }
                (
                    std::fs::canonicalize(workspace.join("data/apps").join(&app_id))?,
                    None,
                    Some(app_id),
                )
            }
        };
        if !path.is_dir() {
            return Err("draft_destination_missing".into());
        }
        let external = expected_repo.as_ref().map(|_| path.clone());
        let classified =
            classify_resolved_folder(&path, &workspace, source_path.as_deref(), |_| external)?;
        match classified {
            FolderClassification::Lucidos { .. }
                if expected_repo.is_none() && expected_app.is_none() =>
            {
                Ok((None, None))
            }
            FolderClassification::External { .. } if expected_repo.is_some() => {
                Ok((expected_repo, None))
            }
            FolderClassification::App { app_id, .. }
                if expected_app.as_deref() == Some(app_id.as_str()) =>
            {
                Ok((None, Some(app_id)))
            }
            _ => Err("draft_destination_changed".into()),
        }
    }
}

#[derive(sqlx::FromRow)]
struct Binding {
    state: String,
    source: String,
    coding_agent: Option<String>,
    cc_repo_id: Option<String>,
    coding_agent_kind: Option<String>,
    coding_agent_folder: Option<String>,
}

fn strict_backend(value: &str) -> Result<CodingAgent, Box<dyn std::error::Error + Send + Sync>> {
    CodingAgent::ALL
        .into_iter()
        .find(|backend| backend.as_str() == value)
        .ok_or_else(|| "draft_coding_backend_unsupported".into())
}

/// Who reads a refusal, so its next step names a command that caller has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalAudience {
    CodingAgent,
    ChatAgent,
}

/// A refusal as a transport reports it: an HTTP status, a stable reason slug
/// and a message that keeps the original slug and names the next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusalReport {
    pub status: u16,
    pub reason: &'static str,
    pub message: String,
}

pub fn refusal_report(
    error: &(dyn std::error::Error + Send + Sync),
    thread_id: Uuid,
    audience: RefusalAudience,
) -> RefusalReport {
    let text = error.to_string();
    let slug = text.split(':').next().unwrap_or_default().trim();
    let (status, reason, next) = match slug {
        "draft_snapshot_stale" => (
            409,
            "draft_snapshot_stale",
            format!(
                "The draft changed since you read it. Read it again with `lucidos threads drafts \
                 --thread {thread_id}`. To retry a send that may have landed, reuse the original \
                 retry key. If the content changed, confirm it with the owner, then submit the \
                 new revision and epoch."
            ),
        ),
        "draft_retry_key_conflict" => (
            409,
            "draft_retry_key_conflict",
            "That retry key already names a different send. Use a fresh key for a new send."
                .to_string(),
        ),
        "draft_owner_authority_missing" => (
            403,
            "draft_owner_authority_missing",
            match audience {
                RefusalAudience::CodingAgent => format!(
                    "The owner has not authorized this send. Ask with `lucidos ask-permission \
                     submit-draft --thread {thread_id} --reason \"<why>\"`, or wait for the owner \
                     to instruct you in their own turn."
                ),
                RefusalAudience::ChatAgent => "The owner has not authorized this send. Ask the \
                     owner to tell you to send it in their own message."
                    .to_string(),
            },
        ),
        "draft_submitter_unattributed" => (
            403,
            "draft_submitter_unattributed",
            "The engine could not attribute this send to an agent thread.".to_string(),
        ),
        "draft_missing" => (
            404,
            "draft_missing",
            "That thread holds no saved draft.".to_string(),
        ),
        "draft_snapshot_token_invalid" | "draft_submission_identity_invalid" => (
            400,
            "draft_request_invalid",
            "The revision, epoch, thread or retry key is malformed.".to_string(),
        ),
        "draft_queue_payload_mismatch"
        | "draft_payload_invalid"
        | "draft_model_registry_unavailable" => (
            500,
            "draft_submission_failed",
            "The engine could not accept the draft.".to_string(),
        ),
        other if other.starts_with("draft_") => (
            422,
            "draft_not_sendable",
            "The saved draft cannot be sent as saved. The owner must fix it in the composer."
                .to_string(),
        ),
        _ => (
            500,
            "draft_submission_failed",
            "The engine could not accept the draft.".to_string(),
        ),
    };
    RefusalReport {
        status,
        reason,
        message: format!("{text}. {next}"),
    }
}

impl super::LucidosEngine {
    /// Native entry point shared by agent transports. The engine resolves the
    /// owner's authority for `caller` and rechecks it at commit.
    pub async fn submit_saved_draft_as_agent(
        &self,
        caller: Uuid,
        request: SubmitDraftRequest,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        let provider = self.current_provider();
        let configured = self.configured_provider_kinds();
        self.draft_submission_service(provider.default_model(), &configured)
            .submit_as_agent(caller, request)
            .await
    }

    /// Tests supply the actor and authority directly.
    #[cfg(test)]
    pub async fn submit_saved_draft(
        &self,
        request: SubmitDraftRequest,
        actor: MessageOrigin,
        authority: DraftSubmissionAuthority,
    ) -> Result<DraftSubmissionReceipt, Box<dyn std::error::Error + Send + Sync>> {
        let provider = self.current_provider();
        let configured = self.configured_provider_kinds();
        self.draft_submission_service(provider.default_model(), &configured)
            .submit(request, actor, authority)
            .await
    }

    fn draft_submission_service<'a>(
        &'a self,
        fallback_model: &'a str,
        configured: &'a [ProviderKind],
    ) -> DraftSubmissionService<'a> {
        DraftSubmissionService {
            bus: &self.event_bus,
            pool: self.pool(),
            workspace: self.workspace_path(),
            defaults: ModelDefaults {
                registry: self.model_registry(),
                fallback_model,
            },
            configured,
        }
    }
}

#[cfg(test)]
#[path = "draft_submission_tests.rs"]
mod tests;
