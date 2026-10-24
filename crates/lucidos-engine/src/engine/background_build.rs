//! This engine's background engine build (dev): when one starts, what a
//! second request does to it, and when it builds once more.
//!
//! Two entry points, by who asks:
//!
//! - [`join_or_start`], for an Apply and for self-heal. A build in flight is
//!   left alone and marked joined. When it finishes behind HEAD, the same task
//!   builds once more. A burst of Applies so costs at most one follow-up, and
//!   no compile is thrown away.
//! - [`restart`], for the explicit `POST /api/v1/engine/rebuild`. It aborts the
//!   build in flight and starts over: the user's escape from a hung build.
//!
//! Generic over [`BuildHost`], so the rules run in tests without cargo.

use crate::engine::engine_version::BuildState;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// What a background build needs from the engine it runs in.
#[async_trait::async_trait]
pub(crate) trait BuildHost: Send + Sync + 'static {
    fn background_build(&self) -> &BackgroundBuild;
    fn build_state(&self) -> BuildState;
    fn set_build_state(&self, state: BuildState);
    async fn emit_build_state_changed(&self, state: &BuildState);
    /// Run one build. `previous_failure` is the failure it retries, if any.
    async fn run_build(&self, previous_failure: Option<&str>) -> FinishedBuild;
    /// Whether `finished` already reflects HEAD, so a join needs no follow-up.
    async fn covers_head(&self, finished: &FinishedBuild) -> bool;
}

/// One completed build: the state it settles to, and the HEAD it started at.
#[derive(Debug, Clone)]
pub(crate) struct FinishedBuild {
    pub(crate) state: BuildState,
    pub(crate) built_head: Option<String>,
}

/// The build task in flight, held as one unit under one lock. A join, a
/// restart and a task settling its result therefore never interleave.
#[derive(Default)]
pub(crate) struct BackgroundBuild {
    slot: Mutex<BuildSlot>,
}

#[derive(Default)]
struct BuildSlot {
    /// The running task. The task clears it when it settles, so `Some` with an
    /// unfinished handle means a build is in flight.
    task: Option<JoinHandle<()>>,
    /// Bumped by every start. A task whose generation is no longer current was
    /// restarted, and must not touch the slot or the build state.
    generation: u64,
    /// An Apply or self-heal arrived while the task was building.
    joined: bool,
}

impl BuildSlot {
    fn running(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.is_finished())
    }
}

/// Start a build, or join the one in flight without disturbing it.
pub(crate) fn join_or_start<H: BuildHost>(host: &Arc<H>) {
    let mut slot = host.background_build().slot.lock().unwrap();
    if slot.running() {
        slot.joined = true;
        crate::log!(
            "[Rebuild] joined the engine build in flight; it builds once more if HEAD has moved past it"
        );
        return;
    }
    start(host, &mut slot, None);
}

/// Abort the build in flight, killing its process group, and start over.
pub(crate) fn restart<H: BuildHost>(host: &Arc<H>) {
    let mut slot = host.background_build().slot.lock().unwrap();
    let superseded = slot.task.take().filter(|t| !t.is_finished());
    if let Some(old) = &superseded {
        old.abort();
        crate::log!("[Rebuild] restarting the engine build in flight on request");
    }
    start(host, &mut slot, superseded);
}

fn start<H: BuildHost>(host: &Arc<H>, slot: &mut BuildSlot, superseded: Option<JoinHandle<()>>) {
    slot.generation += 1;
    slot.joined = false;
    // Stamped once here, so the state the SSE poke reports and the state the
    // elapsed counter reads are the same start moment.
    let building = BuildState::building_now();
    // Read the failure this build is retrying BEFORE `Building` overwrites
    // it. An identical repeat is the only PROOF that retrying is futile,
    // and it is what lets the toast withhold Retry.
    let previous_failure = host.build_state().failure().map(|f| f.summary.clone());
    host.set_build_state(building.clone());
    slot.task = Some(tokio::spawn(drive(
        host.clone(),
        slot.generation,
        building,
        superseded,
        previous_failure,
    )));
}

/// The build task: build, then build again while a join left HEAD ahead.
async fn drive<H: BuildHost>(
    host: Arc<H>,
    generation: u64,
    building: BuildState,
    superseded: Option<JoinHandle<()>>,
    mut previous_failure: Option<String>,
) {
    // Push `building` over SSE so a connected client shows the spinner
    // immediately. The version-status poll alone misses this transient
    // window, since iOS suspends the timer on a backgrounded PWA.
    host.emit_build_state_changed(&building).await;
    // HAND THE BUILD LOCK OVER, do not race it. `abort()` only REQUESTS
    // cancellation, so the restarted task drops its `flock` guard strictly
    // after `abort()` returns. Probing the lock before then reads that guard
    // as a peer's, returns `SkippedLocked`, and leaves NO build running.
    //
    // The handle resolves once the task has stopped and its locals are
    // dropped. Every await in it is cancel-safe and already unblocked.
    if let Some(old) = superseded {
        let _ = old.await;
    }
    let settled = loop {
        let finished = host.run_build(previous_failure.as_deref()).await;
        match settle(&host, generation, &finished).await {
            Settled::Done => break finished.state,
            Settled::Restarted => return,
            Settled::BuildAgain => {
                crate::log!(
                    "[Rebuild] HEAD moved past the engine build an Apply joined; building once more"
                );
                previous_failure = finished.state.failure().map(|f| f.summary.clone());
            }
        }
    };
    host.emit_build_state_changed(&settled).await;
}

enum Settled {
    /// The slot is released and the build state holds the result.
    Done,
    /// A restart replaced this task. It must leave everything alone.
    Restarted,
    /// A join arrived and HEAD is ahead of what this build produced.
    BuildAgain,
}

/// Decide what a finished build does next. Retiring happens under the slot
/// lock, so a join that lands after this check starts a fresh build instead.
async fn settle<H: BuildHost>(host: &Arc<H>, generation: u64, finished: &FinishedBuild) -> Settled {
    loop {
        {
            let mut slot = host.background_build().slot.lock().unwrap();
            if slot.generation != generation {
                return Settled::Restarted;
            }
            if !slot.joined {
                slot.task = None;
                host.set_build_state(finished.state.clone());
                return Settled::Done;
            }
            slot.joined = false;
        }
        // The check takes a git call, and a join may land during it. Looping
        // back to the slot catches that join before retiring.
        if !host.covers_head(finished).await {
            return Settled::BuildAgain;
        }
    }
}

#[cfg(test)]
#[path = "background_build_tests.rs"]
mod tests;
