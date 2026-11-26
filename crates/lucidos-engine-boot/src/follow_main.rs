use lucidos_core::log_legacy;
use lucidos_engine::engine::follow_main::{follow_source_main, FollowHost};
use lucidos_engine::engine::git_ops::{FollowOutcome, LandingOrder};
use lucidos_engine::runtime::is_packaged;
use std::sync::Arc;

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

async fn follow_round<H: FollowHost>(host: &Arc<H>, follower: &mut Follower) {
    match host.landing_order().await {
        LandingOrder::OriginFirst => {
            let outcome = follow_source_main(host.follow_repo()).await;
            follower.react(host, outcome).await;
        }
        LandingOrder::LocalFirst => host.origin_answered(),
        LandingOrder::Unknown(why) => log_legacy!("[FollowMain] skipped a round: {}", why),
    }
}

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
