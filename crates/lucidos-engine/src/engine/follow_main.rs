//! Engine operations used by the source `main` follower.

use super::git_ops::{follow_origin_main, FollowOutcome, LandingOrder, MAIN_LOCKS};
use super::LucidosEngine;
use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Let boot's follower test implement the host trait without a new dependency.
#[doc(hidden)]
pub use async_trait::async_trait;

/// Follow source `main` under the same merge lock the landing lane uses.
pub async fn follow_source_main(repo_root: &Path) -> FollowOutcome {
    follow_origin_main(&MAIN_LOCKS, repo_root).await
}

/// What following `main` needs from the engine it runs in.
#[async_trait]
pub trait FollowHost: Send + Sync + 'static {
    /// The repository whose `main` follows `origin`.
    fn follow_repo(&self) -> &Path;
    async fn follow_interval(&self) -> Duration;
    async fn automatic_builds_enabled(&self) -> bool;
    /// Origin-first while the gateway holds a pairing. Asked every round.
    async fn landing_order(&self) -> LandingOrder;
    /// Wakes the task before the interval runs out.
    fn follow_nudge(&self) -> &Notify;
    /// `origin` answered, so a landing waiting for it can try again.
    fn origin_answered(&self);
    /// Build the version `main` moved to. Never a restart.
    async fn start_build(self: &Arc<Self>);
    /// Tell the user why `main` could not follow `origin`.
    async fn follow_refused(&self, reason: String);
    /// Record that `main` followed `origin`, which the main check reads as a
    /// landing.
    async fn main_followed(&self, from: String, to: String);
}

impl LucidosEngine {
    /// Read the workspace policy for every automatic build decision. Explicit
    /// builds and local Apply deliberately do not use this gate.
    pub(crate) async fn automatic_source_builds_enabled(&self) -> bool {
        automatic_source_builds_enabled(self.pool()).await
    }

    /// A paired gateway landed, so ask `origin` now rather than at the next
    /// interval. Harmless on an engine that does not follow.
    pub fn nudge_follow_main(&self) {
        self.follow_main_nudge.notify_one();
    }
}

async fn automatic_source_builds_enabled(pool: &sqlx::PgPool) -> bool {
    prefs::FOLLOW_MAIN_BUILD.read(pool).await == "auto"
}

#[async_trait]
impl FollowHost for LucidosEngine {
    fn follow_repo(&self) -> &Path {
        self.repo_root()
    }

    async fn follow_interval(&self) -> Duration {
        let minutes = prefs::FOLLOW_MAIN_INTERVAL_MINUTES.read(self.pool()).await;
        Duration::from_secs_f64(minutes * 60.0)
    }

    async fn automatic_builds_enabled(&self) -> bool {
        self.automatic_source_builds_enabled().await
    }

    async fn landing_order(&self) -> LandingOrder {
        match prefs::LANDING_SETTING.try_read(self.pool()).await {
            Ok(setting) if setting == "another-host" => LandingOrder::OriginFirst,
            // An atomic push can land on origin even if its response is lost.
            // Keep following main while a remote member awaits reconciliation.
            Ok(_) if !self.landing_lane_snapshot().remote_members.is_empty() => {
                LandingOrder::OriginFirst
            }
            Ok(_) => crate::landing_gateway::landing_order().await,
            Err(error) => {
                LandingOrder::Unknown(format!("could not read the landing setting: {error}"))
            }
        }
    }

    fn follow_nudge(&self) -> &Notify {
        &self.follow_main_nudge
    }

    fn origin_answered(&self) {
        self.notify_origin_answered();
    }

    async fn start_build(self: &Arc<Self>) {
        self.automatic_background_rebuild().await;
    }

    async fn follow_refused(&self, reason: String) {
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::MainFollowRefused {
                    repo: self.repo_root().display().to_string(),
                    reason,
                }),
                "[FollowMain] MainFollowRefused",
            )
            .await;
    }

    async fn main_followed(&self, from: String, to: String) {
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::MainFollowed {
                    repo: self.repo_root().display().to_string(),
                    from_sha: from,
                    to_sha: to,
                }),
                "[FollowMain] MainFollowed",
            )
            .await;
    }
}

#[cfg(test)]
#[path = "follow_main_tests.rs"]
mod tests;
