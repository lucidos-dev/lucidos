//! HTTP helpers for engine-initiated outbound calls.
//!
//! All workspace-to-workspace traffic goes through `workspace_client` so the
//! `X-Cognos-*` source headers are stamped consistently and the receiving
//! engine can capture a `MessageOrigin::Workspace` from them.

pub mod workspace_client;
