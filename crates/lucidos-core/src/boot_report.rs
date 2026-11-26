use std::time::{Duration, Instant};

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

pub const MIGRATING: &str = "migrating";
pub const RECOVERING: &str = "recovering";

pub fn report(phase: &str) {
    let (Ok(port), Ok(id)) = (
        std::env::var("LUCIDOS_GATEWAY_PORT"),
        std::env::var("LUCIDOS_WORKSPACE_ID"),
    ) else {
        return;
    };
    // INVARIANT: Detached so a slow/unreachable gateway can never stall the boot. We're
    // inside the async startup, so a runtime exists; guard anyway so a stray
    // call off-runtime is a no-op rather than a panic.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let phase = phase.to_string();
    handle.spawn(async move {
        let client = match crate::gateway_auth::client_builder()
            .timeout(Duration::from_secs(2))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let url = format!(
            "{}/~/api/v1/control/workspaces/{id}/boot-phase",
            lucidos_local_token::client::origin(&port)
        );
        let _ = client
            .post(&url)
            .json(&serde_json::json!({ "phase": phase }))
            .send()
            .await;
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
                ("pre-recovery setup", Duration::from_millis(3)),
                ("worktree recovery", Duration::from_millis(1200)),
            ],
        );
        assert_eq!(
            summary,
            "1500ms total: pre-recovery setup 3ms, worktree recovery 1200ms"
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
