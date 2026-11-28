//! The workspace's *event hooks* (ADR 0746): `GET /api/v1/hooks` lists them,
//! and `POST /api/v1/hooks/:slug/arm`, `POST /api/v1/hooks/:slug/disarm` and
//! `DELETE /api/v1/hooks/:slug` arm, disarm and remove one.
//!
//! The `lucidos-event-hooks` leaf crate reads the definitions, through the
//! [`EventHooks`](crate::core::event_hooks::EventHooks) interface that the
//! executable hands to `create_router`. A
//! hook's arming is its latest arming event.
//!
//! Only the owner arms, disarms or removes a hook:
//!
//! - **The owner acts.** A stamped device, or the machine-local token while
//!   it proves the owner. An app document or a device session never does.
//! - **An agent spends a once permission** for the exact act. With none, it
//!   gets `permission_required` and asks with the request id. An arm names the
//!   content digest, so an edit after Allow asks again.
//! - **An unattested caller gets a 403.**
//!
//! An arm may name the digest its caller was shown, and a hook that changed
//! since then is refused. Every check runs before the gate, so nobody is asked
//! about an act Lucidos would refuse. No hook can name an arming event, so no
//! hook stops these.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::actor::{owner_attestation, user_actor, OwnerAttestation};
use crate::api::error::ApiError;
use crate::api::permission_requests::{
    calling_agent_thread, hook_ask, permission_required, spend_or_prepare, Spent,
};
use crate::api::secret_reveal::{reveal_request_allowed, RefererRule};
use crate::api::AppState;
use crate::core::event_hooks::{
    armed_hooks, effective_timeout, ArmState, ArmedHook, HookListing, FIRST_MESSAGE_EVENT_TYPE,
    HOOKS_DIR,
};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::permission::Action;
use crate::engine::thread_events::MessageOrigin;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/hooks", get(list_hooks))
        .route("/hooks/:slug", delete(remove_hook))
        .route("/hooks/pending/:message/cancel", post(cancel_hook_input))
        .route(
            "/hooks/:slug/arguments/:argument/choices",
            get(hook_choices),
        )
        .route("/hooks/:slug/arm", post(arm_hook))
        .route("/hooks/:slug/disarm", post(disarm_hook))
}

async fn cancel_hook_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    if actor.mode() != crate::engine::thread_events::ActorMode::Human {
        return Err(StatusCode::FORBIDDEN.into());
    }
    let canceled = state
        .engine
        .event_bus
        .cancel_hook_input(message, &actor)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if canceled {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(
            "This message has no pending event hook input.",
        ))
    }
}

/// A hook as the list shows it: its folder, then its arming.
#[derive(Debug, Serialize)]
struct HookRow {
    #[serde(flatten)]
    folder: Folder,
    arm_state: ArmState,
    armed: Option<ArmedHook>,
    /// Why the owner cannot arm it now. `None` when they can.
    arm_refusal: Option<String>,
    /// Why the owner cannot remove it here. `None` when they can.
    remove_refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    choices_timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Folder {
    Listed(HookListing),
    Missing(MissingFolder),
}

/// An armed hook whose folder left `data/hooks/`.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename = "missing")]
struct MissingFolder {
    slug: String,
}

impl HookRow {
    fn new(
        folder: Folder,
        armed: Option<ArmedHook>,
        remove_refusal: Option<String>,
        asked: &[&str],
    ) -> Self {
        let listing = match &folder {
            Folder::Listed(listing) => Some(listing),
            Folder::Missing(_) => None,
        };
        let arm_state = ArmState::of(armed.as_ref(), listing);
        let choices_timeout_secs = match listing {
            Some(HookListing::Valid { definition, .. })
                if arm_state == ArmState::Armed
                    && definition.event_type == FIRST_MESSAGE_EVENT_TYPE
                    && !definition.arguments.is_empty() =>
            {
                Some(effective_timeout(definition.timeout_secs).as_secs())
            }
            _ => None,
        };
        let mut row = Self {
            folder,
            arm_state,
            armed,
            arm_refusal: None,
            remove_refusal,
            plugin_id: None,
            choices_timeout_secs,
        };
        row.arm_refusal = row.armable(asked).err();
        row
    }

    fn slug(&self) -> &str {
        match &self.folder {
            Folder::Listed(listing) => listing.slug(),
            Folder::Missing(missing) => &missing.slug,
        }
    }

    /// What arming this hook binds: its event type and content digest. `Err`
    /// says why the owner cannot arm it now.
    fn armable(&self, asked: &[&str]) -> Result<(&str, &str), String> {
        let (definition, content_hash) = match &self.folder {
            Folder::Listed(HookListing::Valid {
                definition,
                content_hash,
                ..
            }) => (definition, content_hash),
            Folder::Listed(HookListing::Invalid { .. }) => {
                return Err("Fix the problems listed first.".to_string())
            }
            Folder::Missing(_) => {
                return Err(format!("Its folder is gone from data/{HOOKS_DIR}/."))
            }
        };
        if self.arm_state == ArmState::Armed {
            return Err("It is armed already.".to_string());
        }
        if !asked.contains(&definition.event_type.as_str()) {
            return Err(format!(
                "Lucidos does not ask hooks about {} events yet, so this hook could never run.",
                definition.event_type
            ));
        }
        Ok((&definition.event_type, content_hash))
    }
}

/// One row per folder, plus one per armed hook whose folder is gone, sorted
/// by slug. `folders` pairs each listing with its removal refusal.
fn rows(
    folders: Vec<(HookListing, Option<String>)>,
    mut armed: Vec<ArmedHook>,
    asked: &[&str],
) -> Vec<HookRow> {
    let mut rows: Vec<HookRow> = folders
        .into_iter()
        .map(|(listing, remove_refusal)| {
            let arming = armed
                .iter()
                .position(|a| a.slug == listing.slug())
                .map(|i| armed.swap_remove(i));
            HookRow::new(Folder::Listed(listing), arming, remove_refusal, asked)
        })
        .collect();
    rows.extend(armed.into_iter().map(|arming| {
        let folder = Folder::Missing(MissingFolder {
            slug: arming.slug.clone(),
        });
        HookRow::new(folder, Some(arming), None, asked)
    }));
    rows.sort_by(|a, b| a.slug().cmp(b.slug()));
    rows
}

async fn read_rows(state: &AppState) -> Result<Vec<HookRow>, ApiError> {
    let event_hooks = state.event_hooks.clone();
    let workspace = state.workspace_path.clone();
    let folders = tokio::task::spawn_blocking(move || {
        let listings = event_hooks.list_definitions(&workspace)?;
        Ok::<_, String>(
            listings
                .into_iter()
                .map(|listing| {
                    let refusal = removal_refusal(&workspace, listing.slug());
                    (listing, refusal)
                })
                .collect(),
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("Listing the hooks stopped: {e}")))?
    .map_err(ApiError::internal)?;
    let armed = armed_hooks(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("The hook armings cannot be read: {e}")))?;
    let mut rows = rows(folders, armed, state.event_hooks.asked_event_types());
    if !rows.is_empty() {
        let mut owners = crate::engine::tools::plugins::event_hook_plugin_ids(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("The plugin hooks cannot be read: {e}")))?;
        for row in &mut rows {
            if let Some(plugin_id) = owners.remove(row.slug()) {
                row.remove_refusal = Some(format!("Uninstall {plugin_id} to remove this hook."));
                row.plugin_id = Some(plugin_id);
            }
        }
    }
    Ok(rows)
}

async fn read_row(state: &AppState, slug: &str) -> Result<Option<HookRow>, ApiError> {
    Ok(read_rows(state)
        .await?
        .into_iter()
        .find(|row| row.slug() == slug))
}

async fn list_hooks(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        serde_json::json!({ "hooks": read_rows(&state).await? }),
    ))
}

#[derive(Deserialize)]
struct HookChoiceQuery {
    #[serde(default)]
    query: String,
}

async fn hook_choices(
    State(state): State<AppState>,
    Path((slug, argument)): Path<(String, String)>,
    Query(query): Query<HookChoiceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let choices = state
        .engine
        .event_bus
        .first_message_hook_choices(&slug, &argument, &query.query)
        .await
        .map_err(|error| ApiError::new(StatusCode::CONFLICT, error))?;
    Ok(Json(serde_json::json!({ "choices": choices })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HookActBody {
    /// Why an agent asks. The owner's card quotes it.
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArmBody {
    #[serde(default)]
    reason: String,
    /// The digest the caller was shown. A hook that hashes otherwise now is
    /// refused, so the owner never arms content they did not see.
    #[serde(default)]
    content_hash: Option<String>,
}

async fn arm_hook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(body): Json<ArmBody>,
) -> Result<Response, ApiError> {
    let verb = Verb::Arm {
        seen: body.content_hash.as_deref(),
    };
    act(&state, &headers, &slug, verb, &body.reason).await
}

async fn disarm_hook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(body): Json<HookActBody>,
) -> Result<Response, ApiError> {
    act(&state, &headers, &slug, Verb::Disarm, &body.reason).await
}

async fn remove_hook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(body): Json<HookActBody>,
) -> Result<Response, ApiError> {
    act(&state, &headers, &slug, Verb::Remove, &body.reason).await
}

#[derive(Debug, Clone, Copy)]
enum Verb<'a> {
    /// `seen` is the digest the caller was shown, when it names one.
    Arm {
        seen: Option<&'a str>,
    },
    Disarm,
    Remove,
}

impl Verb<'_> {
    fn action(self) -> Action {
        match self {
            Self::Arm { .. } => Action::ArmEventHook,
            Self::Disarm => Action::DisarmEventHook,
            Self::Remove => Action::RemoveEventHook,
        }
    }
}

/// One verb on one hook, as checked before anyone is asked.
#[derive(PartialEq, Eq)]
enum Plan {
    Arm {
        event_type: String,
        content_hash: String,
    },
    Disarm {
        event_type: String,
    },
    /// `folder` is false when only an arming is left to end.
    Remove {
        folder: bool,
    },
}

impl Plan {
    fn of(
        verb: Verb<'_>,
        slug: &str,
        row: Option<&HookRow>,
        asked: &[&str],
    ) -> Result<Self, ApiError> {
        let conflict = |message: String| ApiError::new(StatusCode::CONFLICT, message);
        let row = row.ok_or_else(|| ApiError::not_found(format!("No hook '{slug}' exists.")))?;
        match verb {
            Verb::Arm { seen } => {
                let (event_type, content_hash) = row
                    .armable(asked)
                    .map_err(|why| conflict(format!("The hook '{slug}' cannot be armed. {why}")))?;
                if seen.is_some_and(|seen| seen != content_hash) {
                    return Err(conflict(format!(
                        "The hook '{slug}' changed since it was shown. Review it and arm it again."
                    )));
                }
                Ok(Self::Arm {
                    event_type: event_type.to_owned(),
                    content_hash: content_hash.to_owned(),
                })
            }
            Verb::Disarm => match &row.armed {
                Some(armed) => Ok(Self::Disarm {
                    event_type: armed.event_type.clone(),
                }),
                None => Err(conflict(format!("The hook '{slug}' is not armed."))),
            },
            Verb::Remove => match &row.remove_refusal {
                Some(why) => Err(conflict(why.clone())),
                None => Ok(Self::Remove {
                    folder: matches!(row.folder, Folder::Listed(_)),
                }),
            },
        }
    }

    /// The permission's breadth and what the act does, for an agent's card.
    fn ask_terms(&self, slug: &str) -> (String, String) {
        match self {
            Self::Arm {
                event_type,
                content_hash,
            } => (
                format!("{slug}@{content_hash}"),
                format!(
                    "From then on, Lucidos asks its script before it records each `{event_type}` \
                     event, and the script can stop that event. The arming binds this exact \
                     content, so an edit asks again."
                ),
            ),
            Self::Disarm { event_type } => (
                slug.to_owned(),
                format!("Lucidos stops asking it, so it no longer stops any `{event_type}` event."),
            ),
            Self::Remove { .. } => (
                slug.to_owned(),
                format!(
                    "Lucidos deletes its folder, `data/{HOOKS_DIR}/{slug}/`, and commits that. \
                     Any arming of it ends."
                ),
            ),
        }
    }
}

/// Who may act: the owner, an agent holding a once permission, or nobody yet.
enum Gate {
    Act(Option<MessageOrigin>),
    NeedsPermission(String),
}

async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    verb: Verb<'_>,
    slug: &str,
    plan: &Plan,
    reason: &str,
) -> Result<Gate, ApiError> {
    match owner_attestation(headers) {
        // An app document reads the shell's device id, so its fetch carries
        // the owner's stamp. An app an agent wrote must not press these.
        OwnerAttestation::Owner if !reveal_request_allowed(headers, RefererRule::WhenPresent) => {
            Err(ApiError::new(StatusCode::FORBIDDEN, NOT_FROM_AN_APP))
        }
        OwnerAttestation::Owner => Ok(Gate::Act(user_actor(headers, None))),
        OwnerAttestation::Agent => {
            let caller = calling_agent_thread(headers)?;
            let (breadth, effect) = plan.ask_terms(slug);
            let ask = hook_ask(verb.action(), breadth, slug, &effect, reason)?;
            match spend_or_prepare(&state.engine.event_bus, caller, ask).await? {
                Spent::Allowed(_) => Ok(Gate::Act(user_actor(headers, None))),
                Spent::Requested(request_id) => Ok(Gate::NeedsPermission(request_id)),
                Spent::DeviceSessionScope => Err(ApiError::internal(
                    "A device session scope cannot cover an event hook verb.",
                )),
            }
        }
        OwnerAttestation::Unattested => Err(crate::api::owner_door::unattested()),
    }
}

async fn act(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
    verb: Verb<'_>,
    reason: &str,
) -> Result<Response, ApiError> {
    checked_slug(slug)?;
    let row = read_row(state, slug).await?;
    let plan = Plan::of(
        verb,
        slug,
        row.as_ref(),
        state.event_hooks.asked_event_types(),
    )?;
    let actor = match gate(state, headers, verb, slug, &plan, reason).await? {
        Gate::Act(actor) => actor,
        Gate::NeedsPermission(request_id) => return Ok(permission_required(&request_id)),
    };
    let held = state.engine.lock_workspace_repo().await;
    let current = read_row(state, slug).await?;
    let current_plan = Plan::of(
        verb,
        slug,
        current.as_ref(),
        state.event_hooks.asked_event_types(),
    )?;
    if current_plan != plan {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The hook changed. Review it and try again.",
        ));
    }
    let slug = slug.to_owned();
    match plan {
        Plan::Arm {
            event_type,
            content_hash,
        } => {
            let armed = SystemEvent::EventHookArmed {
                slug: slug.clone(),
                event_type,
                content_hash,
                actor,
            };
            record(state, &slug, armed).await?;
        }
        Plan::Disarm { .. } => {
            let disarmed = SystemEvent::EventHookDisarmed {
                slug: slug.clone(),
                actor,
            };
            record(state, &slug, disarmed).await?;
        }
        Plan::Remove { folder } => {
            let removed = SystemEvent::EventHookRemoved {
                slug: slug.clone(),
                actor,
            };
            if folder {
                delete_folder(state, &held, &slug, removed).await?;
            } else {
                record(state, &slug, removed).await?;
            }
            return Ok(Json(serde_json::json!({ "removed": slug })).into_response());
        }
    }
    let row = read_row(state, &slug).await?;
    Ok(Json(row).into_response())
}

async fn record(state: &AppState, slug: &str, event: SystemEvent) -> Result<(), ApiError> {
    state
        .engine
        .event_bus
        .emit(BusEvent::System(event))
        .await
        .map(drop)
        .map_err(|e| ApiError::internal(format!("The act on hook '{slug}' was not recorded: {e}")))
}

/// What an app document is told at these routes.
const NOT_FROM_AN_APP: &str =
    "Arming, disarming and removing an event hook is the owner's act, in Settings > Event \
     Hooks. An app cannot do it, because an agent may have written the app.";

/// Why `slug` is not one plain path segment that is safe inside a card.
fn slug_refusal(slug: &str) -> Option<String> {
    let plain = !slug.is_empty()
        && !slug.starts_with('.')
        && !slug.contains(['/', '\\', '`'])
        && !slug.chars().any(char::is_control);
    (!plain).then(|| format!("'{slug}' does not name a folder under data/{HOOKS_DIR}/."))
}

fn checked_slug(slug: &str) -> Result<(), ApiError> {
    slug_refusal(slug).map_or(Ok(()), |why| Err(ApiError::bad_request(why)))
}

/// Why `data/hooks/<slug>` cannot be removed here, or `None` when it can.
/// `data/hooks` must stay inside `data/`. The entry is removed in place, a
/// link included, so where a link points does not matter.
fn removal_refusal(workspace: &std::path::Path, slug: &str) -> Option<String> {
    let outside = " Rename or delete it outside Lucidos.";
    if let Some(why) = slug_refusal(slug) {
        return Some(format!("{why}{outside}"));
    }
    let data_dir = workspace.join(crate::core::DATA_DIR);
    let Some(root) = crate::core::contained_path::join_data_contained(&data_dir, HOOKS_DIR) else {
        return Some(format!(
            "data/{HOOKS_DIR} links outside data/, so Lucidos leaves it alone."
        ));
    };
    std::fs::symlink_metadata(root.join(slug))
        .err()
        .map(|e| format!("data/{HOOKS_DIR}/{slug} cannot be opened ({e}).{outside}"))
}

/// Delete `data/hooks/<slug>/`, commit that and emit `removed`, under the
/// workspace repo lock.
async fn delete_folder(
    state: &AppState,
    held: &crate::engine::git_ops::WorkspaceRepoLock,
    slug: &str,
    removed: SystemEvent,
) -> Result<(), ApiError> {
    let rel = format!("{HOOKS_DIR}/{slug}");
    let manager = crate::core::artifacts::ArtifactManager::new(state.workspace_path.clone())
        .map_err(|e| ApiError::internal(format!("The workspace repository cannot open: {e}")))?;
    if let Some(why) = removal_refusal(&state.workspace_path, slug) {
        return Err(ApiError::new(StatusCode::CONFLICT, why));
    }
    manager
        .delete_data_dir_and_commit(
            held,
            &state.engine.event_bus,
            &rel,
            &format!("Remove event hook {slug}"),
            removed,
        )
        .await
        .map_err(|e| ApiError::internal(format!("Removing data/{rel} failed: {e}")))?;
    Ok(())
}

/// The hook feature for a test router: a workspace with no hooks.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct NoEventHooks;

#[cfg(any(test, feature = "test-support"))]
impl crate::core::event_hooks::EventHooks for NoEventHooks {
    fn list_definitions(&self, _workspace: &std::path::Path) -> Result<Vec<HookListing>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
