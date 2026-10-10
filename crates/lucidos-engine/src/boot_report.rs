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

use std::time::{Duration, Instant};

/// Wall-clock time per named boot stage, logged as one line when the engine
/// binds. A slow boot then names its slow stage instead of leaving a gap
/// between unrelated log lines.
pub struct BootStageTimer {
    started: Instant,
    last_lap: Instant,
    stages: Vec<(&'static str, Duration)>,
}

impl BootStageTimer {
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_lap: now,
            stages: Vec::new(),
        }
    }

    /// Close the stage that ran since the previous lap, under `name`.
    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.stages.push((name, now - self.last_lap));
        self.last_lap = now;
    }

    pub fn summary(&self) -> String {
        format_stage_summary(self.started.elapsed(), &self.stages)
    }
}

fn format_stage_summary(total: Duration, stages: &[(&'static str, Duration)]) -> String {
    let stages: Vec<String> = stages
        .iter()
        .map(|(name, d)| format!("{name} {}ms", d.as_millis()))
        .collect();
    format!("{}ms total: {}", total.as_millis(), stages.join(", "))
}

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
        let client = match crate::gateway_auth::client_builder()
            .timeout(Duration::from_secs(2))
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
            let url =
                format!("{scheme}://127.0.0.1:{port}/~/api/v1/control/workspaces/{id}/boot-phase");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_summary_lists_every_stage_in_order_after_the_total() {
        let summary = format_stage_summary(
            Duration::from_millis(1500),
            &[
                ("startup lease", Duration::from_millis(3)),
                ("worktree recovery", Duration::from_millis(1200)),
            ],
        );
        assert_eq!(
            summary,
            "1500ms total: startup lease 3ms, worktree recovery 1200ms"
        );
    }

    #[test]
    fn a_lap_records_the_time_since_the_previous_lap() {
        let mut timer = BootStageTimer::start();
        timer.lap("first");
        std::thread::sleep(Duration::from_millis(5));
        timer.lap("second");
        let names: Vec<&str> = timer.stages.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["first", "second"]);
        assert!(timer.stages[1].1 >= Duration::from_millis(5));
    }
}
