//! Following `origin`'s `main` (ADR 0589). Another gateway lands on `origin`,
//! so this engine asks `origin` on an interval and fast-forwards local `main`
//! to it. Only while its gateway holds a pairing: an unpaired engine lands
//! local-first, and nobody else lands for it. A pulled version then builds in the background, and switching to it
//! waits for a tap: nothing here ever restarts the engine.
//!
//! A paired gateway that just landed nudges this task through its gateway, so
//! it asks at once. A lost nudge costs one interval, never a missed version.
//!
//! Every answer from `origin` also tells the landing lane, which retries a
//! landing that waited for `origin`. The lane itself keeps no clock.
//!
//! Generic over [`FollowHost`], so the reactions run in tests without an
//! engine.

use super::git_ops::{follow_origin_main, FollowOutcome, LandingOrder, MAIN_LOCKS};
use super::LucidosEngine;
use crate::core::prefs;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// What following `main` needs from the engine it runs in.
#[async_trait::async_trait]
pub(crate) trait FollowHost: Send + Sync + 'static {
    /// The repository whose `main` follows `origin`.
    fn follow_repo(&self) -> &Path;
    async fn follow_interval(&self) -> Duration;
    /// Origin-first while the gateway holds a pairing. Asked every round.
    async fn landing_order(&self) -> LandingOrder;
    /// Wakes the task before the interval runs out.
    fn follow_nudge(&self) -> &Notify;
    /// `origin` answered, so a landing waiting for it can try again.
    fn origin_answered(&self);
    /// Build the version `main` moved to. Never a restart.
    fn start_build(self: &Arc<Self>);
    /// Tell the user why `main` could not follow `origin`.
    async fn follow_refused(&self, reason: String);
}

/// What the follow task remembers between rounds, so a lasting refusal or
/// outage is reported once, not every round.
#[derive(Default)]
pub(crate) struct Follower {
    refused: Option<String>,
    unreachable: Option<String>,
}

impl Follower {
    pub(crate) async fn react<H: FollowHost>(&mut self, host: &Arc<H>, outcome: FollowOutcome) {
        match outcome {
            FollowOutcome::NoOrigin => return,
            FollowOutcome::Unreachable(why) => {
                if self.unreachable.as_ref() != Some(&why) {
                    log!("[FollowMain] origin did not answer: {}", why);
                }
                self.unreachable = Some(why);
                return;
            }
            FollowOutcome::UpToDate => self.refused = None,
            FollowOutcome::Followed { from, to } => {
                log!("[FollowMain] main followed origin from {} to {}", from, to);
                self.refused = None;
                host.start_build();
            }
            FollowOutcome::Refused(why) => {
                if self.refused.as_ref() != Some(&why) {
                    log!("[FollowMain] main could not follow origin: {}", why);
                    host.follow_refused(why.clone()).await;
                }
                self.refused = Some(why);
            }
        }
        self.unreachable = None;
        host.origin_answered();
    }
}

/// Ask `origin` once now, then once per interval, for as long as the engine
/// runs. A repository without `origin` costs one local git call a round.
pub(crate) fn spawn<H: FollowHost>(host: Arc<H>) {
    tokio::spawn(async move {
        let mut follower = Follower::default();
        loop {
            follow_round(&host, &mut follower).await;
            wait_for_next_round(host.as_ref()).await;
        }
    });
}

/// One round: follow `origin` only while paired.
pub(crate) async fn follow_round<H: FollowHost>(host: &Arc<H>, follower: &mut Follower) {
    match host.landing_order().await {
        LandingOrder::OriginFirst => {
            let outcome = follow_origin_main(&MAIN_LOCKS, host.follow_repo()).await;
            follower.react(host, outcome).await;
        }
        // A landing that waited for `origin` or the gateway now lands locally.
        LandingOrder::LocalFirst => host.origin_answered(),
        LandingOrder::Unknown(why) => log!("[FollowMain] skipped a round: {}", why),
    }
}

/// Sleep one interval, or less when a nudge arrives. A nudge sent during a
/// round is kept, so it still buys one more round.
pub(crate) async fn wait_for_next_round<H: FollowHost>(host: &H) {
    let interval = host.follow_interval().await;
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        _ = host.follow_nudge().notified() => {}
    }
}

impl LucidosEngine {
    /// Start following `origin`'s `main`. A packaged install follows
    /// releases instead, so it starts nothing.
    pub fn start_follow_main(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        spawn(self.clone());
    }

    /// A paired gateway landed, so ask `origin` now rather than at the next
    /// interval. Harmless on an engine that does not follow.
    pub fn nudge_follow_main(&self) {
        self.follow_main_nudge.notify_one();
    }
}

#[async_trait::async_trait]
impl FollowHost for LucidosEngine {
    fn follow_repo(&self) -> &Path {
        self.repo_root()
    }

    async fn follow_interval(&self) -> Duration {
        let minutes = prefs::FOLLOW_MAIN_INTERVAL_MINUTES.read(self.pool()).await;
        Duration::from_secs_f64(minutes * 60.0)
    }

    async fn landing_order(&self) -> LandingOrder {
        crate::landing_gateway::landing_order().await
    }

    fn follow_nudge(&self) -> &Notify {
        &self.follow_main_nudge
    }

    fn origin_answered(&self) {
        self.notify_origin_answered();
    }

    fn start_build(self: &Arc<Self>) {
        self.join_or_start_background_rebuild();
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
}

#[cfg(test)]
#[path = "follow_main_tests.rs"]
mod tests;
