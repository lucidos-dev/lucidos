//! Claude Code session driver, split by lifecycle stage.
//!
//! - [`run`] — the `run_direct_agent` driver: pre-spawn dedup/resolution, the
//!   spawn, and the `'event_loop` stream pump.
//! - [`spawn_context`] — the "start" stage: worktree / branch / system-prompt
//!   resolution ([`spawn_context::SpawnWorktreeContext`]).
//! - [`completion`] — the teardown stage: safety net, cleanup, change proposal.
//! - [`idle_snapshot`] — `CodingAgentIdled` emission helper.
//! - [`entry_guard`] — RAII backstop that reaps the `agent_sessions` entry when
//!   the run future is dropped instead of completed.

mod completion;
mod entry_guard;
mod idle_snapshot;
mod run;
mod spawn_context;

#[cfg(test)]
#[path = "../run_session_tests.rs"]
mod tests;
