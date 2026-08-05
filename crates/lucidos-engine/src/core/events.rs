use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// One row of the `events` table, and the **wire shape** of every event the
/// workspace-facing read surface returns: `GET /api/v1/events/query`,
/// `GET /api/v1/events/:event_id/{context,tool-result}`, the `events` LLM
/// tool's `query` action, and `lucidos events query`.
///
/// `ThreadEvent` rows and `SystemEvent` rows (including workspace-emitted
/// `DomainEvent`s) share this one table and this one shape. The `aggregate` /
/// `aggregate_id` columns exist on the table but are deliberately NOT selected
/// by `EventStore::fetch_events`, so they never reach the wire. See
/// `system-knowhow/thread-events.md` § "One table, two enums".
///
/// The JS SDK mirrors this as `LucidosEvent` in
/// `packages/lucidos-sdk/src/events.ts` (documented in
/// `system-knowhow/js-sdk.md` § `lucidos.events`). Adding, removing or
/// renaming a field here means updating both, and
/// `serialized_key_set_is_the_documented_sdk_wire_shape` fails until you do.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EventRow {
    pub id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created: DateTime<Utc>,
    /// The thread this row belongs to. `EventBus::persist` sets it exactly for
    /// `aggregate = 'thread'` rows, which is every `ThreadEvent` and no
    /// `SystemEvent` the engine persists today (domain events carry
    /// `aggregate = 'domain'`). NULL there means the key is ABSENT from the
    /// JSON rather than null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    /// Monotonic insertion order (`events.sequence`, a NOT NULL bigserial).
    /// Always present on a row read back from the database; `None` only on an
    /// in-memory [`EventRow::new`] that was never persisted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i64>,
}

impl EventRow {
    pub fn new(event_type: &str, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            payload,
            created: Utc::now(),
            thread_id: None,
            sequence: None,
        }
    }
}

/// Default mime for generated images stored inline in event payloads.
/// User images live in the blob store and use their sniffed mime instead.
pub const GENERATED_IMAGE_MIME: &str = "image/jpeg";

/// Metadata-only entry yielded by `walk_thread_images_meta`. Cheap — no
/// blob bytes are read.
pub struct ThreadImageMeta {
    pub index: usize,
    pub source: &'static str,
    pub mime_type: String,
}

/// A single image found while walking thread events. The `base64` field is
/// only populated by the bytes-reading walker (`walk_thread_images`); the
/// `walk_thread_images_meta` API endpoint uses `ThreadImageMeta` instead.
pub struct ThreadImage {
    pub index: usize,
    pub source: &'static str,
    pub base64: String,
    pub mime_type: String,
}

pub trait HasEventPayload {
    fn event_type(&self) -> &str;
    fn payload(&self) -> &serde_json::Value;
    /// Mutable access, so a read path can rewrite an oversized payload before
    /// serving it without caring which of the two row types it holds.
    fn payload_mut(&mut self) -> &mut serde_json::Value;
}

impl HasEventPayload for EventRow {
    fn event_type(&self) -> &str {
        &self.event_type
    }
    fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
    fn payload_mut(&mut self) -> &mut serde_json::Value {
        &mut self.payload
    }
}

/// Iterate (index, source, mime, blob_hash_or_inline_base64) for every
/// image in the thread. The MessageReceived branch yields the hash; the
/// generated branches yield the inline base64. Shared between the two
/// public walkers below so the indexing rule lives in one place.
fn walk_thread_image_refs<E: HasEventPayload>(
    events: &[E],
) -> impl Iterator<Item = (&'static str, ImageRef<'_>)> + '_ {
    events.iter().flat_map(|event| {
        let payload = event.payload();
        match event.event_type() {
            "MessageReceived" => payload
                .get("user_image_hashes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|h| ("user", ImageRef::BlobHash(h)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            "ToolResult" | "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => payload
                .get("images")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|b64| ("generated", ImageRef::InlineBase64(b64)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    })
}

enum ImageRef<'a> {
    BlobHash(&'a str),
    InlineBase64(&'a str),
}

/// Metadata-only walk for endpoints that just need (index, source, mime).
/// One `metadata` syscall per user image, zero reads. Missing blobs still
/// occupy an index — the index matches `walk_thread_images` and the
/// thread:N numbering used by `chat/process.rs::msg_image_starts`. The
/// API serves a 404 for the missing entry.
pub fn walk_thread_images_meta<E: HasEventPayload>(
    workspace: &std::path::Path,
    events: &[E],
) -> Vec<ThreadImageMeta> {
    walk_thread_image_refs(events)
        .enumerate()
        .map(|(i, (source, image_ref))| {
            let mime_type = match image_ref {
                ImageRef::BlobHash(hash) => crate::core::blobs::resolve_blob(workspace, hash)
                    .map(|b| b.mime)
                    .unwrap_or_else(|| GENERATED_IMAGE_MIME.to_string()),
                ImageRef::InlineBase64(_) => GENERATED_IMAGE_MIME.to_string(),
            };
            ThreadImageMeta {
                index: i + 1,
                source,
                mime_type,
            }
        })
        .collect()
}

/// Bytes-loading walk for tool resolution and thread-image GET. Missing
/// blobs yield an entry with empty base64 + the placeholder mime so the
/// index matches `walk_thread_images_meta` and `msg_image_starts`. A
/// callable `thread:N` whose blob is gone reaches the LLM as zero bytes
/// rather than collapsing the numbering and pointing at the wrong image.
pub fn walk_thread_images<E: HasEventPayload>(
    workspace: &std::path::Path,
    events: &[E],
) -> Vec<ThreadImage> {
    walk_thread_image_refs(events)
        .enumerate()
        .map(|(i, (source, image_ref))| {
            let (base64, mime_type) = match image_ref {
                ImageRef::BlobHash(hash) => {
                    crate::core::blobs::read_blob_as_base64(workspace, hash).unwrap_or_else(|| {
                        crate::log!(
                        "[walk_thread_images] Blob {} missing or unreadable, yielding empty entry",
                        hash
                    );
                        (String::new(), GENERATED_IMAGE_MIME.to_string())
                    })
                }
                ImageRef::InlineBase64(b64) => (b64.to_string(), GENERATED_IMAGE_MIME.to_string()),
            };
            ThreadImage {
                index: i + 1,
                source,
                base64,
                mime_type,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let payload = serde_json::json!({"path": "test.txt"});
        let event = EventRow::new("ArtifactCreated", payload.clone());

        assert_eq!(event.event_type, "ArtifactCreated");
        assert_eq!(event.payload, payload);
    }

    #[test]
    fn test_event_serialization() {
        let event = EventRow::new("Test", serde_json::json!({}));
        let json = serde_json::to_string(&event).unwrap();
        let parsed: EventRow = serde_json::from_str(&json).unwrap();

        assert_eq!(event.id, parsed.id);
    }

    /// Drift guard for the workspace-facing event read surface.
    ///
    /// `EventRow` IS the JSON `/api/v1/events/query` returns, and the JS SDK
    /// hand-declares that shape as `LucidosEvent` in
    /// `packages/lucidos-sdk/src/events.ts` (mirrored in
    /// `system-knowhow/js-sdk.md` § `lucidos.events`). Nothing else held the
    /// two together, and they drifted: the SDK declared `aggregate?` /
    /// `aggregate_id?`, which this endpoint never returns (the columns exist
    /// on the table but `EventStore::fetch_events` does not select them), and
    /// omitted `thread_id` / `sequence`, which it always does. An app reading
    /// `e.thread_id` worked at runtime and failed `tsc`, which is what made
    /// "an app cannot see a child thread's outcome" look true.
    ///
    /// Both shapes are asserted because both reach real clients: a
    /// `ThreadEvent` row carries `thread_id`, a `SystemEvent` / domain-event
    /// row has a NULL `thread_id` column and so drops the key entirely.
    #[test]
    fn serialized_key_set_is_the_documented_sdk_wire_shape() {
        fn keys(row: &EventRow) -> Vec<String> {
            let mut k: Vec<String> = serde_json::to_value(row)
                .expect("EventRow serializes")
                .as_object()
                .expect("EventRow serializes to a JSON object")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        }

        // A thread-scoped row, as returned for e.g. ChildThreadCompleted.
        let thread_row = EventRow {
            thread_id: Some(Uuid::new_v4()),
            sequence: Some(42),
            ..EventRow::new("ChildThreadCompleted", serde_json::json!({}))
        };
        assert_eq!(
            keys(&thread_row),
            [
                "created",
                "event_type",
                "id",
                "payload",
                "sequence",
                "thread_id"
            ],
            "wire shape of a thread-scoped event row changed. Update \
             `LucidosEvent` in packages/lucidos-sdk/src/events.ts and the Types \
             block in system-knowhow/js-sdk.md § lucidos.events in the same change.",
        );

        // A workspace-emitted domain event: aggregate = 'domain', so the
        // thread_id column is NULL and `skip_serializing_if` drops the key.
        let domain_row = EventRow {
            sequence: Some(7),
            ..EventRow::new("HabitCompleted", serde_json::json!({"summary": "x"}))
        };
        assert_eq!(
            keys(&domain_row),
            ["created", "event_type", "id", "payload", "sequence"],
            "a domain-event row must omit `thread_id` (absent, not null) so \
             `thread_id?` stays the honest declaration on the SDK side.",
        );
    }
}
