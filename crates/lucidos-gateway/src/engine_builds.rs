//! Which engine build each workspace runs, and "switch all running" (ADR 0824).
//!
//! Every workspace a gateway starts runs the one engine binary at
//! `engine_bin`, so a stopped or new workspace always starts on the newest
//! build. A running engine keeps the build it started on until somebody
//! switches it.
//!
//! INVARIANT: nothing here starts a stopped workspace. A switch goes through
//! [`GatewayState::restart_running`], which skips a workspace with no stack.

use crate::engine_hop::{self, Credential, EngineAddress};
use crate::server::GatewayState;
use axum::body::Body;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct EngineBuilds {
    /// The build a stopped or new workspace starts on. `None` when the binary
    /// cannot say, for example mid-rewrite.
    pub newest: Option<String>,
    pub workspaces: Vec<WorkspaceBuild>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceBuild {
    pub id: String,
    pub name: String,
    pub running: bool,
    /// The build the running engine reports. `None` for a stopped workspace,
    /// for an engine too old to report one, and for one not answering.
    pub build_id: Option<String>,
    /// Running, but its health did not answer, so its build is unknown.
    pub unanswered: bool,
    /// Running, and the newest build is ahead of it.
    pub behind: bool,
}

/// What a running engine's health said about its build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunningBuild {
    Reported(String),
    /// Answered with no `build_id`: an engine from before ADR 0824.
    Unreported,
    /// Did not answer in time, refused, or was shutting down.
    Unanswered,
}

impl EngineBuilds {
    /// The workspaces "switch all running" restarts: running and behind.
    pub fn switch_targets(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .filter(|w| w.running && w.behind)
            .map(|w| w.id.clone())
            .collect()
    }
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SwitchOutcome {
    pub switched: Vec<String>,
    /// Stopped, already switched or not answering by its turn, so left alone.
    pub skipped: Vec<String>,
    pub failed: Vec<SwitchFailure>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SwitchFailure {
    pub id: String,
    pub error: String,
}

/// Serializes "switch all running", so two overlapping requests never restart
/// one workspace twice.
static SWITCH_ALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The build list, read from the binary on disk and each running engine.
pub async fn list(state: &GatewayState) -> EngineBuilds {
    let newest = binary_build_id(state.engine_bin()).await;
    let engines = state.running_engine_addresses().await;
    let probes = engines.into_iter().map(|(id, engine)| async move {
        let build = running_build(&engine).await;
        (id, build)
    });
    let running: HashMap<String, RunningBuild> = futures::future::join_all(probes)
        .await
        .into_iter()
        .collect();
    let registered = state
        .registered_workspaces()
        .into_iter()
        .map(|ws| (ws.id, ws.name))
        .collect();
    assemble(state.engine_bin(), registered, &running, newest).await
}

/// Rows in registry order. A workspace is running when `running` holds it.
pub(crate) async fn assemble(
    engine_bin: &Path,
    registered: Vec<(String, String)>,
    running: &HashMap<String, RunningBuild>,
    newest: Option<String>,
) -> EngineBuilds {
    let rows = registered.into_iter().map(|(id, name)| {
        let build = running.get(&id).cloned();
        let newest = newest.as_deref();
        async move {
            let behind = match (newest, &build) {
                (Some(newest), Some(build)) => is_behind(engine_bin, newest, build).await,
                _ => false,
            };
            WorkspaceBuild {
                id,
                name,
                running: build.is_some(),
                build_id: match &build {
                    Some(RunningBuild::Reported(id)) => Some(id.clone()),
                    _ => None,
                },
                unanswered: build == Some(RunningBuild::Unanswered),
                behind,
            }
        }
    });
    let workspaces = futures::future::join_all(rows).await;
    EngineBuilds { newest, workspaces }
}

/// An engine that did not answer is never behind: its build is unknown, and a
/// restart could hit an engine already switching or busy.
async fn is_behind(engine_bin: &Path, newest: &str, build: &RunningBuild) -> bool {
    match build {
        RunningBuild::Reported(id) => {
            crate::build_id::disk_id_is_upgrade(engine_bin, newest, id).await
        }
        RunningBuild::Unreported => {
            crate::build_id::disk_id_is_upgrade(engine_bin, newest, "").await
        }
        RunningBuild::Unanswered => false,
    }
}

/// Restart every running workspace that is behind, as a Switch from
/// `requested_by` would. Stopped workspaces stay stopped. The listing runs
/// under [`SWITCH_ALL`], so a second request sees the first one's restarts.
/// Each target is probed again under its operation lock, so a workspace its
/// own Switch already moved is left alone.
pub async fn switch_all_running(state: &GatewayState, requested_by: Option<&str>) -> SwitchOutcome {
    let _one_at_a_time = SWITCH_ALL.lock().await;
    let builds = list(state).await;
    let Some(newest) = builds.newest.clone() else {
        return SwitchOutcome::default();
    };
    let targets = builds.switch_targets();
    let ids: Vec<&str> = targets.iter().map(String::as_str).collect();
    let still_behind = |id: String| {
        let newest = newest.clone();
        async move {
            let Some(engine) = state.running_engine_addresses().await.remove(&id) else {
                return false;
            };
            is_behind(state.engine_bin(), &newest, &running_build(&engine).await).await
        }
    };
    tally(
        state
            .restart_running(&ids, requested_by, still_behind)
            .await,
    )
}

pub(crate) fn tally(outcomes: Vec<(String, Result<bool, crate::BoxError>)>) -> SwitchOutcome {
    let mut outcome = SwitchOutcome::default();
    for (id, result) in outcomes {
        match result {
            Ok(true) => outcome.switched.push(id),
            Ok(false) => outcome.skipped.push(id),
            Err(e) => {
                crate::log_legacy!("[Gateway] switching '{id}' to the newest build failed: {e}");
                outcome.failed.push(SwitchFailure {
                    id,
                    error: e.to_string(),
                });
            }
        }
    }
    crate::log_legacy!(
        "[Gateway] switch all running: {} switched, {} skipped, {} failed",
        outcome.switched.len(),
        outcome.skipped.len(),
        outcome.failed.len()
    );
    outcome
}

async fn binary_build_id(engine_bin: &Path) -> Option<String> {
    let out = tokio::process::Command::new(engine_bin)
        .arg("--build-id")
        .output()
        .await
        .ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !id.is_empty()).then_some(id)
}

/// What a running engine's `/api/v1/health` says about its build.
async fn running_build(engine: &EngineAddress) -> RunningBuild {
    let probe = async {
        let request = axum::extract::Request::builder()
            .uri("/api/v1/health")
            .body(Body::empty())
            .ok()?;
        let response = engine_hop::send(engine, request, Credential::None)
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = axum::body::to_bytes(
            Body::new(response.into_body()),
            crate::remote_browser::MAX_CLIENT_HEALTH_BYTES,
        )
        .await
        .ok()?;
        let health: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(match health["build_id"].as_str() {
            Some(id) => RunningBuild::Reported(id.to_string()),
            None => RunningBuild::Unreported,
        })
    };
    tokio::time::timeout(HEALTH_TIMEOUT, probe)
        .await
        .ok()
        .flatten()
        .unwrap_or(RunningBuild::Unanswered)
}

#[cfg(test)]
#[path = "engine_builds_tests.rs"]
mod tests;
