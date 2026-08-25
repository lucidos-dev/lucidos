//! Serve a file over HTTP with cache validators and byte-range support.
//!
//! The one caller today is `GET /api/v1/data/*path`, which serves everything
//! from a 2 KB markdown note to an 80 MB video artifact. Three properties the
//! callers depend on:
//!
//! - **Validators, because `Cache-Control: no-cache` needs something to
//!   revalidate against.** The engine stamps `no-cache` on every response, which
//!   means "revalidate before reuse", not "do not store". Without an `ETag` a
//!   browser cannot make a conditional request, so it reuses the stored body
//!   forever. A rebuilt video then plays its old bytes until the file is renamed.
//! - **Ranges, so a `<video>` element can seek** without refetching the whole
//!   resource.
//! - **Streaming**, so peak memory does not scale with the artifact.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// What a `Range` header asks for, once resolved against the file's length.
#[derive(Debug, PartialEq, Eq)]
enum RangeOutcome {
    /// No range, an unparseable one, or more than one spec. RFC 9110 lets a
    /// server answer any range request with the full body, and multi-range would
    /// otherwise mean multipart/byteranges.
    Full,
    /// Inclusive byte offsets, both within the file.
    Slice { start: u64, end: u64 },
    /// Syntactically valid, but no byte of it exists. Answers 416.
    Unsatisfiable,
}

/// Serve `path` as `content_type`, honouring the conditional and range headers
/// in `request_headers`.
///
/// Always emits `Accept-Ranges: bytes`. Note that the compression layer strips
/// that header from anything it compresses, so `api::mod::compression_predicate`
/// is the other half of making ranges usable on media.
pub(super) async fn serve_file(
    path: &Path,
    content_type: &'static str,
    request_headers: &HeaderMap,
) -> Response {
    // Open first, then stat the open handle, and stream that same handle. A
    // path-based stat would describe a different file from the one we send the
    // moment an artifact is rebuilt mid-request: the headers would carry the old
    // length and ETag over the new bytes. Rebuilding an artifact is exactly the
    // workflow this module exists for, so the window is not hypothetical.
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => return read_failed(e),
    };
    let meta = match file.metadata().await {
        Ok(meta) => meta,
        Err(e) => return read_failed(e),
    };
    // A directory opens fine where the old `fs::read` failed. Without this it
    // would answer 200 with a Content-Length and an empty body.
    if !meta.is_file() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    let len = meta.len();
    let modified = meta.modified().ok();
    let etag = modified.as_ref().and_then(|m| etag_for(len, m));
    let last_modified = modified.as_ref().map(http_date);

    let mut base = HeaderMap::new();
    base.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    base.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(etag) = etag.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
        base.insert(header::ETAG, etag);
    }
    if let Some(date) = last_modified
        .as_deref()
        .and_then(|v| HeaderValue::from_str(v).ok())
    {
        base.insert(header::LAST_MODIFIED, date);
    }

    if let Some(etag) = etag.as_deref() {
        if header_str(request_headers, header::IF_NONE_MATCH)
            .is_some_and(|h| if_none_match_hits(h, etag))
        {
            return (StatusCode::NOT_MODIFIED, base).into_response();
        }
    }

    // A stale `If-Range` degrades to the full body. Serving the requested slice
    // of a file that has since changed would splice old and new bytes together.
    let range_is_fresh = match header_str(request_headers, header::IF_RANGE) {
        None => true,
        Some(v) => {
            let v = v.trim();
            etag.as_deref().is_some_and(|e| e == v)
                || last_modified.as_deref().is_some_and(|d| d == v)
        }
    };
    let outcome = if range_is_fresh {
        resolve_range(header_str(request_headers, header::RANGE), len)
    } else {
        RangeOutcome::Full
    };

    match outcome {
        RangeOutcome::Unsatisfiable => {
            let mut headers = base;
            // No payload, so the file's own type would be a lie.
            headers.remove(header::CONTENT_TYPE);
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{}", len)) {
                headers.insert(header::CONTENT_RANGE, v);
            }
            (StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response()
        }
        RangeOutcome::Full => match body_slice(file, 0, len).await {
            Ok(body) => (StatusCode::OK, with_length(base, len), body).into_response(),
            Err(e) => read_failed(e),
        },
        RangeOutcome::Slice { start, end } => {
            let count = end - start + 1;
            match body_slice(file, start, count).await {
                Ok(body) => {
                    let mut headers = with_length(base, count);
                    if let Ok(v) =
                        HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, len))
                    {
                        headers.insert(header::CONTENT_RANGE, v);
                    }
                    (StatusCode::PARTIAL_CONTENT, headers, body).into_response()
                }
                Err(e) => read_failed(e),
            }
        }
    }
}

fn read_failed(e: std::io::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Error reading file: {}", e),
    )
        .into_response()
}

fn with_length(mut headers: HeaderMap, len: u64) -> HeaderMap {
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers
}

/// Stream `count` bytes from `start`. `ReaderStream` reads in chunks, so an
/// 80 MB artifact never lands in memory whole.
async fn body_slice(
    mut file: tokio::fs::File,
    start: u64,
    count: u64,
) -> Result<Body, std::io::Error> {
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    Ok(Body::from_stream(ReaderStream::new(file.take(count))))
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

/// A strong validator built from length and mtime. Both change on a rebuild, so
/// a same-size overwrite still produces a new tag.
fn etag_for(len: u64, modified: &SystemTime) -> Option<String> {
    let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("\"{}-{}\"", len, nanos))
}

/// IMF-fixdate, the only format RFC 9110 lets a server send. chrono's `%a` and
/// `%b` are English regardless of locale, which is what the format requires.
fn http_date(t: &SystemTime) -> String {
    let stamp: chrono::DateTime<chrono::Utc> = (*t).into();
    stamp.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// `If-None-Match` is a comma-separated list, or `*`. A cache may weaken a tag
/// it stored, so compare past a `W/` prefix.
fn if_none_match_hits(header: &str, etag: &str) -> bool {
    let header = header.trim();
    if header == "*" {
        return true;
    }
    header
        .split(',')
        .map(|candidate| candidate.trim().trim_start_matches("W/"))
        .any(|candidate| candidate == etag)
}

fn resolve_range(header: Option<&str>, len: u64) -> RangeOutcome {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeOutcome::Full;
    };
    let mut parts = spec.split(',');
    let (Some(single), None) = (parts.next(), parts.next()) else {
        return RangeOutcome::Full;
    };
    let single = single.trim();
    let Some((first, last)) = single.split_once('-') else {
        return RangeOutcome::Full;
    };
    let (first, last) = (first.trim(), last.trim());

    // `bytes=-N`: the last N bytes. N == 0 asks for nothing, which is a range no
    // byte satisfies.
    if first.is_empty() {
        let Ok(suffix) = last.parse::<u64>() else {
            return RangeOutcome::Full;
        };
        if suffix == 0 || len == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        return RangeOutcome::Slice {
            start: len.saturating_sub(suffix),
            end: len - 1,
        };
    }

    let Ok(start) = first.parse::<u64>() else {
        return RangeOutcome::Full;
    };
    if start >= len {
        return RangeOutcome::Unsatisfiable;
    }
    // `bytes=N-` runs to the end. An explicit end past the last byte clamps
    // rather than failing, per RFC 9110.
    let end = if last.is_empty() {
        len - 1
    } else {
        match last.parse::<u64>() {
            Ok(end) => end.min(len - 1),
            Err(_) => return RangeOutcome::Full,
        }
    };
    if end < start {
        return RangeOutcome::Unsatisfiable;
    }
    RangeOutcome::Slice { start, end }
}

#[cfg(test)]
#[path = "file_response_tests.rs"]
mod tests;
