//! Private, bounded diagnostic inbox. No bundle file is exposed to agents.

use crate::diagnostic_log::{query as log_query, writer::Record};
use lucidos_diagnostic_log::bundle::{Bundle, MAX_COMPRESSED_BYTES, MAX_EXPANDED_BYTES};
use sha2::{Digest, Sha256};
use std::io::Read;
use uuid::Uuid;

type Error = Box<dyn std::error::Error + Send + Sync>;

pub fn decode(bytes: &[u8]) -> Result<Bundle, Error> {
    decode_with_limits(bytes, MAX_COMPRESSED_BYTES, MAX_EXPANDED_BYTES)
}

fn decode_with_limits(bytes: &[u8], compressed: usize, expanded: usize) -> Result<Bundle, Error> {
    if bytes.is_empty() || bytes.len() > compressed {
        return Err("Diagnostic compressed size limit exceeded".into());
    }
    let mut json = Vec::new();
    let mut decoder = zstd::stream::read::Decoder::new(bytes)?;
    // The producer uses zstd's ordinary small window. Bound decoder memory as
    // well as output; a hostile frame must not request a multi-gigabyte window.
    decoder.window_log_max(23)?;
    decoder.take(expanded as u64 + 1).read_to_end(&mut json)?;
    if json.len() > expanded {
        return Err("Diagnostic expanded size limit exceeded".into());
    }
    let bundle: Bundle = serde_json::from_slice(&json)?;
    if bundle.manifest.format_version != 1 {
        return Err("Unsupported diagnostic bundle version".into());
    }
    let mut previous = None;
    for line in &bundle.timeline {
        let ts = chrono::DateTime::parse_from_rfc3339(&line.ts)?;
        if ts.offset().local_minus_utc() != 0 || previous.is_some_and(|prev| prev > ts) {
            return Err("Diagnostic timeline must be ordered UTC".into());
        }
        previous = Some(ts);
    }
    Ok(bundle)
}

/// A validated delivery. The stable identity is scoped by authenticated origin,
/// so two senders using the same delivery key cannot overwrite one another.
pub struct Incoming {
    id: Uuid,
    origin: String,
    digest: String,
    payload: Vec<u8>,
}

impl Incoming {
    pub(crate) fn id(&self) -> Uuid {
        self.id
    }
    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn new(origin: &str, delivery_key: &str, bytes: &[u8]) -> Result<Self, Error> {
        if origin.is_empty()
            || origin.len() > 256
            || origin.contains('\0')
            || delivery_key.is_empty()
            || delivery_key.len() > 256
        {
            return Err("Invalid diagnostic delivery identity".into());
        }
        let bundle = decode(bytes)?;
        // Drop unknown JSON fields: the inbox stores only the shared contract.
        let json = serde_json::to_vec(&bundle)?;
        let digest = format!("{:x}", Sha256::digest(&json));
        let payload = zstd::stream::encode_all(json.as_slice(), 3)?;
        if payload.len() > MAX_COMPRESSED_BYTES {
            return Err("Diagnostic compressed size limit exceeded".into());
        }
        let identity =
            Sha256::digest(format!("diagnostic-bundle/v1\0{origin}\0{delivery_key}").as_bytes());
        let mut id = [0; 16];
        id.copy_from_slice(&identity[..16]);
        Ok(Self {
            id: Uuid::from_bytes(id),
            origin: origin.into(),
            digest,
            payload,
        })
    }
}

pub fn query(bundle: Bundle, query: &log_query::Query) -> std::io::Result<log_query::QueryResult> {
    log_query::run_records(
        query,
        bundle.timeline.into_iter().map(|line| Record {
            ts: line.ts,
            pid: line.pid,
            label: serde_json::to_value(line.source)
                .expect("Source serializes")
                .as_str()
                .expect("Source is a string")
                .to_owned(),
            msg: line.message,
            line: None,
        }),
    )
}

/// Reads only a live receipt, with the same bounded filters as diagnostic-log.
pub async fn query_received(
    pool: &sqlx::PgPool,
    id: Uuid,
    filter: &log_query::Query,
) -> Result<Option<log_query::QueryResult>, Error> {
    // Bound retained compressed payloads and expanded parsing together. A
    // bounded response must not permit unbounded concurrent inbox scans.
    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let permit = SCANS
        .try_acquire()
        .map_err(|_| "Diagnostic inbox query busy")?;
    let payload: Option<Vec<u8>> = sqlx::query_scalar("SELECT payload FROM diagnostic_bundles WHERE id = $1 AND received > now() - interval '90 days'")
        .bind(id).fetch_optional(pool).await?;
    let filter = filter.clone();
    tokio::task::spawn_blocking(move || {
        // A canceled HTTP future cannot release the slot while parsing lives.
        let _permit = permit;
        payload
            .map(|bytes| query(decode(&bytes)?, &filter).map_err(Into::into))
            .transpose()
    })
    .await?
}

#[cfg(test)]
#[path = "diagnostic_bundles_tests.rs"]
mod tests;
