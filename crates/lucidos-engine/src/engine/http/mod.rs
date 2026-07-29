//! HTTP helpers for engine-initiated outbound calls.
//!
//! All workspace-to-workspace traffic goes through `workspace_client` so the
//! `caller_*` source fields are merged into the request body consistently and
//! the receiving engine can capture a `MessageOrigin::Workspace` from them.

pub mod workspace_client;
pub mod workspace_resolver;
