use super::git_ops::{follow_origin_main, FollowOutcome, LandingOrder, MAIN_LOCKS};
use super::LucidosEngine;
use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[doc(hidden)]
pub use async_trait::async_trait;

pub async fn follow_source_main(repo_root: &Path) -> FollowOutcome {
    follow_origin_main(&MAIN_LOCKS, repo_root).await
}

#[async_trait]
pub trait FollowHost: Send + Sync + 'static {
    fn follow_repo(&self) -> &Path;
    async fn follow_interval(&self) -> Duration;
    async fn automatic_builds_enabled(&self) -> bool;
    async fn landing_order(&self) -> LandingOrder;
    fn follow_nudge(&self) -> &Notify;
    fn origin_answered(&self);
    async fn start_build(self: &Arc<Self>);
    async fn follow_refused(&self, reason: String);
    /// INVARIANT: Record that `main` followed `origin`, which the main check reads as a
    /// landing.
    async fn main_followed(&self, from: String, to: String);
}

impl LucidosEngine {
    pub(crate) async fn automatic_source_builds_enabled(&self) -> bool {
        automatic_source_builds_enabled(self.pool()).await
    }

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
            // INVARIANT: An atomic push can land on origin even if its response is lost.
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
