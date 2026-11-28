//! INVARIANT: The write chokepoint for trigger lifecycle events: emit, then leave the
//! trigger registry consistent before returning.
//!
//! Sibling of [`crate::engine::trigger_group_writes`], which does the same for
//! trigger *groups*, and the reason this module exists is that triggers did
//! not. `trigger_configs` was materialized only by the scheduler's EventBus
//! subscriber, which runs asynchronously: `EventBus::emit` committed the event,
//! broadcast it, and returned, and the HTTP handler answered 200 while the
//! registry might still hold pre-write state. Under a loaded e2e suite that
//! window is wide enough that a `PUT {"paused": true}` answering
//! `success: true` was followed by a run request that fired the trigger it had
//! just paused. The same window is reachable from the LLM tool surface
//! (`pause_trigger` then `run_trigger` in one turn) with no HTTP involved.
//!
//! So every trigger write goes through [`TriggerRegistryWriter`], which gives
//! the whole surface read-your-writes: create, update, delete, pause and resume
//! alike, from HTTP, the LLM tools, plugin resync and the thread-queue overflow
//! guard.
//!
//! **Emit first, then apply, never the reverse.** A failed emit must leave the
//! registry untouched: a resume that is live in memory but absent from the
//! event log would come back paused after a restart, silently. Emit-then-apply
//! can only lose the *fast* apply (the subscriber still gets the broadcast), it
//! can never invent state the log does not carry.
//!
//! **And the pair is serialized**, under `LucidosEngine::trigger_write_lock`.
//! `emit` broadcasts before it returns, so two writers racing on one trigger
//! can interleave as A-emit, B-emit, B-apply, A-apply and leave the registry
//! holding A's older payload for good. The scheduler subscriber replays both in
//! log order but is free to have done so before A's late apply, so it is not
//! the backstop it looks like: the lock is.
//!
//! Callers name a [`TriggerWrite`] rather than building the `SystemEvent`
//! themselves. That is what makes the guarantee mechanical instead of
//! customary: the `SystemEvent::Trigger*` constructors exist only in this file,
//! so there is no way to spell a trigger write that skips the registry. A
//! source-scan test holds that line.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::permission::{
    menu, one_line, parsed_options, Action, PermissionCardText, PermissionChoice, RequestCard,
};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::triggers::config::TriggerProgramGrant;
use crate::triggers::folder_grant::TriggerFolderGrant;
use crate::triggers::registry::materialize_trigger_event;
use crate::triggers::TriggerConfig;
use uuid::Uuid;

pub(crate) const PREVIOUS_KEY: &str = "previous";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerWrite {
    Created,
    Updated,
    Deleted,
    /// INVARIANT: Pause. Distinct from an `Updated` carrying `{"paused": true}` only in
    /// provenance; both land on the same registry field. The engine emits this
    /// one from the thread-queue overflow guard and the legacy migration; the
    /// user-facing pause and resume both go through `Updated`.
    ///
    /// There is deliberately no `Enabled` counterpart: nothing emits
    /// `TriggerEnabled` today. The registry still materializes it, because
    /// workspaces created before the split have the event in their log and
    /// replay walks it.
    Disabled,
}

impl TriggerWrite {
    pub(crate) fn event_type(self) -> &'static str {
        match self {
            Self::Created => "TriggerCreated",
            Self::Updated => "TriggerUpdated",
            Self::Deleted => "TriggerDeleted",
            Self::Disabled => "TriggerDisabled",
        }
    }

    fn into_event(
        self,
        trigger_id: String,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> SystemEvent {
        match self {
            Self::Created => SystemEvent::TriggerCreated {
                trigger_id,
                payload,
                actor,
            },
            Self::Updated => SystemEvent::TriggerUpdated {
                trigger_id,
                payload,
                actor,
            },
            Self::Deleted => SystemEvent::TriggerDeleted {
                trigger_id,
                payload,
                actor,
            },
            Self::Disabled => SystemEvent::TriggerDisabled {
                trigger_id,
                payload,
                actor,
            },
        }
    }
}

pub struct TriggerRegistryWriter<'a> {
    pub event_bus: &'a EventBus,
    pub trigger_configs: &'a RwLock<HashMap<String, TriggerConfig>>,
    pub workspace_path: &'a Path,
    /// INVARIANT: Serializes emit + apply so the registry is written in the order the log
    /// records. See `LucidosEngine::trigger_write_lock` for what goes wrong
    /// without it.
    pub write_lock: &'a tokio::sync::Mutex<()>,
}

#[derive(Serialize, Deserialize)]
pub struct TriggerGrantProposal {
    trigger_id: String,
    definition_digest: String,
    grants: ProposedGrants,
}

#[derive(Serialize, Deserialize)]
enum ProposedGrants {
    Programs(Vec<String>),
    Folders(Vec<TriggerFolderGrant>),
    Named {
        programs: Vec<String>,
        folders: Vec<NamedFolder>,
    },
}

impl TriggerGrantProposal {
    pub fn card(&self, config: &TriggerConfig, caller: Uuid, reason: Option<&str>) -> RequestCard {
        let programs: &[String] = match &self.grants {
            ProposedGrants::Programs(programs) | ProposedGrants::Named { programs, .. } => programs,
            ProposedGrants::Folders(_) => &[],
        };
        let folders: Vec<String> = match &self.grants {
            ProposedGrants::Programs(_) => Vec::new(),
            ProposedGrants::Folders(folders) => folders
                .iter()
                .map(|grant| grant.folder.to_string_lossy().into_owned())
                .collect(),
            ProposedGrants::Named { folders, .. } => folders
                .iter()
                .map(|folder| {
                    let resolved = folder.grant.folder.to_string_lossy();
                    if resolved == folder.requested.as_str() {
                        folder.requested.clone()
                    } else {
                        format!("{} \u{2192} {resolved}", folder.requested)
                    }
                })
                .collect(),
        };
        let name = one_line(&config.name);
        let text = match (programs.is_empty(), folders.is_empty()) {
            (false, true) => {
                PermissionCardText::new(&format!("Let \u{201c}{name}\u{201d} run these programs?"))
                    .items(programs)
            }
            (true, false) => PermissionCardText::new(&format!(
                "Let \u{201c}{name}\u{201d} write in these folders?"
            ))
            .items(&folders),
            _ => PermissionCardText::new(&format!(
                "Let \u{201c}{name}\u{201d} run these programs and write in these folders?"
            ))
            .paragraph("Programs:")
            .items(programs)
            .paragraph("Folders:")
            .items(&folders),
        };
        let question = text.finish(reason);
        let action = if matches!(&self.grants, ProposedGrants::Programs(_))
            || matches!(&self.grants, ProposedGrants::Named { folders, .. } if folders.is_empty())
        {
            Action::GrantTriggerPrograms
        } else {
            Action::GrantTriggerFolders
        };
        let mut options = menu::options(
            action,
            caller,
            "Save nothing. The trigger keeps its grants, and a later card may ask again.",
        );
        for option in &mut options {
            match option.choice {
                PermissionChoice::Grant { .. } => {
                    option.description =
                        "Save the listed access for future runs of this trigger.".into();
                }
                PermissionChoice::RefuseStanding => {
                    option.description = "Deny the listed access for this trigger only. A run \
                                          that needs it ends at once, without a card."
                        .into();
                }
                _ => {}
            }
        }
        RequestCard {
            question,
            header: "Permission",
            action,
            breadth: Some(serde_json::to_string(self).expect("trigger grants are JSON strings")),
            options,
            batch: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerGrantDecision {
    Allow,
    Deny,
}

fn deny_payload(config: &TriggerConfig, next: &TriggerConfig) -> Value {
    serde_json::json!({
        "trigger_id": config.id,
        "program_grants": next.program_grants,
        "folder_grants": next.folder_grants,
        "program_denies": next.program_denies,
        "folder_denies": next.folder_denies,
        PREVIOUS_KEY: config.to_created_payload(),
    })
}

/// INVARIANT: Additions named by a caller. They stay separate from the durable grant lists
/// until the owner approves the resulting card.
#[derive(Default, Deserialize)]
pub struct RequestedTriggerGrants {
    #[serde(default)]
    pub folder_grants: Vec<String>,
    #[serde(default)]
    pub program_grants: Vec<String>,
}

impl RequestedTriggerGrants {
    pub fn is_empty(&self) -> bool {
        self.folder_grants.is_empty() && self.program_grants.is_empty()
    }
}

#[derive(Serialize, Deserialize)]
struct NamedFolder {
    requested: String,
    grant: TriggerFolderGrant,
}

fn displayable_path(path: &str) -> bool {
    !path.chars().any(|ch| {
        ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

fn named_folder(path: &str, workspace: &Path) -> Result<TriggerFolderGrant, String> {
    let folder = Path::new(path);
    if !folder.is_absolute()
        || folder.components().any(|part| part == Component::ParentDir)
        || folder.parent().is_none()
    {
        return Err(format!(
            "Use an absolute folder path without '..', other than '/': {path:?}"
        ));
    }
    if !displayable_path(path) {
        return Err(format!(
            "The folder cannot be displayed on the permission card: {path:?}"
        ));
    }
    let canonical = folder
        .canonicalize()
        .map_err(|_| format!("Folder does not exist: {path:?}"))?;
    let home = std::env::home_dir();
    if canonical.parent().is_none()
        || home.as_ref().is_some_and(|home| {
            folder == home || home.canonicalize().is_ok_and(|home| canonical == home)
        })
    {
        return Err(format!(
            "The root and home folders cannot be granted: {path:?}"
        ));
    }
    if !canonical.to_str().is_some_and(displayable_path) {
        return Err(format!(
            "The resolved folder cannot be displayed on the permission card: {path:?}"
        ));
    }
    TriggerFolderGrant::inspect(&canonical, workspace)
        .filter(|grant| grant.folder == canonical)
        .ok_or_else(|| format!("This folder cannot be granted: {path:?}"))
}

fn program_pattern(program: &str) -> String {
    if program == "Python" {
        program.into()
    } else {
        format!("Bash({program}:*)")
    }
}

impl TriggerRegistryWriter<'_> {
    pub async fn propose_grants(
        &self,
        config: &TriggerConfig,
        action: Action,
    ) -> Result<Option<TriggerGrantProposal>, Box<dyn std::error::Error + Send + Sync>> {
        let definition_digest = config.definition_digest();
        let pool = self.event_bus.pool();
        let grants = match action {
            Action::GrantTriggerPrograms => {
                use crate::engine::command_guard::{
                    self, CommandSite, SecretPaths, StaticVerdict, TrustedHeads,
                };
                use crate::llm::tool_names::{
                    RUN_BASH, RUN_BASH_BACKGROUND, RUN_PYTHON, RUN_PYTHON_BACKGROUND,
                };
                let calls: Vec<(String, Value)> = sqlx::query_as(
                    "SELECT DISTINCT call.payload->>'name', call.payload->'args'
                       FROM events run
                       JOIN events call ON call.thread_id = run.thread_id
                                       AND call.event_type = 'ToolCalled'
                                       AND call.payload->>'request_event_id' = run.id::text
                       JOIN events result ON result.thread_id = call.thread_id
                                         AND result.event_type = 'ToolResult'
                                         AND result.payload->>'tool_called_event_id' = call.id::text
                      WHERE run.event_type = 'TriggerStarted'
                        AND run.payload->>'trigger_id' = $1
                        AND call.payload->>'name' = ANY($2)
                        AND call.payload->'args' IS NOT NULL
                        AND result.payload->'success' = 'true'::jsonb",
                )
                .bind(&config.id)
                .bind(
                    &[
                        RUN_BASH,
                        RUN_BASH_BACKGROUND,
                        RUN_PYTHON,
                        RUN_PYTHON_BACKGROUND,
                    ][..],
                )
                .fetch_all(pool)
                .await?;
                let site = CommandSite::Workspace(self.workspace_path);
                let paths = SecretPaths::conservative();
                let mut patterns = BTreeSet::new();
                for (tool, args) in calls {
                    let Some(command) = command_guard::command_text(&tool, &args) else {
                        continue;
                    };
                    if matches!(
                        command_guard::static_classify_with_paths(
                            &tool,
                            &args,
                            site,
                            &TrustedHeads::NONE,
                            &paths,
                        ),
                        StaticVerdict::Settled(_)
                    ) {
                        continue;
                    }
                    if let Some(pattern) = super::command_permission::trigger_program_pattern(
                        &tool, command, site, &paths,
                    ) {
                        if !config
                            .program_grants
                            .iter()
                            .any(|grant| grant.legacy.is_none() && grant.pattern == pattern)
                        {
                            patterns.insert(pattern);
                        }
                    }
                }
                if patterns.is_empty() {
                    return Ok(None);
                }
                ProposedGrants::Programs(patterns.into_iter().collect())
            }
            Action::GrantTriggerFolders => {
                let recorded: Vec<sqlx::types::Json<TriggerFolderGrant>> = sqlx::query_scalar(
                    "SELECT DISTINCT item->'grant'
                       FROM events
                       CROSS JOIN LATERAL jsonb_array_elements(
                           payload #> '{permission_request,batch,items}') AS item
                      WHERE event_type IN ('UserQuestionAsked', 'UserQuestionQueued')
                        AND payload #>> '{permission_request,action}' = 'run-batch'
                        AND payload #>> '{permission_request,prepared}' IS NULL
                        AND item->>'kind' = 'trigger_folder_grant'
                        AND item->>'trigger_id' = $1
                        AND item->>'definition_digest' = $2
                        AND COALESCE(item->>'run_thread_id', item->>'failed_thread_id') = thread_id::text
                      ORDER BY item->'grant'",
                )
                .bind(&config.id)
                .bind(&definition_digest)
                .fetch_all(pool)
                .await?;
                let grants: Vec<_> = recorded
                    .into_iter()
                    .map(|grant| grant.0)
                    .filter(|grant| {
                        !config.folder_grants.contains(grant)
                            && self.folder_grant_matches_card(grant)
                    })
                    .collect();
                if grants.is_empty() {
                    return Ok(None);
                }
                ProposedGrants::Folders(grants)
            }
            _ => return Ok(None),
        };
        Ok(Some(TriggerGrantProposal {
            trigger_id: config.id.clone(),
            definition_digest,
            grants,
        }))
    }

    fn folder_grant_matches_card(&self, grant: &TriggerFolderGrant) -> bool {
        grant
            .folder
            .to_str()
            .is_some_and(|path| named_folder(path, self.workspace_path).as_ref() == Ok(grant))
    }

    pub fn propose_named_grants(
        &self,
        config: &TriggerConfig,
        requested: RequestedTriggerGrants,
    ) -> Result<Option<TriggerGrantProposal>, String> {
        let mut folders: Vec<NamedFolder> = Vec::new();
        for requested in requested.folder_grants {
            let grant = named_folder(&requested, self.workspace_path)?;
            if !config.folder_grants.contains(&grant)
                && !folders.iter().any(|entry| entry.requested == requested)
            {
                folders.push(NamedFolder { requested, grant });
            }
        }
        let mut programs = BTreeSet::new();
        for program in requested.program_grants {
            if program.is_empty()
                || program.starts_with('-')
                || matches!(program.as_str(), "." | "..")
                || super::command_guard::grants_nothing_by_name(&program)
                || !program
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"._-+".contains(&ch))
            {
                return Err(format!(
                    "Name one program, without arguments or wildcards: {program:?}"
                ));
            }
            let pattern = program_pattern(&program);
            if !config
                .program_grants
                .iter()
                .any(|grant| grant.legacy.is_none() && grant.pattern == pattern)
            {
                programs.insert(program);
            }
        }
        if folders.is_empty() && programs.is_empty() {
            return Ok(None);
        }
        Ok(Some(TriggerGrantProposal {
            trigger_id: config.id.clone(),
            definition_digest: config.definition_digest(),
            grants: ProposedGrants::Named {
                programs: programs.into_iter().collect(),
                folders,
            },
        }))
    }

    pub async fn grant_card_channel(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<EventChannel>, sqlx::Error> {
        let source: Option<String> = sqlx::query_scalar(
            "SELECT source FROM thread_summaries WHERE thread_id = $1 AND NOT is_home",
        )
        .bind(thread_id)
        .fetch_optional(self.event_bus.pool())
        .await?;
        Ok(source.map(|source| EventChannel::from_wire(&source).unwrap_or(EventChannel::Chat)))
    }

    pub(crate) async fn record_grant_answer(
        &self,
        proposal: TriggerGrantProposal,
        decision: TriggerGrantDecision,
        thread_id: Uuid,
        answer: (ThreadEvent, EventMeta),
        placement: Option<(Option<super::InputPosition>, Option<i64>)>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let (answer, meta) = answer;
        if !matches!(meta.actor, Some(MessageOrigin::Device { .. })) {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let config = self
            .trigger_configs
            .read()
            .unwrap()
            .get(&proposal.trigger_id)
            .cloned();
        // INVARIANT: A deny only takes power away, so a changed definition refuses only a grant.
        let Some(config) = config.filter(|config| {
            matches!(decision, TriggerGrantDecision::Deny)
                || config.definition_digest() == proposal.definition_digest
        }) else {
            return Ok(false);
        };
        let (programs, folders) = match proposal.grants {
            ProposedGrants::Programs(programs) => (programs, Vec::new()),
            ProposedGrants::Folders(folders) => {
                if !folders
                    .iter()
                    .all(|grant| self.folder_grant_matches_card(grant))
                {
                    return Ok(false);
                }
                (Vec::new(), folders)
            }
            ProposedGrants::Named { programs, folders } => {
                let mut grants = Vec::with_capacity(folders.len());
                for folder in folders {
                    if named_folder(&folder.requested, self.workspace_path).as_ref()
                        != Ok(&folder.grant)
                    {
                        return Ok(false);
                    }
                    grants.push(folder.grant);
                }
                (
                    programs
                        .iter()
                        .map(|program| program_pattern(program))
                        .collect(),
                    grants,
                )
            }
        };
        let mut next = config.clone();
        let payload = match decision {
            TriggerGrantDecision::Deny => {
                let folders: Vec<_> = folders.into_iter().map(|grant| grant.folder).collect();
                next.deny(&programs, &folders);
                deny_payload(&config, &next)
            }
            TriggerGrantDecision::Allow => {
                for pattern in programs {
                    let grant = TriggerProgramGrant {
                        pattern,
                        legacy: None,
                    };
                    if !next.program_grants.contains(&grant) {
                        next.program_grants.push(grant);
                    }
                }
                for grant in folders {
                    if !next.folder_grants.contains(&grant) {
                        next.folder_grants
                            .retain(|held| held.folder != grant.folder);
                        next.folder_grants.push(grant);
                    }
                }
                serde_json::json!({
                    "trigger_id": config.id, "program_grants": next.program_grants,
                    "folder_grants": next.folder_grants, PREVIOUS_KEY: config.to_created_payload(),
                })
            }
        };
        let write = TriggerWrite::Updated;
        let update = write.into_event(config.id.clone(), payload.clone(), meta.actor.clone());
        if !self
            .event_bus
            .emit_question_with_trigger_write(thread_id, answer, meta, placement, update)
            .await?
        {
            return Ok(false);
        }
        self.apply(write, &config.id, &payload);
        Ok(true)
    }

    pub(crate) async fn deny(
        &self,
        trigger_id: &str,
        programs: &[String],
        folders: &[std::path::PathBuf],
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !matches!(actor, Some(MessageOrigin::Device { .. })) {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let payload = {
            let configs = self.trigger_configs.read().unwrap();
            let Some(config) = configs.get(trigger_id) else {
                return Ok(false);
            };
            let mut next = config.clone();
            next.deny(programs, folders);
            if next.program_denies == config.program_denies
                && next.folder_denies == config.folder_denies
            {
                return Ok(true);
            }
            deny_payload(config, &next)
        };
        self.emit_then_apply(TriggerWrite::Updated, trigger_id, payload, actor)
            .await?;
        Ok(true)
    }

    pub(crate) async fn append_program_grant(
        &self,
        trigger_id: &str,
        definition_digest: &str,
        grant: TriggerProgramGrant,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.append_program_grants(trigger_id, definition_digest, vec![grant], actor)
            .await
    }

    async fn append_program_grants(
        &self,
        trigger_id: &str,
        definition_digest: &str,
        proposed: Vec<TriggerProgramGrant>,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if proposed.is_empty()
            || proposed.iter().any(|grant| grant.legacy.is_some())
            || !matches!(actor, Some(MessageOrigin::Device { .. }))
        {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let grants = {
            let configs = self.trigger_configs.read().unwrap();
            let Some(config) = configs.get(trigger_id) else {
                return Ok(false);
            };
            if config.definition_digest() != definition_digest {
                return Ok(false);
            }
            let mut grants = config.program_grants.clone();
            for grant in proposed {
                if !grants.contains(&grant) {
                    grants.push(grant);
                }
            }
            if grants == config.program_grants {
                return Ok(true);
            }
            grants
        };
        let payload = serde_json::json!({ "trigger_id": trigger_id, "program_grants": grants });
        self.emit_then_apply(TriggerWrite::Updated, trigger_id, payload, actor)
            .await?;
        Ok(true)
    }

    pub(crate) async fn append_folder_grant(
        &self,
        trigger_id: &str,
        definition_digest: &str,
        grant: &TriggerFolderGrant,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.append_folder_grants(trigger_id, definition_digest, vec![grant.clone()], actor)
            .await
    }

    async fn append_folder_grants(
        &self,
        trigger_id: &str,
        definition_digest: &str,
        proposed: Vec<TriggerFolderGrant>,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if proposed.is_empty() || !matches!(actor, Some(MessageOrigin::Device { .. })) {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let grants = {
            let configs = self.trigger_configs.read().unwrap();
            let Some(config) = configs.get(trigger_id) else {
                return Ok(false);
            };
            if config.definition_digest() != definition_digest
                || proposed
                    .iter()
                    .any(|grant| !grant.still_valid(self.workspace_path))
            {
                return Ok(false);
            }
            let mut grants = config.folder_grants.clone();
            for grant in proposed {
                if !grants.contains(&grant) {
                    grants.retain(|held| held.folder != grant.folder);
                    grants.push(grant);
                }
            }
            if grants == config.folder_grants {
                return Ok(true);
            }
            grants
        };
        let payload = serde_json::json!({ "trigger_id": trigger_id, "folder_grants": grants });
        self.emit_then_apply(TriggerWrite::Updated, trigger_id, payload, actor)
            .await?;
        Ok(true)
    }

    /// INVARIANT: Mint this trigger's slug and write it, both under the one guard.
    ///
    /// The slug is the `data/triggers/<slug>/` directory segment, so two
    /// triggers must never share one. Minting it outside the lock does not
    /// guarantee that: two creates of the same name can both read the taken
    /// set before either has emitted, and both then mint the same slug.
    ///
    /// `payload` arrives WITHOUT a slug. An explicit one from the caller is
    /// already in it and is left alone, since the caller named the directory
    /// they mean to write to.
    pub async fn write_created_minting_slug(
        &self,
        trigger_id: &str,
        mut payload: Value,
        name: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _guard = self.write_lock.lock().await;
        self.mint_slug(trigger_id, &mut payload, name);
        self.emit_then_apply(TriggerWrite::Created, trigger_id, payload, actor)
            .await
    }

    /// INVARIANT: Emit a trigger lifecycle event and materialize it into the registry
    /// before returning, so the caller's next read observes its own write.
    ///
    /// A failed emit propagates and applies nothing. Arms no cron job: that
    /// stays with the scheduler's subscriber, which owns `tracked_tasks` and
    /// reacts to the same broadcast.
    pub(crate) async fn write(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Held across both steps: `emit` broadcasts before it returns, so an
        // unguarded writer can be preempted between emitting and applying,
        // and then land its older payload on top of a newer writer's.
        let _guard = self.write_lock.lock().await;
        self.emit_then_apply(write, trigger_id, payload, actor)
            .await
    }

    pub(crate) async fn write_if(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        mut payload: Value,
        actor: Option<MessageOrigin>,
        applies: impl FnOnce(Option<&TriggerConfig>, &Value) -> bool,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _guard = self.write_lock.lock().await;
        {
            let configs = self.trigger_configs.read().unwrap();
            if !applies(configs.get(trigger_id), &payload) {
                return Ok(false);
            }
            if write == TriggerWrite::Created {
                let taken: std::collections::HashSet<String> =
                    configs.values().map(|c| c.slug.clone()).collect();
                let slug = payload.get("slug").and_then(Value::as_str);
                if slug.is_some_and(|slug| taken.contains(slug)) {
                    let name = payload["name"].as_str().unwrap_or(trigger_id).to_string();
                    payload["slug"] = Value::String(crate::triggers::mint_unique_trigger_slug(
                        &name, trigger_id, &taken,
                    ));
                }
            }
        }
        self.emit_then_apply(write, trigger_id, payload, actor)
            .await?;
        Ok(true)
    }

    /// INVARIANT: Save a creation or edit and publish its grant card before the scheduler
    /// sees the new definition. The payload never carries requested grants.
    pub async fn write_with_requested_grants(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        mut payload: Value,
        actor: Option<MessageOrigin>,
        requested: RequestedTriggerGrants,
        host: Option<Uuid>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _guard = self.write_lock.lock().await;
        if write == TriggerWrite::Created {
            let name = payload["name"].as_str().unwrap_or(trigger_id).to_owned();
            self.mint_slug(trigger_id, &mut payload, &name);
        }
        self.prepare_payload(write, trigger_id, &mut payload, actor.as_ref());
        if !requested.is_empty() {
            let next = match write {
                TriggerWrite::Created => TriggerConfig::from_created_payload(&payload)?,
                TriggerWrite::Updated => {
                    let mut next = self
                        .trigger_configs
                        .read()
                        .unwrap()
                        .get(trigger_id)
                        .cloned()
                        .ok_or("Trigger not found")?;
                    next.apply_update(&payload);
                    next
                }
                _ => return Err("A trigger grant requires a create or update".into()),
            };
            if let Some(proposal) = self.propose_named_grants(&next, requested)? {
                let caller = host.ok_or(
                    "Request trigger grants from the thread creating or editing the trigger.",
                )?;
                let channel = self
                    .grant_card_channel(caller)
                    .await?
                    .ok_or("Request trigger grants from a work thread outside Home.")?;
                let (words, request) = proposal.card(&next, caller, None).into_parts();
                let question = ThreadEvent::UserQuestionAsked {
                    tool_use_id: format!(
                        "{}{}",
                        crate::engine::thread_events::permission::request::CARD_ID_PREFIX,
                        Uuid::new_v4()
                    ),
                    cc_session_id: String::new(),
                    options: parsed_options(&words),
                    question: words.question,
                    worktree_path: None,
                    multi_select: false,
                    owner_approval: None,
                    permission_request: Some(request),
                };
                let meta = EventMeta {
                    channel: Some(channel),
                    ..EventMeta::NONE
                };
                let event = write.into_event(trigger_id.to_string(), payload.clone(), actor);
                if !self
                    .event_bus
                    .emit_question_with_trigger_write(caller, question, meta, None, event)
                    .await?
                {
                    return Err("The trigger grant card could not be recorded.".into());
                }
                self.apply(write, trigger_id, &payload);
                return Ok(true);
            }
        }
        let event = write.into_event(trigger_id.to_string(), payload.clone(), actor);
        self.event_bus.emit(BusEvent::System(event)).await?;
        self.apply(write, trigger_id, &payload);
        Ok(false)
    }

    fn mint_slug(&self, trigger_id: &str, payload: &mut Value, name: &str) {
        if payload.get("slug").and_then(Value::as_str).is_none() {
            let taken = self
                .trigger_configs
                .read()
                .unwrap()
                .values()
                .map(|c| c.slug.clone())
                .collect();
            let slug = crate::triggers::mint_unique_trigger_slug(name, trigger_id, &taken);
            payload["slug"] = Value::String(slug);
        }
    }

    fn prepare_payload(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: &mut Value,
        actor: Option<&MessageOrigin>,
    ) {
        if matches!(write, TriggerWrite::Updated | TriggerWrite::Deleted) {
            let configs = self.trigger_configs.read().unwrap();
            if let Some(previous) = configs.get(trigger_id) {
                if write == TriggerWrite::Updated
                    && !matches!(actor, Some(MessageOrigin::Device { .. }))
                    && (!previous.program_grants.is_empty() || !previous.folder_grants.is_empty())
                {
                    let mut next = previous.clone();
                    next.apply_update(payload);
                    if previous.unowned_edit_clears_grants(&next) {
                        // INVARIANT: A grant may have arrived after the caller read the
                        // trigger. Check again under this writer's lock.
                        TriggerConfig::clear_grants_in(payload);
                    }
                }
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(PREVIOUS_KEY.to_string(), previous.to_created_payload());
                }
            }
        }
    }

    fn apply(&self, write: TriggerWrite, trigger_id: &str, payload: &Value) {
        materialize_trigger_event(
            self.trigger_configs,
            self.workspace_path,
            write.event_type(),
            trigger_id,
            payload,
        );
    }

    /// INVARIANT: The caller holds the trigger write lock across emission and apply.
    async fn emit_then_apply(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        mut payload: Value,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.prepare_payload(write, trigger_id, &mut payload, actor.as_ref());
        let event = write.into_event(trigger_id.to_string(), payload.clone(), actor);
        self.event_bus.emit(BusEvent::System(event)).await?;
        self.apply(write, trigger_id, &payload);
        Ok(())
    }

    pub(crate) async fn write_or_log(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
        module: &str,
    ) {
        if let Err(e) = self.write(write, trigger_id, payload, actor).await {
            crate::log_legacy!(
                "[EventBus] {} {} emit failed: {}",
                module,
                write.event_type(),
                e
            );
        }
    }
}

impl LucidosEngine {
    pub fn trigger_registry_writer(&self) -> TriggerRegistryWriter<'_> {
        TriggerRegistryWriter {
            event_bus: &self.event_bus,
            trigger_configs: &self.trigger_configs,
            workspace_path: &self.workspace_path,
            write_lock: &self.trigger_write_lock,
        }
    }

    pub async fn emit_trigger_write(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.trigger_registry_writer()
            .write(write, trigger_id, payload, actor)
            .await
    }

    pub async fn emit_trigger_write_or_log(
        &self,
        write: TriggerWrite,
        trigger_id: &str,
        payload: Value,
        actor: Option<MessageOrigin>,
        module: &str,
    ) {
        self.trigger_registry_writer()
            .write_or_log(write, trigger_id, payload, actor, module)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::registry::TRIGGER_LIFECYCLE_EVENTS;

    const ALL_WRITES: [TriggerWrite; 4] = [
        TriggerWrite::Created,
        TriggerWrite::Updated,
        TriggerWrite::Deleted,
        TriggerWrite::Disabled,
    ];

    #[test]
    fn every_write_names_an_event_the_registry_applies() {
        let named: Vec<&str> = ALL_WRITES.iter().map(|k| k.event_type()).collect();
        for kind in ALL_WRITES {
            assert!(
                TRIGGER_LIFECYCLE_EVENTS.contains(&kind.event_type()),
                "{:?} names {}, which the registry ignores",
                kind,
                kind.event_type()
            );
        }
        let unwritten: Vec<&str> = TRIGGER_LIFECYCLE_EVENTS
            .into_iter()
            .filter(|e| !named.contains(e))
            .collect();
        assert_eq!(
            unwritten,
            vec!["TriggerEnabled"],
            "a lifecycle event with no TriggerWrite can only be reached by replay; \
             add a variant if something now writes it"
        );
    }

    #[test]
    fn a_write_builds_the_event_it_names() {
        for kind in ALL_WRITES {
            let event = kind.into_event(
                "t-1".to_string(),
                serde_json::json!({ "trigger_id": "t-1" }),
                None,
            );
            assert_eq!(event.event_type(), kind.event_type());
            assert_eq!(event.aggregate_id(), "t-1");
        }
    }

    #[tokio::test]
    async fn trigger_program_grants_require_an_owner_and_clear_on_an_unowned_edit() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        use crate::triggers::config::TriggerProgramGrant;

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let workspace = tempfile::tempdir().unwrap();
        let configs = RwLock::new(HashMap::new());
        let lock = tokio::sync::Mutex::new(());
        let writer = TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: workspace.path(),
            write_lock: &lock,
        };
        writer
            .write(
                TriggerWrite::Created,
                "t",
                serde_json::json!({
                    "trigger_id": "t", "name": "Trigger", "schedule": ["0 0 2 * * *"],
                    "timezone": "UTC", "run": { "type": "intent", "intent": "Original" }
                }),
                None,
            )
            .await
            .unwrap();
        let digest = configs.read().unwrap()["t"].definition_digest();
        let grant = TriggerProgramGrant {
            pattern: "Python".into(),
            legacy: None,
        };
        assert!(!writer
            .append_program_grant("t", &digest, grant.clone(), None)
            .await
            .unwrap());

        let owner = Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        });
        for _ in 0..2 {
            assert!(writer
                .append_program_grant("t", &digest, grant.clone(), owner.clone())
                .await
                .unwrap());
        }
        assert_eq!(
            configs.read().unwrap()["t"].program_grants.as_slice(),
            std::slice::from_ref(&grant)
        );
        writer
            .write(
                TriggerWrite::Updated,
                "t",
                serde_json::json!({
                    "trigger_id": "t", "run": { "type": "intent", "intent": "Different" }
                }),
                None,
            )
            .await
            .unwrap();
        assert!(configs.read().unwrap()["t"].program_grants.is_empty());
        assert!(!writer
            .append_program_grant("t", &digest, grant, owner)
            .await
            .unwrap());

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_leave_the_registry_where_the_log_ends() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        use crate::triggers::TriggerConfig;
        use std::collections::HashMap;
        use std::sync::{Arc, RwLock};

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = Arc::new(tempfile::tempdir().unwrap());
        let configs: Arc<RwLock<HashMap<String, TriggerConfig>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let write_lock = Arc::new(tokio::sync::Mutex::new(()));
        let bus = Arc::new(bus);

        TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        }
        .write(
            TriggerWrite::Created,
            "t-1",
            serde_json::json!({
                "trigger_id": "t-1",
                "name": "Start",
                "slug": "racy",
                "schedule": ["0 0 2 * * *"],
                "timezone": "UTC",
                "run": { "type": "intent", "intent": "x" },
            }),
            None,
        )
        .await
        .expect("create");

        let mut handles = Vec::new();
        for writer_id in 0..2 {
            let (bus, configs, ws, write_lock) =
                (bus.clone(), configs.clone(), ws.clone(), write_lock.clone());
            handles.push(tokio::spawn(async move {
                for round in 0..8 {
                    TriggerRegistryWriter {
                        event_bus: &bus,
                        trigger_configs: &configs,
                        workspace_path: ws.path(),
                        write_lock: &write_lock,
                    }
                    .write(
                        TriggerWrite::Updated,
                        "t-1",
                        serde_json::json!({
                            "trigger_id": "t-1",
                            "name": format!("w{writer_id}-r{round}"),
                        }),
                        None,
                    )
                    .await
                    .expect("update");
                }
            }));
        }
        for h in handles {
            h.await.expect("writer task panicked");
        }

        let last_logged: String = sqlx::query_scalar(
            "SELECT payload->>'name' FROM events \
             WHERE event_type = 'TriggerUpdated' AND aggregate_id = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind("t-1")
        .fetch_one(&pool)
        .await
        .expect("last logged name");

        assert_eq!(
            configs.read().unwrap()["t-1"].name,
            last_logged,
            "the registry must agree with the newest event in the log, not with \
             whichever writer applied last"
        );
        teardown_test_db(&db).await;
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn only_the_chokepoint_constructs_a_trigger_lifecycle_event() {
            use crate::test_support::source_scan::production_sources;

            const ALLOWED: [&str; 2] = ["engine/trigger_writes.rs", "scheduler/bus_subscriber.rs"];

            let mut offenders: Vec<String> = Vec::new();
            for (rel, text) in production_sources() {
                if ALLOWED.contains(&rel.as_str()) {
                    continue;
                }
                for event in TRIGGER_LIFECYCLE_EVENTS {
                    let ctor = format!("SystemEvent::{} {{", event);
                    if text.contains(&ctor) && !text.contains(&format!("{} .. }}", ctor)) {
                        offenders.push(format!("{rel}: {event}"));
                    }
                }
            }
            offenders.sort();
            assert!(
                offenders.is_empty(),
                "a trigger lifecycle event must be written through \
             LucidosEngine::emit_trigger_write (or the free emit_trigger_write), \
             so the registry is consistent before the caller's next read. \
             Constructed outside the chokepoint in:\n  {}",
                offenders.join("\n  ")
            );
        }
    }
}
