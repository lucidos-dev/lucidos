//! Closed workspace operations and operation-bound attestations.
//!
//! These contracts confer no authority by themselves. The source engine must
//! verify its agent origin; gateways must verify the current directional pairing
//! and any required return access grant before dispatch. A spawn additionally
//! carries the source engine's existing spawn-gate verdict.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Read and spawn signatures use different claim kinds, even with equal fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Read,
    Spawn,
}

/// The complete operation. No caller can supply an HTTP method or path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkspaceOperation {
    Read {
        request: ReadRequest,
    },
    /// The existing chat/stream body. Source and target still validate its
    /// schema and enforce agent mode, provenance and the source spawn verdict.
    Spawn {
        body: Map<String, Value>,
    },
}

impl WorkspaceOperation {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::Read { .. } => OperationKind::Read,
            Self::Spawn { .. } => OperationKind::Spawn,
        }
    }

    /// Hash canonical JSON, including the operation kind and every argument.
    /// Object key ordering is insignificant, including under serde_json's
    /// `preserve_order` feature; array order and scalar values are significant.
    pub fn digest(&self) -> Result<String, &'static str> {
        if let Self::Read { request } = self {
            request.path()?;
        }
        let value = serde_json::to_value(self).map_err(|_| "Invalid operation arguments.")?;
        let mut bytes = Vec::new();
        canonical_json(&value, &mut bytes)?;
        Ok(crate::hex(&Sha256::digest(bytes)))
    }
}

fn canonical_json(value: &Value, out: &mut Vec<u8>) -> Result<(), &'static str> {
    match value {
        Value::Object(object) => {
            out.push(b'{');
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
            for (i, (key, value)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key).map_err(|_| "Invalid argument key.")?;
                out.push(b':');
                canonical_json(value, out)?;
            }
            out.push(b'}');
        }
        Value::Array(array) => {
            out.push(b'[');
            for (i, value) in array.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                canonical_json(value, out)?;
            }
            out.push(b']');
        }
        _ => serde_json::to_writer(out, value).map_err(|_| "Invalid argument value.")?,
    }
    Ok(())
}

/// Every allowed read names a single existing API handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadKind {
    ThreadSummary,
    ThreadMessages,
    ThreadLiveness,
    Threads,
    ThreadsCount,
    Events,
    EventsCount,
    IntelligenceLadder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub kind: ReadKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Only the operation's explicit query keys are accepted. Values are
    /// passed as encoded query pairs, never appended to a URI as raw text.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
}

impl ReadRequest {
    /// Validate the closed operation and construct its path. A thread id
    /// cannot inject another path, encoded path, query, fragment or authority.
    /// The existing handler remains responsible for query value validation.
    pub fn path(&self) -> Result<String, &'static str> {
        let allowed: &[&str] = match self.kind {
            ReadKind::ThreadSummary | ReadKind::ThreadMessages | ReadKind::ThreadLiveness => &[],
            ReadKind::Threads | ReadKind::ThreadsCount => &[
                "active",
                "status",
                "source",
                "limit",
                "parent",
                "has_draft",
                "change_state",
            ],
            ReadKind::Events => &[
                "type",
                "since",
                "until",
                "before_event_id",
                "after_event_id",
                "limit",
                "byte_limit",
                "event_id",
            ],
            ReadKind::EventsCount => &["type", "since", "until"],
            ReadKind::IntelligenceLadder => &["surface"],
        };
        if self
            .query
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            return Err("This query parameter is not allowed for this read.");
        }
        if self
            .thread_id
            .as_deref()
            .is_some_and(|id| !crate::is_plain_target(id))
        {
            return Err("The thread id must be a plain path segment.");
        }
        match self.kind {
            ReadKind::ThreadSummary | ReadKind::ThreadMessages | ReadKind::ThreadLiveness => {
                let thread = self
                    .thread_id
                    .as_deref()
                    .ok_or("This read needs a thread id.")?;
                let suffix = match self.kind {
                    ReadKind::ThreadMessages => "/messages",
                    ReadKind::ThreadLiveness => "/liveness",
                    _ => "",
                };
                Ok(format!("/api/v1/threads/{thread}{suffix}"))
            }
            ReadKind::Threads | ReadKind::ThreadsCount if self.thread_id.is_some() => {
                Err("Thread listing uses filters, not a thread id.")
            }
            ReadKind::Threads => Ok("/api/v1/threads/list".into()),
            ReadKind::ThreadsCount => Ok("/api/v1/threads/count".into()),
            ReadKind::Events => Ok("/api/v1/events/query".into()),
            ReadKind::EventsCount => Ok("/api/v1/events/count".into()),
            ReadKind::IntelligenceLadder => {
                if self.thread_id.is_some() && self.query.contains_key("surface") {
                    return Err("Select a ladder by thread or surface, not both.");
                }
                Ok("/api/v1/intelligence-ladder".into())
            }
        }
    }

    pub fn query_pairs(&self) -> Result<Vec<(String, String)>, &'static str> {
        self.path()?;
        let mut query: Vec<_> = self
            .query
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if matches!(
            self.kind,
            ReadKind::Events | ReadKind::EventsCount | ReadKind::IntelligenceLadder
        ) {
            if let Some(thread) = &self.thread_id {
                query.push(("thread_id".into(), thread.clone()));
            }
        }
        Ok(query)
    }
}

/// An engine/gateway's attestation of an operation. This is not a device
/// identity or a grant; receivers must authenticate their own current hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceOperationClaim<'a> {
    pub kind: OperationKind,
    pub routing_gateway: &'a str,
    pub target_workspace: &'a str,
    pub caller_gateway: &'a str,
    pub caller_workspace: &'a str,
    pub caller_thread_id: &'a str,
    pub arguments_digest: &'a str,
    /// Empty for reads; the source engine's unchanged spawn verdict for spawns.
    pub verdict: &'a str,
}

impl WorkspaceOperationClaim<'_> {
    pub(crate) fn signed_fields(&self) -> (&'static str, Vec<String>) {
        let kind = match self.kind {
            OperationKind::Read => "workspace-read",
            OperationKind::Spawn => "workspace-spawn",
        };
        (
            kind,
            [
                self.routing_gateway,
                self.target_workspace,
                self.caller_gateway,
                self.caller_workspace,
                self.caller_thread_id,
                self.arguments_digest,
                self.verdict,
            ]
            .map(str::to_owned)
            .to_vec(),
        )
    }
}
