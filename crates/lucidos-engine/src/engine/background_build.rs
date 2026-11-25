//! This engine's background engine build (dev): when one starts, what a
//! second request does to it, and when it builds once more.
//!
//! Entry points, by who asks:
//!
//! - [`automatic`], for Apply, follow main and self-heal. Coding activity can
//!   hold a request before it starts. A build in flight is
//!   left alone and marked joined. When it finishes behind HEAD, the same task
//!   builds once more. A burst of Applies so costs at most one follow-up, and
//!   no compile is thrown away.
//! - [`join_or_start`], the explicit Build now override, joins or starts immediately.
//! - [`restart`], for the explicit `POST /api/v1/engine/rebuild`. It aborts the
//!   build in flight and starts over: the user's escape from a hung build.
//!
//! Generic over [`BuildHost`], so the rules run in tests without cargo.

pub(crate) use super::build_deferral::BuildDeferral;
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
    async fn run_build(&self, previous_failure: Option<&str>, generation: u64) -> FinishedBuild;
    /// Whether `finished` already reflects HEAD, so a join needs no follow-up.
    async fn covers_head(&self, finished: &FinishedBuild) -> bool;
    async fn automatic_deferral(&self) -> BuildDeferral;
    /// Recheck coverage when a held request resumes: a peer may have built it.
    async fn deferred_build_needed(&self) -> bool;
}

/// One completed build and the checkout HEAD it last tried to cover.
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

/// Apply a running build's update atomically with the restart generation check.
pub(crate) fn with_current_build<H: BuildHost, R>(
    host: &H,
    generation: u64,
    update: impl FnOnce() -> R,
) -> Option<R> {
    let slot = host.background_build().slot.lock().unwrap();
    if slot.generation != generation {
        return None;
    }
    let result = update();
    drop(slot);
    Some(result)
}

/// Start a build, or join the one in flight without disturbing it.
pub(crate) fn join_or_start<H: BuildHost>(host: &Arc<H>) {
    let mut slot = host.background_build().slot.lock().unwrap();
    if slot.running() {
        slot.joined = true;
        crate::log_legacy!(
            "[Rebuild] joined the engine build in flight; it builds once more if HEAD has moved past it"
        );
        return;
    }
    start(host, &mut slot, None);
}

/// Automatic requests share one hold; explicit requests bypass admission.
pub(crate) async fn automatic<H: BuildHost>(host: &Arc<H>) -> BuildDeferral {
    let deferral = host.automatic_deferral().await;
    automatic_with_deferral(host, deferral).await
}

/// Use one admission result for both self-heal accounting and the request.
pub(crate) async fn automatic_with_deferral<H: BuildHost>(
    host: &Arc<H>,
    deferral: BuildDeferral,
) -> BuildDeferral {
    let changed = {
        let mut slot = host.background_build().slot.lock().unwrap();
        if slot.running() {
            slot.joined = true;
            return BuildDeferral::Admit;
        }
        match &deferral {
            BuildDeferral::Admit => {
                start(host, &mut slot, None);
                None
            }
            BuildDeferral::Defer(_) => {
                let state = host.build_state().with_deferral();
                let changed = host.build_state().deferred().is_none();
                host.set_build_state(state.clone());
                changed.then_some(state)
            }
        }
    };
    if let Some(state) = changed {
        host.emit_build_state_changed(&state).await;
    }
    deferral
}

pub(crate) async fn start_deferred_build_if_admitted<H: BuildHost>(host: &Arc<H>) {
    if host.build_state().deferred().is_none() {
        return;
    }
    let deferral = host.automatic_deferral().await;
    if !matches!(deferral, BuildDeferral::Admit) {
        return;
    }
    let needed = host.deferred_build_needed().await;
    let changed = {
        let mut slot = host.background_build().slot.lock().unwrap();
        if slot.running() || host.build_state().deferred().is_none() {
            return;
        }
        if needed {
            start(host, &mut slot, None);
            None
        } else {
            let state = match host.build_state() {
                BuildState::Ready { built_head, .. } => BuildState::ready_from(built_head),
                _ => BuildState::Idle,
            };
            host.set_build_state(state.clone());
            Some(state)
        }
    };
    if let Some(state) = changed {
        host.emit_build_state_changed(&state).await;
    }
}

/// Abort the build in flight, killing its process group, and start over.
pub(crate) fn restart<H: BuildHost>(host: &Arc<H>) {
    let mut slot = host.background_build().slot.lock().unwrap();
    let superseded = slot.task.take().filter(|t| !t.is_finished());
    if let Some(old) = &superseded {
        old.abort();
        crate::log_legacy!("[Rebuild] restarting the engine build in flight on request");
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
    // it, so repeated failures retain their diagnostic classification.
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
    // immediately. A version-status read can miss this transient window;
    // the stream also invalidates the snapshot for the next read.
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
        let finished = host
            .run_build(previous_failure.as_deref(), generation)
            .await;
        match settle(&host, generation, &finished).await {
            Settled::Done => break host.build_state(),
            Settled::Restarted => return,
            Settled::BuildAgain => {
                crate::log_legacy!(
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
            let deferral = host.automatic_deferral().await;
            let mut slot = host.background_build().slot.lock().unwrap();
            if slot.generation != generation {
                return Settled::Restarted;
            }
            if matches!(deferral, BuildDeferral::Defer(_)) {
                slot.task = None;
                host.set_build_state(finished.state.clone().with_deferral());
                return Settled::Done;
            }
            return Settled::BuildAgain;
        }
    }
}

#[cfg(test)]
#[path = "background_build_tests.rs"]
mod tests;
