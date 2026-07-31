//! One-shot migration of legacy base64 image payloads to content-addressed
//! blobs.
//!
//! Two carriers in the legacy schema:
//!   - `events.payload->'images'` (current) and `events.payload->'user_images'`
//!     (older, exists only in test fixtures) on `MessageReceived` rows —
//!     historical sends, immutable in spirit but encoding-mutable.
//!   - `thread_summaries.compose_images` — in-progress drafts.
//!
//! Each entry is `{base64, mime_type}`. After migration the carriers become
//! `user_image_hashes: [hash, ...]` and `compose_image_hashes: [hash, ...]`,
//! and the bytes live exactly once under `data/blobs/<hh>/<hash>.<ext>`.
//!
//! See `docs/plans/2026-05-07-image-blob-store-design.md` § Section 2.

use base64::Engine as _;
use serde_json::{json, Value};
use std::path::Path;

use crate::core::blobs::{write_blob, BlobError};

/// Result of migrating one payload's image array. `hashes` is the list of
/// hashes that survived the migration (in original order, with failures
/// dropped); `failed_decode` counts images dropped due to bad base64 or
/// unsupported mime — caller decides whether to log per-image.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PayloadMigration {
    pub hashes: Vec<String>,
    pub failed_decode: usize,
}

/// Replace a legacy base64-image array field on `payload` with a hash-array
/// field; write the bytes to the workspace blob store.
///
/// Reads `payload[legacy_field]` (expected: `[{base64, mime_type}, ...]`)
/// and replaces it with `payload[new_field] = ["<hash>", ...]`.
///
/// Returns `Ok(None)` when the legacy field is absent (nothing to migrate),
/// otherwise `Ok(Some(PayloadMigration))`. The legacy field is removed from
/// `payload` even when zero hashes survive — keeping the empty array would
/// re-trigger the migration gate on subsequent runs.
///
/// Bad base64 / unsupported mime → that one image is dropped from the
/// resulting hash array and `failed_decode` is incremented; caller logs.
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
        // Nothing to write but still mutate the field shape so the gate
        // query stops matching this payload on future startups.
        obj.insert(new_field.to_string(), json!([]));
        return Ok(Some(PayloadMigration::default()));
    }
    // Defense in depth — when legacy_field == new_field (the compose
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
        // Sniffed mime is the source of truth — write_blob re-sniffs
        // and returns UnsupportedMime for anything outside the image
        // allowlist. The legacy `mime_type` field on each entry was
        // user-supplied and intentionally ignored.
        match write_blob(workspace, &bytes) {
            Ok(blob) => result.hashes.push(blob.hash),
            Err(BlobError::UnsupportedMime) => result.failed_decode += 1,
            Err(e @ BlobError::Io(_)) => return Err(e),
            // write_blob never returns BadEncoding — that variant fires
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

/// Async orchestrator: walk every legacy event + draft row, migrate each in
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
    // Two legacy event-payload field names on MessageReceived: the
    // long-lived production field `images`, and an older `user_images`
    // shape that survives only in legacy test fixtures. Both carry the
    // same `[{base64, mime_type}, ...]` structure and migrate to the same
    // canonical destination.
    let events_v1 = migrate_legacy_event_images(pool, workspace, "user_images").await?;
    let events_v2 = migrate_legacy_event_images(pool, workspace, "images").await?;
    let drafts_migrated = migrate_legacy_compose_images(pool, workspace).await?;
    Ok((events_v1 + events_v2, drafts_migrated))
}

/// Page size for the migration row scan. Each row's `payload` carries
/// inline base64 — a 5 MB image inflates to ~7 MB JSONB. At 50 rows per
/// batch the peak in-memory footprint is ~350 MB, well under any laptop
/// budget; loading the full table at once would OOM workspaces with
/// thousands of legacy image events.
const MIGRATION_BATCH_SIZE: i64 = 50;

async fn migrate_legacy_event_images(
    pool: &sqlx::PgPool,
    workspace: &Path,
    legacy_field: &str,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut count = 0usize;
    loop {
        // The gate query stops matching every row this iteration migrated,
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
                    crate::log!(
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
        crate::log!(
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
        // The compose_images column is reused for both legacy
        // (`[{base64, mime_type}, ...]`) and new (`[hash, ...]`) shapes
        // because Phase 2 doesn't rename the column. Filter to the
        // legacy shape via the existence of `base64` on element 0 — new
        // string entries don't have that key and the gate stops
        // matching, so re-runs never overwrite a hash array with [].
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
            // Wrap the array in a payload object so `migrate_image_payload`
            // can use its mutate-in-place contract; we only care about the
            // resulting hash array on this side.
            let mut wrapper = json!({ "compose_images": images });
            let migration = migrate_image_payload(
                &mut wrapper,
                workspace,
                "compose_images",
                "compose_image_hashes",
            )?;
            if let Some(m) = migration {
                if m.failed_decode > 0 {
                    crate::log!(
                        "[ImageMigration] thread={thread_id} dropped {} undecodable draft image(s)",
                        m.failed_decode
                    );
                }
                // The compose_images JSONB column now stores the
                // post-migration shape (an array of hash strings).
                // Phase 3 renames the column for clarity, but the data
                // is already forward-compatible — `["abc...", ...]`
                // either way.
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
        crate::log!("[ImageMigration] migrated {count} compose draft(s)");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic PNG bytes the migrator decodes + sniffs + writes.
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
        // Blob bytes on disk match the original.
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

    /// Idempotency: re-running the migration on a payload that's already
    /// in the new shape returns `None` and changes nothing. This is what
    /// makes the "no marker flag" gate safe — re-runs are no-ops.
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

    /// Regression: when `legacy_field == new_field` (the compose case),
    /// a payload already carrying `Vec<String>` of hashes must be
    /// detected as already-migrated and left intact. Without the
    /// string-shape guard, the inner loop would treat each string as a
    /// missing-base64 entry, count them as failures, and OVERWRITE the
    /// hash array with `[]` — silently destroying the user's draft.
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

    /// Bad base64 (or unsupported mime bytes) for one image must NOT abort
    /// the migration of the rest of the payload — drop the bad one, count
    /// it, continue. A single corrupt image can't block a years-old
    /// workspace from booting on a new engine.
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

    /// Bytes that decode but aren't a recognized image type also get
    /// dropped (no garbage in the blob store). Same path as bad base64.
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
        // Field shape still flipped so the migration gate stops matching.
        assert!(payload.get("user_images").is_none());
        assert_eq!(payload["user_image_hashes"], json!([]));
    }

    /// Empty `user_images: []` — already-empty drafts get the field
    /// renamed (so the gate stops matching) but no blob is written.
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

    /// Two distinct images get two distinct hashes; same image twice
    /// dedupes on disk (write_blob is content-addressed) but produces
    /// two hash entries (per-payload identity is positional).
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
