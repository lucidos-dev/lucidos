use base64::Engine as _;
use serde_json::{json, Value};
use std::path::Path;

use crate::core::blobs::{write_blob, BlobError};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PayloadMigration {
    pub hashes: Vec<String>,
    pub failed_decode: usize,
}

pub fn migrate_image_payload(
    payload: &mut Value,
    workspace: &Path,
    legacy_field: &str,
    new_field: &str,
) -> Result<Option<PayloadMigration>, BlobError> {
    let obj = match payload.as_object_mut() {
        Some(o) => o,
        None => return Ok(None),
    };
    let images = match obj.remove(legacy_field) {
        Some(Value::Array(a)) => a,
        Some(_) | None => return Ok(None),
    };
    if images.is_empty() {
        obj.insert(new_field.to_string(), json!([]));
        return Ok(Some(PayloadMigration::default()));
    }
    // INVARIANT: Defense in depth — when legacy_field == new_field (the compose
    // case), a previously-migrated row carries `Vec<String>` of hashes
    // here. We must NOT overwrite those with `[]`, which would silently
    // wipe the user's draft. Treat the row as already migrated and
    // restore the field intact.
    if images.iter().all(|v| v.is_string()) {
        obj.insert(new_field.to_string(), Value::Array(images));
        return Ok(None);
    }

    let mut result = PayloadMigration::default();
    for entry in images {
        let base64_str = entry
            .get("base64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bytes = match base64::engine::general_purpose::STANDARD.decode(base64_str) {
            Ok(b) => b,
            Err(_) => {
                result.failed_decode += 1;
                continue;
            }
        };
        match write_blob(workspace, &bytes) {
            Ok(blob) => result.hashes.push(blob.hash),
            Err(BlobError::UnsupportedMime(_)) => result.failed_decode += 1,
            Err(e @ BlobError::Io(_)) => return Err(e),
            // INVARIANT: write_blob never returns BadEncoding — that variant fires
            // only through write_blob_from_base64. We've already decoded.
            Err(BlobError::BadEncoding(_)) => unreachable!(),
        }
    }

    obj.insert(
        new_field.to_string(),
        Value::Array(result.hashes.iter().cloned().map(Value::String).collect()),
    );
    Ok(Some(result))
}

/// INVARIANT: Async orchestrator: walk every legacy event + draft row, migrate each in
/// place. Returns `(events_migrated, drafts_migrated)`.
///
/// Per-row, no big transaction: a crash mid-migration loses no data because
/// (a) blob writes are idempotent (content-addressed), (b) payload rewrites
/// are idempotent (the gate query stops matching once the legacy field is
/// removed). Re-running on a fresh / already-migrated workspace = no-op.
///
/// Synchronous-on-startup (called before HTTP bind) so readers never see a
/// mixed shape across the new and legacy fields.
pub async fn migrate_legacy_image_payloads(
    pool: &sqlx::PgPool,
    workspace: &Path,
) -> Result<(usize, usize), Box<dyn std::error::Error + Send + Sync>> {
    let events_v1 = migrate_legacy_event_images(pool, workspace, "user_images").await?;
    let events_v2 = migrate_legacy_event_images(pool, workspace, "images").await?;
    let drafts_migrated = migrate_legacy_compose_images(pool, workspace).await?;
    Ok((events_v1 + events_v2, drafts_migrated))
}

const MIGRATION_BATCH_SIZE: i64 = 50;

async fn migrate_legacy_event_images(
    pool: &sqlx::PgPool,
    workspace: &Path,
    legacy_field: &str,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut count = 0usize;
    loop {
        // INVARIANT: The gate query stops matching every row this iteration migrated,
        // so no OFFSET is needed — the next iteration sees only rows that
        // still have the legacy field. Re-runs and crash-resumes share the
        // same loop shape.
        //
        // The element-shape predicate (`->0->>'base64' IS NOT NULL`) is
        // load-bearing for the `images` field: that name is reused by other
        // event types (e.g. `ResponseGenerated.images` carries a
        // `Vec<String>` of memory hits) — the `event_type` filter narrows
        // to MessageReceived, and the base64 check guarantees we only
        // touch rows still in the legacy `[{base64, mime_type}, ...]` shape.
        let sql = format!(
            "SELECT id, payload FROM events \
             WHERE event_type = 'MessageReceived' \
               AND payload ? '{legacy_field}' \
               AND jsonb_typeof(payload->'{legacy_field}') = 'array' \
               AND jsonb_array_length(payload->'{legacy_field}') > 0 \
               AND payload->'{legacy_field}'->0->>'base64' IS NOT NULL \
             LIMIT $1",
        );
        let batch: Vec<(uuid::Uuid, Value)> = sqlx::query_as(&sql)
            .bind(MIGRATION_BATCH_SIZE)
            .fetch_all(pool)
            .await?;
        if batch.is_empty() {
            break;
        }
        for (id, mut payload) in batch {
            let migration =
                migrate_image_payload(&mut payload, workspace, legacy_field, "user_image_hashes")?;
            if let Some(m) = migration {
                if m.failed_decode > 0 {
                    crate::log_legacy!(
                        "[ImageMigration] event={id} dropped {} undecodable image(s)",
                        m.failed_decode
                    );
                }
                sqlx::query("UPDATE events SET payload = $1 WHERE id = $2")
                    .bind(&payload)
                    .bind(id)
                    .execute(pool)
                    .await?;
                count += 1;
            }
        }
    }
    if count > 0 {
        crate::log_legacy!(
            "[ImageMigration] migrated {count} MessageReceived event(s) from `{legacy_field}` → `user_image_hashes`"
        );
    }
    Ok(count)
}

async fn migrate_legacy_compose_images(
    pool: &sqlx::PgPool,
    workspace: &Path,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut count = 0usize;
    loop {
        let batch: Vec<(uuid::Uuid, Value)> = sqlx::query_as(
            "SELECT thread_id, compose_images FROM thread_summaries \
             WHERE compose_images IS NOT NULL \
               AND jsonb_array_length(compose_images) > 0 \
               AND compose_images->0->>'base64' IS NOT NULL \
             LIMIT $1",
        )
        .bind(MIGRATION_BATCH_SIZE)
        .fetch_all(pool)
        .await?;
        if batch.is_empty() {
            break;
        }
        for (thread_id, images) in batch {
            let mut wrapper = json!({ "compose_images": images });
            let migration = migrate_image_payload(
                &mut wrapper,
                workspace,
                "compose_images",
                "compose_image_hashes",
            )?;
            if let Some(m) = migration {
                if m.failed_decode > 0 {
                    crate::log_legacy!(
                        "[ImageMigration] thread={thread_id} dropped {} undecodable draft image(s)",
                        m.failed_decode
                    );
                }
                let new_array = wrapper
                    .get("compose_image_hashes")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                sqlx::query("UPDATE thread_summaries SET compose_images = $1 WHERE thread_id = $2")
                    .bind(&new_array)
                    .bind(thread_id)
                    .execute(pool)
                    .await?;
                count += 1;
            }
        }
    }
    if count > 0 {
        crate::log_legacy!("[ImageMigration] migrated {count} compose draft(s)");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes() -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00,
        ]
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn migrate_image_payload_writes_blob_and_replaces_field_with_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({
            "text": "hi",
            "user_images": [{ "base64": b64(&png_bytes()), "mime_type": "image/png" }]
        });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap()
                .expect("migration must report Some when legacy field present");
        assert_eq!(result.hashes.len(), 1);
        assert_eq!(result.failed_decode, 0);
        assert!(payload.get("user_images").is_none(), "legacy field removed");
        let hashes = payload["user_image_hashes"].as_array().unwrap();
        assert_eq!(hashes.len(), 1);
        let hash = hashes[0].as_str().unwrap();
        let blob_path = dir
            .path()
            .join("data/blobs")
            .join(&hash[..2])
            .join(format!("{hash}.png"));
        assert_eq!(std::fs::read(&blob_path).unwrap(), png_bytes());
    }

    #[test]
    fn migrate_image_payload_returns_none_when_legacy_field_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({ "text": "no images here" });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap();
        assert_eq!(result, None);
        assert_eq!(payload, json!({ "text": "no images here" }));
    }

    #[test]
    fn migrate_image_payload_idempotent_on_already_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({ "text": "hi", "user_image_hashes": ["abc123"] });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap();
        assert_eq!(result, None);
        assert_eq!(payload["user_image_hashes"], json!(["abc123"]));
    }

    #[test]
    fn migrate_image_payload_preserves_already_migrated_when_field_name_shared() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({
            "compose_images": ["abcd1234", "ef567890"]
        });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "compose_images", "compose_images")
                .unwrap();
        assert_eq!(
            result, None,
            "must report None, not a destructive migration"
        );
        assert_eq!(
            payload["compose_images"],
            json!(["abcd1234", "ef567890"]),
            "already-migrated hash array must survive intact"
        );
    }

    #[test]
    fn migrate_image_payload_drops_bad_base64_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({
            "text": "two images",
            "user_images": [
                { "base64": b64(&png_bytes()), "mime_type": "image/png" },
                { "base64": "not!valid!base64", "mime_type": "image/png" }
            ]
        });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap()
                .unwrap();
        assert_eq!(result.hashes.len(), 1);
        assert_eq!(result.failed_decode, 1);
        assert_eq!(payload["user_image_hashes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn migrate_image_payload_drops_non_image_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({
            "text": "bogus mime",
            "user_images": [
                { "base64": b64(b"not an image"), "mime_type": "image/png" }
            ]
        });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap()
                .unwrap();
        assert_eq!(result.hashes.len(), 0);
        assert_eq!(result.failed_decode, 1);
        assert!(payload.get("user_images").is_none());
        assert_eq!(payload["user_image_hashes"], json!([]));
    }

    #[test]
    fn migrate_image_payload_handles_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({ "text": "empty", "user_images": [] });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap()
                .unwrap();
        assert_eq!(result.hashes.len(), 0);
        assert_eq!(result.failed_decode, 0);
        assert!(payload.get("user_images").is_none());
        assert_eq!(payload["user_image_hashes"], json!([]));
    }

    #[test]
    fn migrate_image_payload_dedup_on_disk_but_keeps_positional_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = json!({
            "user_images": [
                { "base64": b64(&png_bytes()), "mime_type": "image/png" },
                { "base64": b64(&png_bytes()), "mime_type": "image/png" }
            ]
        });
        let result =
            migrate_image_payload(&mut payload, dir.path(), "user_images", "user_image_hashes")
                .unwrap()
                .unwrap();
        assert_eq!(result.hashes.len(), 2);
        assert_eq!(result.hashes[0], result.hashes[1], "same bytes = same hash");
    }
}
