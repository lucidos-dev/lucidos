//! The chat agentic loop, split into the driver and its free helpers.
//!
//! - [`run`] holds `run_agentic_loop`, the cohesive loop driver.
//! - [`helpers`] holds the free helper functions / small types it uses.
//!
//! Helpers are re-exported here so existing `agentic_loop::X` callers (and the
//! `super::X` references in the special-tool and test sibling modules) keep
//! resolving unchanged.

mod helpers;
mod run;

pub(crate) use helpers::*;

#[path = "../agentic_loop_special_tool.rs"]
mod special_tool;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/flush.rs"]
mod flush_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/call_key.rs"]
mod call_key_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/serialize.rs"]
mod serialize_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/flush_text.rs"]
mod flush_text_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/sentinel.rs"]
mod sentinel_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/capture.rs"]
mod capture_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/injection_coalescing.rs"]
mod injection_coalescing_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_tests.rs"]
mod agentic_loop_db_tests;
