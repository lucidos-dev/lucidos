//! *Thread triage*: which inbox threads need the user, and what to do with the
//! rest (ADR 0349).
//!
//! One classifier answers for two callers. The `threads` tool's `triage`
//! action proposes an action per thread, and Archive all puts away only what
//! this module calls safe. A thread is judged from facts, never its title.

pub(crate) mod archivable_count;
pub(crate) mod archive_all;
pub(crate) mod facts;
pub(crate) mod proposal;
pub(crate) mod report;
pub(crate) mod tool;

#[cfg(test)]
mod tests;

pub(crate) use crate::engine::triage_policy::*;
