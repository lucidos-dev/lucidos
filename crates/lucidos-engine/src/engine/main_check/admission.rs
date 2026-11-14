//! Repair admission samples chosen workspace work and the machine-wide build pool.
//! The settle clock is disposable: a restart begins a fresh idle window.
use std::time::{Duration, Instant};

pub(crate) const REPAIR_SETTLE: Duration = Duration::from_secs(120);
pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkSample {
    pub(crate) working_threads: bool,
    pub(crate) applying: bool,
    pub(crate) queued: bool,
    pub(crate) host_builds: bool,
}

impl WorkSample {
    pub(crate) fn waiting_on(&self) -> Vec<String> {
        [
            (self.working_threads, "running coding-agent work"),
            (self.applying, "an Apply in progress"),
            (self.queued, "queued work"),
            (self.host_builds, "machine builds"),
        ]
        .into_iter()
        .filter(|(busy, _)| *busy)
        .map(|(_, label)| label.to_string())
        .collect()
    }

    fn busy(&self) -> bool {
        self.working_threads || self.applying || self.queued || self.host_builds
    }
}

#[derive(Default)]
pub(crate) struct Admission {
    idle_since: Option<Instant>,
}

impl Admission {
    pub(crate) fn sample(&mut self, sample: &WorkSample, now: Instant) -> bool {
        if sample.busy() {
            self.idle_since = None;
            return false;
        }
        now.duration_since(*self.idle_since.get_or_insert(now)) >= REPAIR_SETTLE
    }

    pub(crate) fn reset(&mut self) {
        self.idle_since = None;
    }
}

pub(crate) async fn sample_work(pool: &sqlx::PgPool) -> Result<WorkSample, sqlx::Error> {
    let (working_threads, applying, queued): (bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE status = 'running' AND is_coding_agent AND NOT keep_alive_turn),
                EXISTS(SELECT 1 FROM thread_summaries WHERE apply_in_flight),
                EXISTS(SELECT 1 FROM thread_queue WHERE status = 'queued')"
    ).fetch_one(pool).await?;
    // Probing the flock pool takes no slot. Failure drops only this term.
    static POOL_WARNING: std::sync::Once = std::sync::Once::new();
    let host_builds =
        tokio::task::spawn_blocking(|| {
            match lucidos_build_slot::BuildSlotPool::open()
                .and_then(|pool| Ok(pool.held_slots() > 0 || pool.try_anyone_waiting()?))
            {
                Ok(busy) => busy,
                Err(error) => {
                    POOL_WARNING.call_once(|| {
                        log!(
                            "[MainCheck] build pool unavailable; admission uses workspace work: {}",
                            error
                        )
                    });
                    false
                }
            }
        })
        .await
        .unwrap_or(true);
    Ok(WorkSample {
        working_threads,
        applying,
        queued,
        host_builds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_busy_term_restarts_the_full_idle_window() {
        for busy in [
            WorkSample {
                working_threads: true,
                ..Default::default()
            },
            WorkSample {
                applying: true,
                ..Default::default()
            },
            WorkSample {
                queued: true,
                ..Default::default()
            },
            WorkSample {
                host_builds: true,
                ..Default::default()
            },
        ] {
            let mut gate = Admission::default();
            let now = Instant::now();
            let idle = WorkSample::default();
            assert!(!gate.sample(&idle, now));
            assert!(!gate.sample(&busy, now + REPAIR_SETTLE));
            assert!(!gate.sample(&idle, now + REPAIR_SETTLE));
            assert!(gate.sample(&idle, now + REPAIR_SETTLE * 2));
            assert_eq!(busy.waiting_on().len(), 1);
        }
    }

    #[test]
    fn a_restart_or_new_owner_requires_a_new_settle_window() {
        let mut gate = Admission::default();
        let now = Instant::now();
        let idle = WorkSample::default();
        gate.sample(&idle, now);
        gate.reset();
        assert!(!gate.sample(&idle, now + REPAIR_SETTLE));
        assert!(gate.sample(&idle, now + REPAIR_SETTLE * 2));
    }
}
