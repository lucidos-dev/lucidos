use super::*;

/// 26 bytes, so an offset reads back as a letter and an off-by-one is visible.
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(name.clone(), HeaderValue::from_str(value).unwrap());
    }
    map
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

fn header_of(response: &Response, name: header::HeaderName) -> Option<String> {
    Some(response.headers().get(name)?.to_str().unwrap().to_string())
}

#[tokio::test]
async fn a_full_read_advertises_ranges_and_carries_validators() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "note.md", b"# hello");

    let response = serve_file(&path, "text/plain", &HeaderMap::new()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, header::ACCEPT_RANGES).as_deref(),
        Some("bytes")
    );
    assert_eq!(
        header_of(&response, header::CONTENT_TYPE).as_deref(),
        Some("text/plain")
    );
    assert_eq!(
        header_of(&response, header::CONTENT_LENGTH).as_deref(),
        Some("7")
    );
    let etag = header_of(&response, header::ETAG).expect("etag");
    assert!(etag.starts_with("\"7-"), "etag was {etag}");
    let modified = header_of(&response, header::LAST_MODIFIED).expect("last-modified");
    assert!(modified.ends_with(" GMT"), "last-modified was {modified}");
    assert_eq!(body_bytes(response).await, b"# hello");
}

#[tokio::test]
async fn a_mid_file_range_serves_only_that_slice() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::RANGE, "bytes=5-9")]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&response, header::CONTENT_RANGE).as_deref(),
        Some("bytes 5-9/26")
    );
    assert_eq!(
        header_of(&response, header::CONTENT_LENGTH).as_deref(),
        Some("5")
    );
    assert_eq!(
        header_of(&response, header::ACCEPT_RANGES).as_deref(),
        Some("bytes")
    );
    assert_eq!(body_bytes(response).await, b"fghij");
}

#[tokio::test]
async fn an_open_ended_range_runs_to_the_last_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::RANGE, "bytes=20-")]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&response, header::CONTENT_RANGE).as_deref(),
        Some("bytes 20-25/26")
    );
    assert_eq!(body_bytes(response).await, b"uvwxyz");
}

#[tokio::test]
async fn a_suffix_range_serves_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(&path, "video/mp4", &headers(&[(header::RANGE, "bytes=-3")])).await;

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&response, header::CONTENT_RANGE).as_deref(),
        Some("bytes 23-25/26")
    );
    assert_eq!(body_bytes(response).await, b"xyz");
}

#[tokio::test]
async fn a_range_past_the_end_is_416_with_the_total_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::RANGE, "bytes=99-200")]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        header_of(&response, header::CONTENT_RANGE).as_deref(),
        Some("bytes */26")
    );
    assert!(body_bytes(response).await.is_empty());
}

#[tokio::test]
async fn a_multi_range_request_falls_back_to_the_full_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::RANGE, "bytes=0-3,10-12")]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(body_bytes(response).await, ALPHABET);
}

#[tokio::test]
async fn a_matching_if_none_match_is_304_with_no_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let first = serve_file(&path, "video/mp4", &HeaderMap::new()).await;
    let etag = header_of(&first, header::ETAG).expect("etag");

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::IF_NONE_MATCH, &etag)]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_of(&response, header::ETAG).as_deref(), Some(&*etag));
    assert!(body_bytes(response).await.is_empty());
}

#[tokio::test]
async fn a_weakened_or_listed_if_none_match_still_hits() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);
    let etag = header_of(
        &serve_file(&path, "video/mp4", &HeaderMap::new()).await,
        header::ETAG,
    )
    .expect("etag");

    for value in [
        format!("W/{etag}"),
        format!("\"stale\", {etag}"),
        "*".into(),
    ] {
        let response = serve_file(
            &path,
            "video/mp4",
            &headers(&[(header::IF_NONE_MATCH, &value)]),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_MODIFIED,
            "if-none-match: {value}"
        );
    }
}

/// The reported bug: a rebuilt artifact kept playing its old bytes. The
/// validator has to move when the file does, or the browser never refetches.
#[tokio::test]
async fn a_rewritten_file_gets_a_new_etag() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);
    let before = header_of(
        &serve_file(&path, "video/mp4", &HeaderMap::new()).await,
        header::ETAG,
    )
    .expect("etag");

    // A same-second rewrite would be indistinguishable on a coarse clock, so
    // move both halves of the tag: the length and the mtime.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(&path, b"rebuilt with different bytes entirely").unwrap();

    let after = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::IF_NONE_MATCH, &before)]),
    )
    .await;

    assert_eq!(after.status(), StatusCode::OK);
    assert_ne!(header_of(&after, header::ETAG), Some(before));
    assert_eq!(
        body_bytes(after).await,
        b"rebuilt with different bytes entirely"
    );
}

/// A range against a validator that has moved must never splice old and new
/// bytes together.
#[tokio::test]
async fn a_stale_if_range_degrades_to_the_full_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(
        &path,
        "video/mp4",
        &headers(&[(header::IF_RANGE, "\"26-1\""), (header::RANGE, "bytes=5-9")]),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(body_bytes(response).await, ALPHABET);
}

#[tokio::test]
async fn a_fresh_if_range_serves_the_slice() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);
    let first = serve_file(&path, "video/mp4", &HeaderMap::new()).await;
    let etag = header_of(&first, header::ETAG).expect("etag");
    let modified = header_of(&first, header::LAST_MODIFIED).expect("last-modified");

    for validator in [etag, modified] {
        let response = serve_file(
            &path,
            "video/mp4",
            &headers(&[(header::IF_RANGE, &validator), (header::RANGE, "bytes=5-9")]),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::PARTIAL_CONTENT,
            "if-range: {validator}"
        );
    }
}

/// The headers and the bytes must describe one file. A rebuild that lands
/// between them would otherwise send new bytes under the old length and ETag.
/// A build tool writes a temp file and renames it over the target, so the open
/// handle keeps the version its headers were measured from.
#[tokio::test]
async fn a_rebuild_mid_request_cannot_change_the_bytes_already_promised() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "clip.mp4", ALPHABET);

    let response = serve_file(&path, "video/mp4", &HeaderMap::new()).await;
    assert_eq!(
        header_of(&response, header::CONTENT_LENGTH).as_deref(),
        Some("26")
    );

    let replacement = write(
        dir.path(),
        "clip.mp4.tmp",
        b"a much longer rebuilt artifact",
    );
    std::fs::rename(&replacement, &path).unwrap();

    assert_eq!(
        body_bytes(response).await,
        ALPHABET,
        "the body must be the file the headers measured"
    );
}

#[tokio::test]
async fn a_missing_file_is_404_and_a_directory_is_not_an_empty_200() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();

    let missing = serve_file(&dir.path().join("gone.md"), "text/plain", &HeaderMap::new()).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let directory = serve_file(&dir.path().join("nested"), "text/plain", &HeaderMap::new()).await;
    assert_eq!(directory.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_empty_file_reads_as_an_empty_200() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "empty.txt", b"");

    let response = serve_file(&path, "text/plain", &HeaderMap::new()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, header::CONTENT_LENGTH).as_deref(),
        Some("0")
    );
    assert!(body_bytes(response).await.is_empty());
}

#[tokio::test]
async fn every_byte_of_a_larger_file_survives_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let path = write(dir.path(), "big.bin", &bytes);

    let full = serve_file(&path, "application/octet-stream", &HeaderMap::new()).await;
    assert_eq!(body_bytes(full).await, bytes);

    let slice = serve_file(
        &path,
        "application/octet-stream",
        &headers(&[(header::RANGE, "bytes=131072-262143")]),
    )
    .await;
    assert_eq!(
        header_of(&slice, header::CONTENT_LENGTH).as_deref(),
        Some("131072")
    );
    assert_eq!(body_bytes(slice).await, bytes[131072..=262143]);
}

#[test]
fn range_specs_resolve_against_the_file_length() {
    use RangeOutcome::*;

    assert_eq!(resolve_range(None, 26), Full);
    assert_eq!(
        resolve_range(Some("bytes=0-0"), 26),
        Slice { start: 0, end: 0 }
    );
    // An end past the last byte clamps rather than failing.
    assert_eq!(
        resolve_range(Some("bytes=20-999"), 26),
        Slice { start: 20, end: 25 }
    );
    // A suffix longer than the file is the whole file.
    assert_eq!(
        resolve_range(Some("bytes=-999"), 26),
        Slice { start: 0, end: 25 }
    );
    assert_eq!(resolve_range(Some("bytes=26-"), 26), Unsatisfiable);
    assert_eq!(resolve_range(Some("bytes=-0"), 26), Unsatisfiable);
    assert_eq!(resolve_range(Some("bytes=9-5"), 26), Unsatisfiable);
    assert_eq!(resolve_range(Some("bytes=0-0"), 0), Unsatisfiable);
    // Anything we cannot read is answered with the full body, never a guess.
    for junk in ["", "bytes=", "items=0-1", "bytes=abc-def", "bytes=1-x"] {
        assert_eq!(resolve_range(Some(junk), 26), Full, "spec: {junk}");
    }
}
