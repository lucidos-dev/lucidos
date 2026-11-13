//! Identified callers share the snapshot service. Owned workers bridge request
//! cancellation across job publication/scheduling and deletion/announcement.
use super::*;
use crate::core::{
    prefs,
    snapshot::{
        self,
        handle::Handle,
        jobs,
        service::{CaptureRequest, Session},
        ArchiveJobKind,
    },
};
use std::sync::Arc;

#[path = "snapshots_restore.rs"]
mod restore;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Create {
    label: Option<String>,
    #[serde(default)]
    keep: bool,
}

fn failure(error: snapshot::BoxError) -> ApiError {
    // Service errors are fixed slugs. Never propagate a primitive error here.
    let slug = error.to_string();
    match slug.as_str() {
        "archive_job_busy" => {
            ApiError::new(StatusCode::CONFLICT, "archive_job_busy").with_reason("archive_job_busy")
        }
        "snapshot_key_unavailable" | "snapshot_workspace_identity_unavailable" => {
            ApiError::new(StatusCode::FORBIDDEN, slug)
        }
        "snapshot_restore_caller_unverified"
        | "snapshot_restore_own_workspace_refused"
        | "snapshot_restore_target_protected"
        | "snapshot_restore_approval_required" => ApiError::new(StatusCode::FORBIDDEN, slug),
        "snapshot_restore_binding_mismatch"
        | "snapshot_restore_preflight_failed"
        | "snapshot_restore_admission_failed" => ApiError::new(StatusCode::CONFLICT, slug),
        "snapshot_recovery_device_required" => ApiError::new(StatusCode::FORBIDDEN, slug),
        "snapshot_unavailable" | "snapshot_job_unavailable" | "snapshot_restore_unavailable" => {
            ApiError::not_found(slug)
        }
        _ => ApiError::internal("snapshot_operation_failed"),
    }
}

async fn worker<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, snapshot::BoxError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::internal("snapshot_worker_failed"))?
        .map_err(failure)
}

async fn open(state: &AppState, kind: ArchiveJobKind) -> Result<Arc<Session>, ApiError> {
    // Preference failure refuses the operation rather than authorizing pruning
    // using defaults when the configured retention is unknown.
    let manual_retention = prefs::SNAPSHOT_RETENTION
        .try_read(&state.pool)
        .await
        .map_err(|_| ApiError::internal("snapshot_preferences_unavailable"))?
        .round() as usize;
    let recovery_retention = prefs::SNAPSHOT_RECOVERY_RETENTION
        .try_read(&state.pool)
        .await
        .map_err(|_| ApiError::internal("snapshot_preferences_unavailable"))?
        .round() as usize;
    let events = snapshot::events::JobEvents {
        bus: state.engine.event_bus.clone(),
        manual_retention,
        recovery_retention,
    };
    let generation = snapshot::boundary::read(&state.pool)
        .await
        .map_err(|_| ApiError::internal("snapshot_generation_unavailable"))?
        .map(|boundary| boundary.restore_id);
    let slots = state.engine.archive_job_slot.clone();
    let database_url = crate::core::database_url();
    let slug = super::base_path::workspace_id();
    worker(move || {
        Ok(Arc::new(
            Session::open_for_database(&database_url, slug.as_deref(), &slots, kind)?
                .with_events(events)
                .with_restore_generation(generation),
        ))
    })
    .await
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<Create>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    // Own initialization too: a dropped request cannot leave a durable Running
    // job between publication and scheduling.
    let job = tokio::spawn(async move {
        let running: i64 =
            sqlx::query_scalar("SELECT count(*) FROM thread_summaries WHERE status = 'running'")
                .fetch_one(&state.pool)
                .await
                .map_err(|_| ApiError::internal("snapshot_threads_unavailable"))?;
        let session = open(&state, ArchiveJobKind::SnapshotCapture).await?;
        let handle = Handle::generate();
        let writer = session.clone();
        let job = worker(move || writer.create_job(handle, jobs::Action::Capture, actor)).await?;
        let request = CaptureRequest {
            handle: job.handle.clone(),
            actor: job.actor.clone(),
            label: req.label,
            keep: req.keep,
            workspace_name: state
                .workspace_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace")
                .to_owned(),
            running_threads: running as u64,
        };
        let id = job.id;
        tokio::spawn(async move {
            if session
                .run_capture_job(
                    id,
                    &state.pool,
                    &crate::core::database_url(),
                    &state.workspace_path,
                    request,
                )
                .await
                .is_err()
            {
                crate::log!("[Snapshot] snapshot_job_failed");
            }
        });
        Ok::<_, ApiError>(job)
    })
    .await
    .map_err(|_| ApiError::internal("snapshot_worker_failed"))??;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"job_id":job.id,"handle":job.handle})),
    ))
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<snapshot::manifest::Manifest>>, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let session = open(&state, ArchiveJobKind::SnapshotCheck).await?;
    worker(move || session.list()).await.map(Json)
}

async fn get_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<Handle>,
) -> Result<Json<snapshot::manifest::Manifest>, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let session = open(&state, ArchiveJobKind::SnapshotCheck).await?;
    worker(move || session.get(&handle)).await.map(Json)
}

async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<Handle>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let job = tokio::spawn(async move {
        let session = open(&state, ArchiveJobKind::SnapshotCheck).await?;
        let writer = session.clone();
        let job = worker(move || {
            writer.get(&handle)?;
            writer.create_job(handle, jobs::Action::Check, actor)
        })
        .await?;
        let id = job.id;
        tokio::spawn(async move {
            if session
                .run_check_job(id, &crate::core::database_url())
                .await
                .is_err()
            {
                crate::log!("[Snapshot] snapshot_job_failed");
            }
        });
        Ok::<_, ApiError>(job)
    })
    .await
    .map_err(|_| ApiError::internal("snapshot_worker_failed"))??;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"job_id":job.id,"handle":job.handle})),
    ))
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<jobs::Job>, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    worker(move || {
        let database = crate::core::secret_store::boot::database_name(&crate::core::database_url())
            .map_err(|_| "snapshot_job_unavailable")?;
        let identity = snapshot::identity::workspace_id(
            super::base_path::workspace_id().as_deref(),
            &database,
        )
        .map_err(|_| "snapshot_job_unavailable")?;
        let store = snapshot::store::Store::existing_for_database(&database, identity)
            .map_err(|_| "snapshot_job_unavailable")?;
        jobs::read(&store, id)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keep {
    keep: bool,
}

async fn keep(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<Handle>,
    Json(request): Json<Keep>,
) -> Result<Json<snapshot::manifest::Manifest>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let session = open(&state, ArchiveJobKind::SnapshotCheck).await?;
    let manifest = worker(move || session.set_keep(&handle, request.keep)).await?;
    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::SnapshotKeepChanged {
                workspace_id: manifest.workspace_id.clone(),
                handle: manifest.handle.clone(),
                keep: manifest.keep,
                actor: Some(actor),
            },
        ))
        .await
        .map_err(|_| ApiError::internal("snapshot_keep_announcement_failed"))?;
    Ok(Json(manifest))
}

async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<Handle>,
) -> Result<StatusCode, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    tokio::spawn(async move {
        let session = open(&state, ArchiveJobKind::SnapshotCheck).await?;
        session
            .delete_announced(handle, actor)
            .await
            .map_err(failure)?;
        Ok::<_, ApiError>(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| ApiError::internal("snapshot_worker_failed"))?
}

fn verified_restore_thread(headers: &HeaderMap) -> Result<uuid::Uuid, ApiError> {
    match super::actor::subprocess_origin(headers) {
        super::actor::SubprocessOrigin::Subprocess {
            source_thread_id: Some(thread),
            ..
        } => Ok(thread),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "snapshot_restore_caller_unverified",
        )),
    }
}

/// Only the issuing engine can verify its process-local origin MAC, so the
/// origin token alone is the gate. A sandboxed agent cannot read the local
/// token, and the gateway adds it to every proxied request anyway.
fn restore_caller_proof(
    headers: &HeaderMap,
    workspace: impl FnOnce() -> Result<String, ApiError>,
) -> Result<(uuid::Uuid, String), ApiError> {
    let thread = verified_restore_thread(headers)?;
    Ok((thread, workspace()?))
}

/// The target engine must use the gateway's answer, never a body origin hint.
async fn restore_caller(headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread, workspace) = restore_caller_proof(&headers, restore_workspace_identity)?;
    Ok(Json(
        serde_json::json!({"workspace_id":workspace,"source_thread_id":thread}),
    ))
}

/// A gateway slug must name the database this engine actually serves. A bare
/// engine cannot request or authorize a restore, even with inherited hints.
pub(super) fn restore_workspace_identity() -> Result<String, ApiError> {
    let refuse = || ApiError::new(StatusCode::FORBIDDEN, "snapshot_restore_caller_unverified");
    let workspace = super::base_path::workspace_id().ok_or_else(refuse)?;
    let expected =
        lucidos_gateway_constants::workspace_database_name(&workspace).map_err(|_| refuse())?;
    let database = crate::core::secret_store::boot::database_name(&crate::core::database_url())
        .map_err(|_| refuse())?;
    if database != expected {
        return Err(refuse());
    }
    Ok(workspace)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreApprovalTarget {
    target_workspace_id: String,
    snapshot_handle: Handle,
}

/// This POST is source-side authorization, not a restore of this engine.
/// The card is spent only for the exact workspace and handle.
async fn spend_restore_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(target): Json<RestoreApprovalTarget>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread, workspace) = restore_caller_proof(&headers, restore_workspace_identity)?;
    if !super::owner_approval::spend_restore_approval(
        &state.engine.event_bus,
        thread,
        &workspace,
        &target.target_workspace_id,
        &target.snapshot_handle,
    )
    .await
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "snapshot_restore_approval_required",
        ));
    }
    Ok(Json(
        serde_json::json!({"workspace_id":workspace,"source_thread_id":thread}),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/snapshots", get(list).post(create))
        .route("/snapshots/jobs/:id", get(status))
        .route("/snapshots/restores/:id", get(restore::status))
        .route(
            "/snapshots/restore-caller",
            get(restore_caller).post(spend_restore_approval),
        )
        .route(
            "/snapshots/:handle",
            get(get_snapshot).delete(delete).patch(keep),
        )
        .route("/snapshots/:handle/check", post(check))
        .route("/snapshots/:handle/restore", post(restore::restore))
}

#[cfg(test)]
pub(crate) fn assert_restore_slot_busy(database_url: &str, slots: &snapshot::ArchiveJobSlot) {
    let error = Session::open_for_database(
        database_url,
        Some("constructor-workspace"),
        slots,
        ArchiveJobKind::SnapshotRestore,
    )
    .err()
    .expect("capture must exclude restore admission");
    assert_eq!(failure(error).status, StatusCode::CONFLICT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_caller_requires_a_thread_bound_token_from_this_engine() {
        use crate::api::actor::{init_agent_origin_secret, mint_agent_origin_token};
        init_agent_origin_secret("snapshot-caller-test-secret".into());
        let mut headers = HeaderMap::new();
        assert!(verified_restore_thread(&headers).is_err());
        for token in [
            "forged".to_owned(),
            mint_agent_origin_token(None, 0, None).unwrap(),
        ] {
            headers.insert(
                super::super::actor::HEADER_AGENT_ORIGIN_TOKEN,
                token.parse().unwrap(),
            );
            assert!(verified_restore_thread(&headers).is_err());
        }
        let thread = uuid::Uuid::new_v4();
        let token = mint_agent_origin_token(Some(thread), 0, None).unwrap();
        headers.insert(
            super::super::actor::HEADER_AGENT_ORIGIN_TOKEN,
            token.parse().unwrap(),
        );
        assert_eq!(verified_restore_thread(&headers).unwrap(), thread);
        headers.insert(
            super::super::actor::HEADER_AGENT_ORIGIN_TOKEN,
            format!("{token}0").parse().unwrap(),
        );
        assert!(verified_restore_thread(&headers).is_err());
    }

    #[test]
    fn restore_caller_proof_needs_only_an_origin_token_from_this_engine() {
        use crate::api::actor::{init_agent_origin_secret, mint_agent_origin_token};
        init_agent_origin_secret("snapshot-caller-test-secret".into());
        let workspace = || Ok("ws-source".to_owned());
        let refused = |headers: &HeaderMap| {
            let err = restore_caller_proof(headers, workspace).unwrap_err();
            assert_eq!(err.status, StatusCode::FORBIDDEN);
            assert_eq!(err.message, "snapshot_restore_caller_unverified");
        };
        let mut headers = HeaderMap::new();
        refused(&headers);
        headers.insert(
            super::super::actor::HEADER_AGENT_ORIGIN_TOKEN,
            "forged".parse().unwrap(),
        );
        refused(&headers);
        let thread = uuid::Uuid::new_v4();
        headers.insert(
            super::super::actor::HEADER_AGENT_ORIGIN_TOKEN,
            mint_agent_origin_token(Some(thread), 0, None)
                .unwrap()
                .parse()
                .unwrap(),
        );
        assert_eq!(
            restore_caller_proof(&headers, workspace).unwrap(),
            (thread, "ws-source".to_owned())
        );
    }

    #[test]
    fn public_errors_discard_primitive_details_and_distinguish_busy() {
        let err = failure("postgres://private:credential@host/database /private/keys".into());
        assert_eq!(err.message, "snapshot_operation_failed");
        let err = failure("archive_job_busy".into());
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.reason, Some("archive_job_busy"));
        assert_eq!(
            failure("snapshot_recovery_device_required".into()).status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn capture_cannot_accept_actor_identity_or_host_paths_from_the_body() {
        for value in [
            serde_json::json!({"actor":{}}),
            serde_json::json!({"database_url":"unused"}),
            serde_json::json!({"output_path":"unused"}),
        ] {
            assert!(serde_json::from_value::<Create>(value).is_err());
        }
        let request: Create = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!request.keep);
        assert!(request.label.is_none());
    }
}
