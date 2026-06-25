//! Best-effort boot-phase reporting to the workspace gateway (ADR 0014 §11).
//!
//! A cold engine boot — especially the first ever, where the embedding model is
//! downloaded (hundreds of MB) before HTTP binds — keeps the gateway's boot
//! splash up for a long time. The gateway can't see *inside* our startup (our
//! HTTP server isn't up yet), so we tell it which phase we're in by POSTing the
//! gateway's boot-phase control endpoint. The gateway renders the matching label
//! on the splash (see `lucidos-gateway/src/boot_phase.rs`).
//!
//! This is pure telemetry: fire-and-forget, short timeout, all errors swallowed,
//! and a **no-op when not spawned by the gateway** (`LUCIDOS_GATEWAY_PORT` /
//! `LUCIDOS_WORKSPACE_ID` unset — the `LUCIDOS_NO_GATEWAY` dev mode and the e2e
//! direct-engine harness). It must never affect startup correctness or timing,
//! so it never blocks the caller: the POST runs on a detached task.

use std::time::Duration;

/// Kebab-case phase wire values understood by the gateway
/// (`BootPhase::from_wire`). Engine-reported phases only — the gateway sets the
/// `provisioning-database` / `starting-engine` phases itself.
pub const MIGRATING: &str = "migrating";
pub const BUILDING_SEARCH_INDEX: &str = "building-search-index";
pub const RECOVERING: &str = "recovering";

/// Report the current cold-boot `phase` to the gateway, if we were spawned by
/// one. Returns immediately; the POST runs detached so startup never waits on
/// it. No-op outside a tokio runtime or when the gateway env vars are unset.
pub fn report(phase: &str) {
    let (Ok(port), Ok(id)) = (
        std::env::var("LUCIDOS_GATEWAY_PORT"),
        std::env::var("LUCIDOS_WORKSPACE_ID"),
    ) else {
        return; // not gateway-spawned (LUCIDOS_NO_GATEWAY / e2e) — nothing to report
    };
    // Detached so a slow/unreachable gateway can never stall the boot. We're
    // inside the async startup, so a runtime exists; guard anyway so a stray
    // call off-runtime is a no-op rather than a panic.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let phase = phase.to_string();
    handle.spawn(async move {
        let url =
            format!("http://127.0.0.1:{port}/~/api/v1/control/workspaces/{id}/boot-phase");
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        // Best-effort: the gateway's own health probe is the source of truth for
        // readiness; a dropped phase report only costs a slightly less specific
        // splash label, and the next phase (or the healthy probe) supersedes it.
        let _ = client
            .post(&url)
            .json(&serde_json::json!({ "phase": phase }))
            .send()
            .await;
    });
}
