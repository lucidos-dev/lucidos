//! Best-effort boot-phase reporting to the workspace gateway (ADR 0014 §11).
//!
//! A cold engine boot — migrations plus the recovery sweeps — keeps the
//! gateway's boot splash up until our HTTP server binds. The gateway can't see
//! *inside* our startup (our HTTP server isn't up yet), so we tell it which
//! phase we're in by POSTing the gateway's boot-phase control endpoint. The
//! gateway renders the matching label on the splash (see
//! `lucidos-gateway/src/boot_phase.rs`). (The embedding model is NOT a boot
//! phase — it loads in the background and never blocks boot; see
//! `memory::EmbedderSlot`.)
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
        // Loopback call to the co-located gateway; accept its self-signed dev
        // cert and bypass any ambient proxy — the same posture as the
        // Apply-restart callback (`api/history.rs::restart_via_gateway`).
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        // Scheme via `net_config::peer_scheme_order` (never hardcoded — the dev
        // gateway serves TLS, packaged serves plain http): resolved scheme
        // first, the other protocol as fallback so a mismatch still reports.
        //
        // Best-effort: the gateway's own health probe is the source of truth for
        // readiness; a dropped phase report only costs a slightly less specific
        // splash label, and the next phase (or the healthy probe) supersedes it.
        for scheme in crate::net_config::peer_scheme_order() {
            let url = format!(
                "{scheme}://127.0.0.1:{port}/~/api/v1/control/workspaces/{id}/boot-phase"
            );
            if client
                .post(&url)
                .json(&serde_json::json!({ "phase": phase }))
                .send()
                .await
                .is_ok()
            {
                return; // reached the gateway (any response) — done
            }
        }
    });
}
