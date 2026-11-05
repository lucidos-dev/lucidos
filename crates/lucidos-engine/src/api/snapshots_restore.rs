//! Admission runs on the target. Body workspace names are routing hints only.
use super::*;
use snapshot::{
    journal::Journal, service::OfflineRestoreRequest, source_authority::VerifiedSource,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Restore {
    expected_workspace_id: String,
    #[serde(default)]
    confirmed: bool,
    source_workspace: Option<String>,
    reason: Option<String>,
}

fn check_target(request: &Restore, target: &OfflineRestoreRequest) -> Result<(), ApiError> {
    if request.expected_workspace_id != target.gateway_slug {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "snapshot_restore_binding_mismatch",
        ));
    }
    Ok(())
}

pub(super) async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<Handle>,
    Json(request): Json<Restore>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Own admission across request cancellation. Never resubmit a lost gateway
    // response: it may already have stopped this engine.
    tokio::spawn(async move {
        let slug = super::super::base_path::workspace_id().ok_or_else(|| {
            ApiError::new(StatusCode::FORBIDDEN, "snapshot_restore_gateway_required")
        })?;
        let port: u16 = super::super::base_path::gateway_port()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                ApiError::new(StatusCode::FORBIDDEN, "snapshot_restore_gateway_required")
            })?;
        let workspace = state.workspace_path.clone();
        let database_url = crate::core::database_url();
        let binding_url = database_url.clone();
        let target = worker(move || {
            let app_data =
                lucidos_home::gateway_data_dir().ok_or("snapshot_restore_binding_mismatch")?;
            snapshot::registry_binding::resolve(
                &app_data,
                &workspace,
                &binding_url,
                &slug,
                uuid::Uuid::new_v4(),
            )
        })
        .await?;
        check_target(&request, &target)?;
        // Any origin credential forces source verification, including credentials
        // this target cannot verify. Borrowed device attribution cannot bypass it.
        let source = if headers.contains_key(super::super::actor::HEADER_AGENT_ORIGIN_TOKEN)
            || request.source_workspace.is_some()
        {
            let token = headers
                .get(super::super::actor::HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    ApiError::new(StatusCode::FORBIDDEN, "snapshot_restore_caller_unverified")
                })?;
            let hint = request
                .source_workspace
                .as_deref()
                .unwrap_or(&target.gateway_slug);
            Some(
                VerifiedSource::verify(port, hint, token, &target)
                    .await
                    .map_err(failure)?,
            )
        } else {
            None
        };
        let actor = if let Some(source) = &source {
            crate::engine::thread_events::MessageOrigin::Workspace {
                workspace: source.workspace().into(),
                thread_id: Some(source.thread()),
                event_id: None,
                user_agent: None,
                mode: crate::engine::thread_events::ActorMode::Agent,
            }
        } else {
            let actor = super::super::actor::require_owner_device_for(
                &headers,
                &state.pool,
                None,
                "snapshot_restore_owner_device_required",
            )
            .await?;
            if !request.confirmed {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "snapshot_restore_confirmation_required",
                ));
            }
            actor
        };
        let session = open(&state, ArchiveJobKind::SnapshotRestore).await?;
        let reader = session.clone();
        let snapshot = handle.clone();
        let url = database_url.clone();
        let workspace = state.workspace_path.clone();
        worker(move || reader.preflight_restore(&snapshot, &url, &workspace)).await?;
        if let Some(source) = &source {
            let policy = prefs::SNAPSHOT_RESTORE_BY_AGENTS
                .try_read(&state.pool)
                .await
                .map_err(|_| ApiError::internal("snapshot_preferences_unavailable"))?;
            match policy.as_str() {
                "allow" => {}
                "ask" => source.spend(&target, &handle).await.map_err(failure)?,
                _ => {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "snapshot_restore_policy_unavailable",
                    ))
                }
            }
        }
        let mut journal = Journal::new(
            &target.gateway_slug,
            handle.clone(),
            actor.clone(),
            request.reason,
        );
        journal.restore_id = target.restore_id;
        let writer = session.clone();
        worker(move || writer.admit_restore(&journal)).await?;
        let requested = state
            .engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::SnapshotRestoreRequested {
                    workspace_id: target.gateway_slug.clone(),
                    handle,
                    restore_id: target.restore_id,
                    actor,
                },
            ))
            .await;
        let delivered = match requested {
            Ok(_) => {
                deliver_restore(
                    port,
                    &target.gateway_slug,
                    target.restore_id,
                    crate::net_config::peer_scheme_order(),
                )
                .await
            }
            Err(_) => Err(Undelivered::Refused),
        };
        match delivered {
            Ok(()) => {}
            // The gateway never took the restore, so the journal must not keep
            // the workspace in maintenance behind a running engine.
            Err(Undelivered::Refused) => {
                let refuser = session.clone();
                let id = target.restore_id;
                worker(move || refuser.refuse_restore(id)).await?;
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "snapshot_restore_not_delivered",
                ));
            }
            Err(Undelivered::Unknown) => {
                return Err(ApiError::internal("snapshot_restore_delivery_unknown"))
            }
        }
        drop(session);
        Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"restore_id":target.restore_id})),
        ))
    })
    .await
    .map_err(|_| ApiError::internal("snapshot_worker_failed"))?
}

/// Why an admitted restore did not reach the gateway.
#[derive(Debug, PartialEq, Eq)]
enum Undelivered {
    /// The gateway provably did not start it: nothing was sent, or it answered.
    Refused,
    /// The POST left and no answer came back. The restore may be running.
    Unknown,
}

/// Probe, then POST the restore once. The gateway answers 202 only after it
/// has taken the restore, and any other answer means it took nothing.
async fn deliver_restore(
    port: u16,
    workspace: &str,
    restore_id: uuid::Uuid,
    schemes: [&'static str; 2],
) -> Result<(), Undelivered> {
    let client = crate::gateway_auth::client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(6))
        .build()
        .map_err(|_| Undelivered::Refused)?;
    let scheme = probe_restore_gateway(&client, port, workspace, schemes)
        .await
        .map_err(|_| Undelivered::Refused)?;
    let response = client
        .post(format!(
            "{scheme}://127.0.0.1:{port}/~/api/v1/control/workspaces/{workspace}/snapshot-restore"
        ))
        .json(&serde_json::json!({ "restore_id": restore_id }))
        .send()
        .await
        .map_err(|_| Undelivered::Unknown)?;
    if response.status() == reqwest::StatusCode::ACCEPTED {
        Ok(())
    } else {
        Err(Undelivered::Refused)
    }
}

/// An HTTP refusal is authoritative. Only a failed transport permits trying
/// the other scheme before the single destructive delivery.
async fn probe_restore_gateway(
    client: &reqwest::Client,
    port: u16,
    workspace: &str,
    schemes: [&'static str; 2],
) -> Result<&'static str, ApiError> {
    for scheme in schemes {
        match client.get(format!("{scheme}://127.0.0.1:{port}/~/api/v1/control/workspaces/{workspace}/snapshot-restore-status"))
            .send().await {
            Ok(response) if response.status().is_success() => return Ok(scheme),
            Ok(_) => break,
            Err(_) => continue,
        }
    }
    Err(ApiError::internal("snapshot_restore_delivery_unknown"))
}

/// Read only external state; never expose PreparedFiles or its host paths.
pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::super::actor::require_user_actor(&headers, &state.pool, None).await?;
    worker(move || {
        let database = crate::core::secret_store::boot::database_name(&crate::core::database_url())
            .map_err(|_| "snapshot_restore_unavailable")?;
        let identity = snapshot::identity::workspace_id(
            super::super::base_path::workspace_id().as_deref(),
            &database,
        )
        .map_err(|_| "snapshot_restore_unavailable")?;
        let store = snapshot::store::Store::existing_for_database(&database, identity)
            .map_err(|_| "snapshot_restore_unavailable")?;
        let journal = store
            .journals()?
            .into_iter()
            .find(|journal| journal.restore_id == id)
            .ok_or("snapshot_restore_unavailable")?;
        Ok(serde_json::json!({
            "restore_id":journal.restore_id, "workspace_id":journal.workspace_id,
            "handle":journal.snapshot, "recovery":journal.recovery, "phase":journal.phase(),
            "actor":journal.actor, "updated_at":journal.updated_at,
        }))
    })
    .await
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delivery_probe_never_bypasses_http_refusals() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        for status in [301, 401, 403, 404, 500, 502, 503, 504] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let router = Router::new().fallback(move || {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    StatusCode::from_u16(status).unwrap()
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .unwrap();
            // Repeating the reachable scheme makes an erroneous fallback
            // observable as a second HTTP request without a TLS test server.
            assert!(probe_restore_gateway(
                &client,
                port,
                "target",
                [crate::net_config::SCHEME_HTTP; 2]
            )
            .await
            .is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }

    /// Only a POST that left without an answer stays ambiguous. Every other
    /// failure proves the gateway took nothing, so the caller can settle it.
    #[tokio::test]
    async fn delivery_is_refused_unless_the_post_left_unanswered() {
        use axum::routing::{get, post};
        let serve = |router: Router| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            (port, server)
        };
        let status = "/~/api/v1/control/workspaces/target/snapshot-restore-status";
        let restore = "/~/api/v1/control/workspaces/target/snapshot-restore";
        let http = [crate::net_config::SCHEME_HTTP; 2];
        for (answer, expected) in [
            (StatusCode::ACCEPTED, Ok(())),
            (StatusCode::CONFLICT, Err(Undelivered::Refused)),
            (StatusCode::NOT_FOUND, Err(Undelivered::Refused)),
        ] {
            let router = Router::new()
                .route(status, get(|| async { StatusCode::OK }))
                .route(restore, post(move || async move { answer }));
            let (port, server) = serve(router).await;
            let id = uuid::Uuid::new_v4();
            assert_eq!(deliver_restore(port, "target", id, http).await, expected);
            server.abort();
        }

        let (port, server) = serve(
            Router::new()
                .route(status, get(|| async { StatusCode::OK }))
                .route(
                    restore,
                    post(|| async {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        StatusCode::ACCEPTED
                    }),
                ),
        )
        .await;
        let pending = tokio::spawn(deliver_restore(port, "target", uuid::Uuid::new_v4(), http));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        server.abort();
        assert_eq!(pending.await.unwrap(), Err(Undelivered::Unknown));

        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(
            deliver_restore(port, "target", uuid::Uuid::new_v4(), http).await,
            Err(Undelivered::Refused)
        );
    }

    #[test]
    fn restore_body_cannot_supply_authority_or_paths() {
        for field in [
            "actor",
            "origin_token",
            "database_url",
            "workspace_path",
            "restore_id",
            "approval",
        ] {
            let mut body = serde_json::json!({"expected_workspace_id":"target"});
            body[field] = serde_json::json!("forged");
            assert!(serde_json::from_value::<Restore>(body).is_err());
        }
        let request: Restore =
            serde_json::from_value(serde_json::json!({"expected_workspace_id":"other"})).unwrap();
        assert!(!request.confirmed);
        let target = OfflineRestoreRequest {
            restore_id: uuid::Uuid::new_v4(),
            gateway_slug: "target".into(),
            workspace_name: "Target".into(),
        };
        assert!(check_target(&request, &target).is_err());
    }
}
