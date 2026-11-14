//! Backup capture has no provider or encryption dependency. Its scratch
//! directory belongs to the caller and remains alive through upload.
use super::*;

pub(super) fn capture_workspace_archive(
    workspace: &Path,
    database_url: &str,
    temp_path: &Path,
    pending_dir: Option<&Path>,
    ws_bytes: u64,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<PathBuf, BoxError> {
    // Load data/.backupignore once for this run — shared by the tar walk
    // (the workspace can have tens of thousands of entries; never
    // re-parse per file).
    let ignore = BackupIgnore::load(workspace);

    // Phase weights from the size already computed for preflight.
    progress("estimating", 0, 100);
    let (dump_end, _, _) = estimate_weights(ws_bytes);

    // Phase 1: pg_dump (0% → dump_end%)
    crate::log!("[Backup] Phase 1/4: pg_dump");
    progress("dumping_db", 1, 100);
    let dump_path = temp_path.join("lucidos_backup.dump");
    pg_dump(database_url, &dump_path)?;
    // The dump holds sealed secrets, so the archive carries their key
    // beside it, inside the same encryption (ADR 0509).
    let secret_key_path = temp_path.join(SECRET_STORE_KEY_ENTRY);
    lucidos_local_token::write_owner_only(
        &secret_key_path,
        &crate::core::secret_store::key()?.to_base64(),
    )?;

    // Phase 2: tar + zstd compress (dump_end% → compress_end%)
    crate::log!("[Backup] Phase 2/4: tar + zstd compress");
    progress("compressing", dump_end, 100);
    let compressed_path = temp_path.join("backup.tar.zst");
    tar_and_compress(
        workspace,
        &dump_path,
        &secret_key_path,
        pending_dir,
        &compressed_path,
        &ignore,
    )?;
    Ok(compressed_path)
}

/// Metadata read in the exact PostgreSQL snapshot imported by pg_dump.
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseCapture {
    pub db_captured_at: DateTime<Utc>,
    pub schema_version: i64,
    pub table_rows: std::collections::BTreeMap<String, i64>,
    pub secret_key_ids: std::collections::BTreeSet<String>,
}

/// Export a repeatable-read snapshot and keep its transaction alive until the
/// dump subprocess finishes. The pool must use the engine role.
pub async fn capture_database(
    pool: &PgPool,
    database_url: &str,
    output: &Path,
) -> Result<DatabaseCapture, BoxError> {
    use crate::core::secret_store::{sealed_key_id, Shape, SECRET_COLUMNS};
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let snapshot: String = sqlx::query_scalar("SELECT pg_export_snapshot()")
        .fetch_one(&mut *tx)
        .await?;
    let db_captured_at = sqlx::query_scalar("SELECT transaction_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut table_rows = std::collections::BTreeMap::new();
    for table in &tables {
        let count = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM public.{}",
            quote_identifier(table)
        ))
        .fetch_one(&mut *tx)
        .await?;
        table_rows.insert(table.clone(), count);
    }
    let schema_version = sqlx::query_scalar(
        "SELECT COALESCE(max(version), 0) FROM public._sqlx_migrations WHERE success",
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut secret_key_ids = std::collections::BTreeSet::new();
    for spec in SECRET_COLUMNS {
        let table = quote_identifier(spec.table);
        let column = quote_identifier(spec.column);
        let query = match spec.shape {
            Shape::Text => {
                format!("SELECT {column} FROM public.{table} WHERE {column} IS NOT NULL")
            }
            Shape::JsonValues => {
                format!("SELECT value FROM public.{table}, jsonb_each_text({column})")
            }
        };
        // Values stay inside the engine and only their key ids leave this loop.
        let values: Vec<String> = sqlx::query_scalar(&query).fetch_all(&mut *tx).await?;
        for value in values {
            if let Some(id) = sealed_key_id(&value) {
                secret_key_ids.insert(id.to_owned());
            }
        }
    }
    let database_url = database_url.to_owned();
    let output = output.to_path_buf();
    tokio::task::spawn_blocking(move || {
        super::pg_dump_with_snapshot(&database_url, &output, Some(&snapshot))
    })
    .await??;
    tx.commit().await?;
    Ok(DatabaseCapture {
        db_captured_at,
        schema_version,
        table_rows,
        secret_key_ids,
    })
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// An index describes the bytes actually archived, including padding required
/// when a live file shrinks during capture.
#[derive(Debug, Serialize, Deserialize)]
pub struct FileIndexEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mode: u32,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileCapture {
    pub files_started_at: DateTime<Utc>,
    pub files_finished_at: DateTime<Utc>,
    pub files_changed_during_read: u64,
    pub count: u64,
    pub bytes: u64,
    pub tree_sha256: String,
}

/// Capture backup-scope workspace files only. Neither a database dump nor a
/// key-store key is added to this archive. Output belongs outside the workspace.
pub fn capture_files(workspace: &Path, output: &Path) -> Result<FileCapture, BoxError> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    if output
        .canonicalize()?
        .starts_with(workspace.canonicalize()?)
    {
        return Err("snapshot output must be outside the workspace".into());
    }
    let files_started_at = Utc::now();
    let ignore = BackupIgnore::load(workspace);
    let archive_path = output.join("files.tar.zst");
    let encoder = zstd::Encoder::new(std::fs::File::create(&archive_path)?, 3)?;
    let mut builder = tar::Builder::new(encoder);
    let mut files_changed_during_read = 0;
    let mut paths = walkdir(workspace)?;
    paths.sort();
    for path in paths {
        let relative = path.strip_prefix(workspace)?;
        if is_excluded_workspace_path(relative, &ignore) {
            continue;
        }
        let Some(file) = skip_if_vanished(&path, std::fs::File::open(&path))? else {
            continue;
        };
        let before = file.metadata()?;
        if !before.is_file() {
            continue;
        }
        let mut header = header_for_file(&before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            header.set_mode(before.permissions().mode() & 0o7777);
            header.set_cksum();
        }
        let padded = append_exact(&mut builder, &mut header, relative, &file)?;
        if copy_drift(&before, &file.metadata()?, padded).is_some() {
            files_changed_during_read += 1;
        }
    }
    builder.into_inner()?.finish()?;
    // Index the archived bytes, rather than reading the live files twice.
    let decoder = zstd::Decoder::new(std::fs::File::open(&archive_path)?)?;
    let mut archive = tar::Archive::new(decoder);
    let mut index = zstd::Encoder::new(
        std::fs::File::create(output.join("files.index.jsonl.zst"))?,
        3,
    )?;
    let mut tree_hash = Sha256::new();
    let mut count = 0;
    let mut bytes = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let len = entry.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            hash.update(&buffer[..len]);
        }
        let record = FileIndexEntry {
            path: entry.path()?.into_owned(),
            size: entry.size(),
            mode: entry.header().mode()?,
            sha256: format!("{:x}", hash.finalize()),
        };
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        tree_hash.update(&line);
        index.write_all(&line)?;
        count += 1;
        bytes += record.size;
    }
    index.finish()?;
    Ok(FileCapture {
        files_started_at,
        files_finished_at: Utc::now(),
        files_changed_during_read,
        count,
        bytes,
        tree_sha256: format!("{:x}", tree_hash.finalize()),
    })
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
