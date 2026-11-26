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

pub const GENERATED_IMAGE_MIME: &str = "image/jpeg";

pub struct ThreadImageMeta {
    pub index: usize,
    pub source: &'static str,
    pub mime_type: String,
    pub handle: String,
}

pub struct ThreadImage {
    pub index: usize,
    pub source: &'static str,
    pub base64: String,
    pub mime_type: String,
    pub handle: String,
}

const IMAGE_HANDLE_HEX: usize = 16;

/// INVARIANT: The stable, position-free address of a thread image: `img-<16 hex>`.
///
/// `thread:N` renumbers whenever an earlier image appears, so a number the
/// agent noted in one turn can name a different picture in the next. This is
/// content-addressed, so it survives renumbering, restarts and re-reads.
///
/// Derived from what the payload already carries: the sha256 the blob store
/// keyed a user upload by, or a hash of a generated image's base64. Neither
/// reads a file, which is what keeps `walk_thread_images_meta` free of blob
/// reads. Two byte-identical images therefore share one address and resolve
/// to the same picture, which is the right answer.
pub fn image_handle(image_ref: ImageRef<'_>) -> String {
    let hex = match image_ref {
        ImageRef::BlobHash(hash) => hash.to_string(),
        ImageRef::InlineBase64(b64) => crate::core::blobs::compute_hash(b64.as_bytes()),
    };
    let short: String = hex.chars().take(IMAGE_HANDLE_HEX).collect();
    format!("img-{short}")
}

pub trait HasEventPayload {
    fn event_type(&self) -> &str;
    fn payload(&self) -> &serde_json::Value;
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

/// INVARIANT: The thread images one event adds, in order, with their source.
///
/// The single rule for what counts as a thread image. The walkers below and
/// the history's message builder both read it, so `thread:N` names the same
/// picture in the history as in the resolver.
///
/// A user upload is a blob hash on `MessageReceived`. A generated image is
/// inline base64 on its `ToolResult`. A response's own `images` field does
/// not count: it lists browser screenshot artifact paths, not image bytes.
pub fn thread_image_refs_of<'a>(
    event_type: &str,
    payload: &'a serde_json::Value,
) -> Vec<(&'static str, ImageRef<'a>)> {
    let (field, source, image_ref): (_, _, fn(&'a str) -> ImageRef<'a>) = match event_type {
        "MessageReceived" => ("user_image_hashes", "user", ImageRef::BlobHash),
        "ToolResult" => ("images", "generated", ImageRef::InlineBase64),
        _ => return Vec::new(),
    };
    payload
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| (source, image_ref(s)))
                .collect()
        })
        .unwrap_or_default()
}

fn walk_thread_image_refs<E: HasEventPayload>(
    events: &[E],
) -> impl Iterator<Item = (&'static str, ImageRef<'_>)> + '_ {
    events
        .iter()
        .flat_map(|event| thread_image_refs_of(event.event_type(), event.payload()))
}

pub fn walk_thread_image_handles<E: HasEventPayload>(events: &[E]) -> Vec<String> {
    walk_thread_image_refs(events)
        .map(|(_, image_ref)| image_handle(image_ref))
        .collect()
}

#[derive(Clone, Copy)]
pub enum ImageRef<'a> {
    BlobHash(&'a str),
    InlineBase64(&'a str),
}

/// INVARIANT: Metadata-only walk for endpoints that just need (index, source, mime).
/// One `metadata` syscall per user image, zero reads. Missing blobs still
/// occupy an index — the index matches `walk_thread_images` and
/// `walk_thread_image_handles`. The API serves a 404 for the missing entry.
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
                handle: image_handle(image_ref),
            }
        })
        .collect()
}

/// INVARIANT: Bytes-loading walk for tool resolution and thread-image GET. Missing
/// blobs yield an entry with empty base64 + the placeholder mime so the
/// index matches `walk_thread_images_meta` and `walk_thread_image_handles`. A
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
                        crate::log_legacy!(
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
                handle: image_handle(image_ref),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_responses_screenshot_paths_are_not_thread_images() {
        let events = vec![
            EventRow::new(
                "MessageReceived",
                serde_json::json!({"text": "a", "user_image_hashes": ["aa11"]}),
            ),
            EventRow::new(
                "ResponseGenerated",
                serde_json::json!({"text": "b", "images": ["screenshots/page_2026.png"]}),
            ),
            EventRow::new(
                "MessageReceived",
                serde_json::json!({"text": "c", "user_image_hashes": ["bb22"]}),
            ),
        ];
        assert_eq!(
            walk_thread_image_handles(&events),
            vec![
                image_handle(ImageRef::BlobHash("aa11")),
                image_handle(ImageRef::BlobHash("bb22")),
            ]
        );
    }

    #[test]
    fn a_generated_image_takes_its_slot_in_the_walk() {
        let events = vec![
            EventRow::new(
                "MessageReceived",
                serde_json::json!({"text": "a", "user_image_hashes": ["aa11"]}),
            ),
            EventRow::new(
                "ToolResult",
                serde_json::json!({"name": "generate_image", "images": ["R0lGODlh"]}),
            ),
            EventRow::new(
                "MessageReceived",
                serde_json::json!({"text": "c", "user_image_hashes": ["bb22"]}),
            ),
        ];
        assert_eq!(
            walk_thread_image_handles(&events),
            vec![
                image_handle(ImageRef::BlobHash("aa11")),
                image_handle(ImageRef::InlineBase64("R0lGODlh")),
                image_handle(ImageRef::BlobHash("bb22")),
            ]
        );
    }

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
             block in system-knowhow/js-sdk/lucidos-media.md § lucidos.events in the same change.",
        );

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
