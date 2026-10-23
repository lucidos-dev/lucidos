//! The *main check* (ADR 0464): after a landing moves `main`, the engine runs
//! the engine suite and every e2e suite on GitHub against a stripped copy
//! of `main`. It gates nothing. One check runs at a time, and the next covers
//! everything that landed meanwhile. A red check opens one *fix thread*, and
//! no check starts while it is open, until a fix lands or the maintainer
//! closes it.
//!
//! - `state`: the pure scheduler.
//! - `runner`: the child that runs `scripts/main-check.sh`.
//! - `fix_thread`: the red check's fix thread, its title and prompt.
//! - `recovery`: the check rebuilt from events after a restart.
//! - `snapshot`: the check as clients see it.
//! - `driver`: the task that runs it all, one message at a time.

mod driver;
mod fix_thread;
mod recovery;
mod runner;
mod snapshot;
mod state;

pub(crate) use driver::MainCheckMsg;
pub(crate) use runner::RunnerSlot;
pub(crate) use snapshot::MainCheckSnapshot;
pub(crate) use state::CloseRefusal;
pub use state::FailedTest;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use driver::{Driver, DriverHooks, DriverSetup};
use runner::MainCheckCommand;

/// Why a red main check closed, on `MainCheckRedClosed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedCloseReason {
    /// A change from its fix thread landed.
    FixLanded,
    /// The maintainer closed it.
    Closed,
}

/// The check script, relative to the repository root.
const MAIN_CHECK_SCRIPT: &str = "scripts/main-check.sh";

/// The event types [`landing_of`] reads a landing from. The landing feed
/// replays only these after a lag.
const LANDING_EVENT_TYPES: [&str; 1] = ["ChangeApplied"];

/// What a bus event tells the main check: a `ChangeApplied` with a landing
/// merge is a Lucidos-source change that moved `main`. Only the landing lane
/// records one.
pub(crate) fn landing_of(event: &BusEvent) -> Option<MainCheckMsg> {
    match event {
        BusEvent::Thread {
            thread_id,
            event:
                ThreadEvent::ChangeApplied {
                    landing_merge_sha: Some(_),
                    pre_merge_sha,
                    ..
                },
            ..
        } => Some(MainCheckMsg::Landed {
            thread_id: *thread_id,
            pre_sha: pre_merge_sha.clone(),
        }),
        _ => None,
    }
}

/// Hand `notify` every landing one read of the bus carries, including what a
/// lag dropped. `false` once the bus closes.
async fn forward_landings(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    pool: &sqlx::PgPool,
    notify: impl Fn(MainCheckMsg),
) -> bool {
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for msg in frames.iter().filter_map(|frame| landing_of(&frame.typed)) {
        notify(msg);
    }
    true
}

impl LucidosEngine {
    /// Hand a message to the main check. A closed channel drops it: recovery
    /// rebuilds the check from events at the next start.
    pub(crate) fn notify_main_check(&self, msg: MainCheckMsg) {
        if self.main_check_tx.send(msg).is_err() {
            log!("[MainCheck] driver gone, dropping a message");
        }
    }

    /// The main check as clients see it, as the driver last published it.
    pub(crate) fn main_check_snapshot(&self) -> MainCheckSnapshot {
        match self.main_check_snapshot.read() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Close the red check for the maintainer. `Err` when the driver stopped
    /// before it answered; the inner `Err` when no red is open.
    pub(crate) async fn close_main_check_red(
        &self,
        actor: Option<MessageOrigin>,
    ) -> Result<Result<(), CloseRefusal>, String> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.notify_main_check(MainCheckMsg::Close { actor, reply });
        answer
            .await
            .map_err(|_| "The main check stopped before it answered.".to_string())
    }

    /// Start the main check's driver and the bus subscriber that feeds it
    /// landings. Spawned once at engine startup.
    pub fn start_main_check_driver(self: &Arc<Self>) {
        let rx = crate::engine::MAIN_CHECK_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            log!("[MainCheck] driver receiver missing, main check not started");
            return;
        };
        let repo_root = self.repo_root().to_path_buf();
        let setup = DriverSetup {
            pool: self.pool().clone(),
            bus: self.event_bus.clone(),
            command: MainCheckCommand {
                program: repo_root.join(MAIN_CHECK_SCRIPT),
                cwd: repo_root.clone(),
            },
            repo_root,
            workspace_name: self.workspace_name(),
            tx: self.main_check_tx.clone(),
            runner: self.main_check_runner.clone(),
        };
        let opener = self.clone();
        let publisher = self.clone();
        let hooks = DriverHooks {
            open_fix_thread: Box::new(move |opening| {
                opener.spawn_engine_coding_turn_as(
                    opening.thread_id,
                    opening.prompt,
                    Some(opening.title),
                    opening.reason,
                    opening.implementer,
                    |_| {},
                );
            }),
            publish: Box::new(move |snapshot| {
                match publisher.main_check_snapshot.write() {
                    Ok(mut published) => *published = snapshot,
                    Err(poisoned) => *poisoned.into_inner() = snapshot,
                }
                let engine = publisher.clone();
                tokio::spawn(async move { engine.broadcast_changes_updated().await });
            }),
        };
        let mut driver = Driver::new(setup, hooks);
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                driver.deliver(msg).await;
            }
            log!("[MainCheck] driver exiting, channel closed");
        });
        self.start_main_check_landing_feed();
    }

    /// Feed the driver every landing the bus announces. A lag replays the
    /// landings it dropped, so a fix landing still reaches its red.
    fn start_main_check_landing_feed(self: &Arc<Self>) {
        let (mut rx, mut replay) = LagReplay::subscribe(
            &self.event_bus,
            ReplayScope::Workspace,
            Some(LANDING_EVENT_TYPES.to_vec()),
        );
        let engine = self.clone();
        tokio::spawn(async move {
            while forward_landings(rx.recv().await, &mut replay, engine.pool(), |msg| {
                engine.notify_main_check(msg)
            })
            .await
            {}
        });
    }

    /// Rebuild the main check from events after a restart.
    pub fn recover_main_check(&self) {
        self.notify_main_check(MainCheckMsg::Restore);
    }

    /// Stop the running check's child at shutdown. The next start reattaches
    /// to a run on record, and abandons a check whose run is not.
    pub async fn stop_main_check(&self) {
        self.main_check_runner.stop().await;
    }
}

#[cfg(test)]
#[path = "integration_tests.rs"]
mod integration_tests;
