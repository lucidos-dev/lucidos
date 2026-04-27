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

/// A single image found while walking thread events.
pub struct ThreadImage<'a> {
    /// 1-based sequential index across the thread.
    pub index: usize,
    /// "user" (from MessageReceived) or "generated" (from ToolResult/ResponseGenerated/ResponseCanceled/ResponseAborted).
    pub source: &'static str,
    /// Base64 image data. For user images this is from `images[].base64`; for generated it's a plain string.
    pub base64: &'a str,
    /// MIME type (from user images) or default "image/jpeg" for generated.
    pub mime_type: &'a str,
}

/// Trait for event types that have event_type and payload fields.
/// Implemented by both `EventRow` and `ThreadEventRow`.
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

/// Walk thread events and yield all images in sequential order.
/// Used by API image endpoints and tool image resolution.
pub fn walk_thread_images<E: HasEventPayload>(events: &[E]) -> Vec<ThreadImage<'_>> {
    let mut images = Vec::new();
    let mut index: usize = 0;

    for event in events {
        match event.event_type() {
            "MessageReceived" => {
                if let Some(imgs) = event.payload().get("images").and_then(|v| v.as_array()) {
                    for img in imgs {
                        index += 1;
                        let b64 = img.get("base64").and_then(|v| v.as_str()).unwrap_or("");
                        let mime = img
                            .get("mime_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/jpeg");
                        images.push(ThreadImage {
                            index,
                            source: "user",
                            base64: b64,
                            mime_type: mime,
                        });
                    }
                }
            }
            "ToolResult" | "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => {
                if let Some(imgs) = event.payload().get("images").and_then(|v| v.as_array()) {
                    for img_val in imgs {
                        if let Some(b64) = img_val.as_str() {
                            index += 1;
                            images.push(ThreadImage {
                                index,
                                source: "generated",
                                base64: b64,
                                mime_type: "image/jpeg",
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    images
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
