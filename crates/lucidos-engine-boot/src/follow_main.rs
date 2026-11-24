//! Following `origin`'s `main` (ADR 0589). Another gateway lands on `origin`,
//! so this engine asks `origin` on an interval and fast-forwards local `main`
//! to it. Only while its gateway holds a pairing: an unpaired engine lands
//! local-first, and nobody else lands for it. A pulled version builds in the background
//! when automatic builds are enabled, and switching to it
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

use lucidos_core::log_legacy;
use lucidos_engine::engine::follow_main::{follow_source_main, FollowHost};
use lucidos_engine::engine::git_ops::{FollowOutcome, LandingOrder};
use lucidos_engine::runtime::is_packaged;
use std::sync::Arc;

/// What the follow task remembers between rounds, so a lasting refusal or
/// outage is reported once, not every round.
#[derive(Default)]
struct Follower {
    refused: Option<String>,
    unreachable: Option<String>,
}

impl Follower {
    async fn react<H: FollowHost>(&mut self, host: &Arc<H>, outcome: FollowOutcome) {
        match outcome {
            FollowOutcome::NoOrigin => return,
            FollowOutcome::Unreachable(why) => {
                if self.unreachable.as_ref() != Some(&why) {
                    log_legacy!("[FollowMain] origin did not answer: {}", why);
                }
                self.unreachable = Some(why);
                return;
            }
            FollowOutcome::UpToDate => self.refused = None,
            FollowOutcome::Followed { from, to } => {
                log_legacy!("[FollowMain] main followed origin from {} to {}", from, to);
                self.refused = None;
                host.main_followed(from, to).await;
                if host.automatic_builds_enabled().await {
                    host.start_build().await;
                }
            }
            FollowOutcome::Refused(why) => {
                if self.refused.as_ref() != Some(&why) {
                    log_legacy!("[FollowMain] main could not follow origin: {}", why);
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
/// A packaged install follows releases instead.
pub fn spawn<H: FollowHost>(host: &Arc<H>) {
    if is_packaged() {
        return;
    }
    let host = host.clone();
    tokio::spawn(async move {
        let mut follower = Follower::default();
        loop {
            follow_round(&host, &mut follower).await;
            wait_for_next_round(host.as_ref()).await;
        }
    });
}

/// Follow `origin` when the landing order requires it.
async fn follow_round<H: FollowHost>(host: &Arc<H>, follower: &mut Follower) {
    match host.landing_order().await {
        LandingOrder::OriginFirst => {
            let outcome = follow_source_main(host.follow_repo()).await;
            follower.react(host, outcome).await;
        }
        // A landing that waited for `origin` or the gateway now lands locally.
        LandingOrder::LocalFirst => host.origin_answered(),
        LandingOrder::Unknown(why) => log_legacy!("[FollowMain] skipped a round: {}", why),
    }
}

/// Sleep one interval, or less when a nudge arrives. A nudge sent during a
/// round is kept, so it still buys one more round.
async fn wait_for_next_round<H: FollowHost>(host: &H) {
    let interval = host.follow_interval().await;
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        _ = host.follow_nudge().notified() => {}
    }
}

#[cfg(test)]
#[path = "follow_main_tests.rs"]
mod tests;
