use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EventRow {
    pub id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
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
}

impl HasEventPayload for EventRow {
    fn event_type(&self) -> &str {
        &self.event_type
    }
    fn payload(&self) -> &serde_json::Value {
        &self.payload
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
                ImageRef::BlobHash(hash) => crate::core::blobs::read_blob_as_base64(
                    workspace, hash,
                )
                .unwrap_or_else(|| {
                    crate::log!(
                        "[walk_thread_images] Blob {} missing or unreadable, yielding empty entry",
                        hash
                    );
                    (String::new(), GENERATED_IMAGE_MIME.to_string())
                }),
                ImageRef::InlineBase64(b64) => {
                    (b64.to_string(), GENERATED_IMAGE_MIME.to_string())
                }
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
}
