//! *Successor threads*: a coding-agent thread's context size, and the handoff
//! to a new thread that carries on with a fresh context.
//!
//! - `GET /api/v1/threads/:thread_id/context`: the size and what it calls for.
//! - `POST /api/v1/threads/:thread_id/successor`: start the successor.
//!
//! The successor sits in the old thread's place (ADR 0168): top-level when the
//! old thread was, else a child of the same parent with the same wake mode.
//! Its branch starts at the old thread's last commit
//! (`engine::successor::carried_base`).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::thread_reach::{refuse_without_authority, ThreadReachError, ThreadReachVerb};
use crate::api::AppState;
use crate::core::store::thread_link;
use crate::engine::successor::{missing_narrative_sections, record_handoff, ThreadContext};
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};

fn parse_thread_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))
}

pub(in crate::api) async fn thread_context(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<ThreadContext>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    if !exists {
        return Err(ApiError::not_found(format!(
            "Thread {thread_id} does not exist"
        )));
    }
    crate::engine::successor::thread_context(&state.pool, thread_id)
        .await
        .map(Json)
        .map_err(ApiError::db)
}

/// Exactly one of the two. The agent's handoff carries a *handoff narrative*;
/// the composer's carries the user's next message and no narrative.
#[derive(Debug, Deserialize)]
pub(in crate::api) struct SuccessorRequest {
    #[serde(default)]
    narrative: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct SuccessorResponse {
    successor_thread_id: Uuid,
    /// A markdown link to the successor, as `lucidos spawn-thread` prints one.
    link: String,
}

/// The old thread, as the handoff reads it.
#[derive(sqlx::FromRow)]
struct OldThread {
    is_coding_agent: bool,
    coding_agent: Option<String>,
    cc_repo_id: Option<String>,
    parent_thread_id: Option<Uuid>,
    wake_mode: Option<String>,
    spawning_event_id: Option<Uuid>,
    archive_state: String,
    successor_thread_id: Option<Uuid>,
    coding_agent_integrated: bool,
    apply_in_flight: bool,
    status: String,
    title: Option<String>,
}

fn refusal(status: StatusCode, reason: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, message).with_reason(reason)
}

pub(in crate::api) async fn spawn_successor(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SuccessorRequest>,
) -> Result<Json<SuccessorResponse>, ApiError> {
    let old_id = parse_thread_id(&thread_id)?;
    let engine = &state.engine;
    let pool = &state.pool;
    let workspace = engine.workspace_name();
    let link = |title: &str, id: Uuid| format!("[{title}]({})", thread_link(&workspace, id));

    let opening = match (request.narrative, request.message) {
        (Some(narrative), None) => {
            let missing = missing_narrative_sections(&narrative);
            if !missing.is_empty() {
                return Err(refusal(
                    StatusCode::BAD_REQUEST,
                    "narrative-incomplete",
                    format!(
                        "The handoff narrative lacks these sections: {}. Add a heading for each, then try again.",
                        missing.join(", ")
                    ),
                ));
            }
            Opening::Narrative(narrative)
        }
        (None, Some(message)) if !message.trim().is_empty() => Opening::Message(message),
        _ => {
            return Err(ApiError::bad_request(
                "Send exactly one of `narrative` (an agent's handoff) or `message` (the user's next message).",
            ))
        }
    };

    let old: OldThread = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, cc_repo_id, parent_thread_id, wake_mode, \
                spawning_event_id, archive_state, successor_thread_id, coding_agent_integrated, \
                apply_in_flight, status, \
                COALESCE(title, LEFT(first_message, 80)) AS title \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .ok_or_else(|| ApiError::not_found(format!("Thread {old_id} does not exist")))?;
    let old_title = old
        .title
        .clone()
        .unwrap_or_else(|| "the earlier thread".to_string());
    if !old.is_coding_agent {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "not-coding-agent",
            format!("\"{old_title}\" is not a coding-agent thread, so it has no successor."),
        ));
    }
    if old.coding_agent_integrated {
        return Err(refusal(
            StatusCode::CONFLICT,
            "integrated",
            format!("\"{old_title}\" is integrated into its parent, whose agent merges its branch. It hands off to no successor."),
        ));
    }
    if let Some(successor) = old.successor_thread_id {
        return Err(refusal(
            StatusCode::CONFLICT,
            "already-handed-off",
            format!(
                "\"{old_title}\" already continued in {}.",
                link("its successor", successor)
            ),
        ));
    }
    if old.archive_state == "archived" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "archived",
            format!("\"{old_title}\" is archived. Unarchive it first."),
        ));
    }
    // The composer offers its handoff only at an idle, and a stale tap must
    // not hand off a thread whose turn has started since. The agent's own
    // handoff runs inside its turn by design.
    if matches!(opening, Opening::Message(_)) && old.status != "idle" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "thread-busy",
            format!("\"{old_title}\" is working. Send to a new thread once it is idle."),
        ));
    }

    // A thread hands off only itself: an agent cannot hand off another
    // thread's work.
    let from_agent = match crate::api::actor::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => {
            if source_thread_id != Some(old_id) {
                return Err(refusal(
                    StatusCode::FORBIDDEN,
                    "not-your-thread",
                    "A thread can hand off only itself. Run `lucidos successor spawn` in the thread that continues.",
                ));
            }
            true
        }
        SubprocessOrigin::NotSubprocess => false,
    };

    if old.apply_in_flight {
        return Err(refusal(
            StatusCode::CONFLICT,
            "applying",
            format!("\"{old_title}\" is applying its change. Hand off once the apply ends."),
        ));
    }
    if crate::engine::successor::awaits_landing(pool, old_id)
        .await
        .map_err(ApiError::db)?
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "awaiting-landing",
            format!(
                "\"{old_title}\" has a change waiting in the landing lane. Hand off once it lands."
            ),
        ));
    }
    let base_sha = carried_head(&state, old_id, &old_title).await?;

    // Last before any write: an owner approval this spends must not be
    // wasted on a refusal above.
    let placement = crate::engine::successor::Placement::of_old_thread(
        old.parent_thread_id,
        old.wake_mode.as_deref(),
        old.spawning_event_id,
    );
    // A top-level successor is a top-thread, which needs no authority (ADR
    // 0475). A child one is an act in its parent's subtree.
    if let Some(parent) = placement.parent_thread_id {
        refuse_without_authority(
            &engine.event_bus,
            &headers,
            Some(parent),
            ThreadReachVerb::SpawnSuccessor,
        )
        .await
        .map_err(|e| authority_refusal(e, parent))?;
    }

    let actor = crate::api::actor::user_actor(&headers, None);
    let (mode, origin) = if from_agent {
        (
            ActorMode::Agent,
            Some(MessageOrigin::ThreadLink {
                thread_id: old_id,
                title: Some(old_title.clone()),
                spawning_event_id: None,
                mode: ActorMode::Agent,
                direction: ThreadDirection::Parent,
            }),
        )
    } else {
        // The first message speaks as the credential: a device is the user,
        // the local token is the machine. A mismatch would fail the spawn
        // after the handoff is recorded.
        let origin = actor.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "A handoff needs a credential that names who asks.",
            )
        })?;
        (origin.mode(), Some(origin))
    };
    let device_id = match &actor {
        Some(MessageOrigin::Device { device_id }) => Some(device_id.clone()),
        _ => None,
    };

    let old_link = link(&old_title, old_id);
    let message = match opening {
        Opening::Narrative(narrative) => format!(
            "This thread continues {old_link}. Its worktree starts at that thread's last \
             commit, so it already holds all of its work. Read the handoff narrative below, \
             then carry on from its next step.\n\n{narrative}"
        ),
        Opening::Message(message) => {
            let applied = last_applied_summary(pool, old_id).await;
            let applied = applied
                .map(|s| format!(" The change it applied last: {s}"))
                .unwrap_or_default();
            format!(
                "{message}\n\n---\nThis thread continues {old_link}. Its worktree starts at \
                 that thread's last commit.{applied}"
            )
        }
    };

    // Every fallible read comes before the handoff is recorded, so a failure
    // here leaves the old thread as it was.
    let app_id: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'app_id' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'coding_agent_kind' = 'app' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .flatten();
    let (cc_model, reasoning_effort) = engine.cc_thread_settings(old_id).await;
    let successor_id = Uuid::new_v4();
    let successor_title = format!("{old_title} (continued)");
    let request = crate::engine::thread_queue::ThreadQueueRequest::AgentChat {
        depth: 0,
        message,
        thread_id: successor_id,
        event_id: Some(Uuid::new_v4().to_string()),
        image_hashes: Vec::new(),
        device_id,
        model: None,
        reasoning_effort,
        provider: None,
        use_coding_agent: Some(true),
        repo_id: if app_id.is_some() {
            None
        } else {
            old.cc_repo_id
        },
        cc_model,
        coding_agent: old
            .coding_agent
            .as_deref()
            .map(crate::runtime::CodingAgent::parse),
        title: Some(successor_title.clone()),
        mode,
        origin: origin.clone(),
        parent_thread_id: placement.parent_thread_id,
        spawning_event_id: placement.spawning_event_id,
        wake_mode: placement.wake_mode,
        app_id,
        integrated: false,
    };

    // One handoff per thread, across engines: the lock serializes two
    // requests, and the second re-reads the link the first recorded.
    let mut claim = pool.begin().await.map_err(ApiError::db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("successor-handoff:{old_id}"))
        .execute(&mut *claim)
        .await
        .map_err(ApiError::db)?;
    if let Some(successor) = crate::engine::successor::successor_thread_of(pool, old_id)
        .await
        .map_err(ApiError::db)?
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "already-handed-off",
            format!(
                "\"{old_title}\" already continued in {}.",
                link("its successor", successor)
            ),
        ));
    }
    record_handoff(
        &engine.event_bus,
        pool,
        old_id,
        successor_id,
        &base_sha,
        actor.clone(),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not record the handoff: {e}")))?;
    claim.commit().await.map_err(ApiError::db)?;
    engine.thread_queue.submit(request, origin, None).await;

    Ok(Json(SuccessorResponse {
        successor_thread_id: successor_id,
        link: link(&successor_title, successor_id),
    }))
}

/// A refused handoff names the approval that lets it through.
fn authority_refusal(e: ThreadReachError, parent: Uuid) -> ApiError {
    let approval = format!(
        "`lucidos ask-owner-approval spawn-successor --thread {parent} --reason \"<why>\"`"
    );
    match e {
        ThreadReachError::NoStandingInstruction(_) | ThreadReachError::OutOfReach { .. } => {
            refusal(
                StatusCode::FORBIDDEN,
                "needs-owner-approval",
                format!(
                    "A successor sits in this thread's place, which is the workspace owner's \
                     call in a turn they did not open. Run {approval}, ask with the id it \
                     prints, and retry once the owner allows it."
                ),
            )
        }
        other => other.into(),
    }
}

enum Opening {
    Narrative(String),
    Message(String),
}

/// The old thread's last commit, which the successor's branch starts at.
/// Refuses uncommitted work rather than leave it behind, and refuses when
/// git cannot answer rather than guess.
async fn carried_head(state: &AppState, old_id: Uuid, old_title: &str) -> Result<String, ApiError> {
    let live = {
        let sessions = state.engine.agent_sessions.lock().await;
        crate::api::repositories::live_thread_worktree(sessions.get(&old_id))
    };
    let worktree = crate::api::repositories::resolve_diff_worktree(
        &state.pool,
        old_id,
        live,
        state.engine.workspace_path(),
    )
    .await
    .ok_or_else(|| {
        refusal(
            StatusCode::CONFLICT,
            "no-worktree",
            format!(
                "\"{old_title}\" has no worktree left on disk, so there is no branch to carry."
            ),
        )
    })?;
    let unknown = || {
        refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "git-unanswered",
            format!("Could not read the state of \"{old_title}\"'s worktree. Nothing changed; try again."),
        )
    };
    use crate::engine::git_ops::GitAnswer;
    match crate::engine::git_ops::worktree_dirtiness(&worktree).await {
        GitAnswer::No => {}
        GitAnswer::Yes => {
            return Err(refusal(
                StatusCode::CONFLICT,
                "uncommitted-work",
                format!("\"{old_title}\" has uncommitted changes. Commit them first, so the successor carries them."),
            ))
        }
        GitAnswer::Unknown => return Err(unknown()),
    }
    crate::engine::git_ops::current_head_sha(&worktree)
        .await
        .ok_or_else(unknown)
}

/// What the old thread's last applied change did, for the composer's
/// successor. Best effort: a successor without it is still whole.
async fn last_applied_summary(pool: &sqlx::PgPool, old_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(summary, description) FROM changes \
         WHERE thread_id = $1 AND status = 'applied' \
         ORDER BY resolved_at DESC NULLS LAST LIMIT 1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Successor] last applied change of {}: {}", old_id, e);
        None
    })
}
