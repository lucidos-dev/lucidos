//! Projection updaters for `EventBus`. Both methods take an open
//! `sqlx::Transaction` and run inside the same DB tx as the event insert,
//! so a projection failure rolls back the event itself — never persist an
//! event whose projection didn't apply.
//!
//! Re-export barrel: this module is declared via `#[path]` inside `event_bus`
//! and stays a single file so that declaration never moves. The bulk lives in
//! the sibling files declared below, split by projection responsibility:
//!
//! - `event_bus_projection_thread.rs`      — the `thread_summaries` projection
//!   (`update_thread_projection`)
//! - `event_bus_projection_system.rs`      — the `notifications` projection
//!   (`update_system_projection`)
//! - `event_bus_projection_propagation.rs` — blocking/attention propagation
//!   helpers + `rebuild_blocking_descendant_count`
//!
//! The `impl EventBus` methods keep their original call paths; the helpers are
//! shared sibling-to-sibling, so nothing outside this module changes.

#[path = "event_bus_projection_propagation.rs"]
mod propagation;
#[path = "event_bus_projection_system.rs"]
mod system;
#[path = "event_bus_projection_thread.rs"]
mod thread;
