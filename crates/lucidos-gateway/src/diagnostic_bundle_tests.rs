use super::*;
use lucidos_diagnostic_log::{line, private, writer::Record};

fn record(ts: &str, text: &str) -> Record {
    Record {
        ts: ts.into(),
        pid: 4,
        label: "Test".into(),
        msg: "CACHE_PRIVATE_MARKER".into(),
        line: Some(line!("failure {}", private(text))),
    }
}
fn roots() -> (tempfile::TempDir, Sources) {
    let dir = tempfile::tempdir().unwrap();
    let sources = Sources::new(
        &dir.path().join("workspace"),
        &dir.path().join("gateway"),
        Some(dir.path().join("client")),
    );
    (dir, sources)
}
fn write(sources: &Sources, source: Source, name: &str, records: &[Record]) {
    let dir = sources
        .logs
        .iter()
        .find(|(kind, _)| *kind == source)
        .unwrap()
        .1
        .as_ref()
        .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(name),
        records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}
fn unpack(bytes: &[u8]) -> Bundle {
    serde_json::from_slice(&zstd::stream::decode_all(bytes).unwrap()).unwrap()
}

#[test]
fn engine_down_keeps_all_four_logs_and_lists_missing_parts() {
    let (_dir, sources) = roots();
    for (i, source) in [
        Source::Engine,
        Source::Gateway,
        Source::Client,
        Source::ClientRelaunch,
    ]
    .into_iter()
    .enumerate()
    {
        write(
            &sources,
            source,
            FILE_NAME,
            &[record(
                &format!("2026-11-16T00:00:0{}Z", 4 - i),
                "PRIVATE_MARKER",
            )],
        );
    }
    std::fs::write(
        sources.logs[1].1.as_ref().unwrap().join("client.log"),
        "RAW_MARKER",
    )
    .unwrap();
    let owner = unpack(&build(&sources, None, Destination::Owner, MAX_COMPRESSED_BYTES).unwrap());
    assert_eq!(owner.timeline.len(), 4);
    assert!(owner.timeline.windows(2).all(|w| w[0].ts <= w[1].ts));
    for source in [
        Source::Events,
        Source::Faults,
        Source::EngineStatus,
        Source::Config,
    ] {
        assert_eq!(
            owner.manifest.sources[&source].missing,
            Some(Missing::EngineUnavailable)
        );
    }
    let bytes = build(
        &sources,
        None,
        Destination::Maintainer,
        MAX_COMPRESSED_BYTES,
    )
    .unwrap();
    let decoded = String::from_utf8(zstd::stream::decode_all(bytes.as_slice()).unwrap()).unwrap();
    for marker in ["PRIVATE_MARKER", "CACHE_PRIVATE_MARKER", "RAW_MARKER"] {
        assert!(!decoded.contains(marker));
    }
    assert!(decoded.contains("<text 14 chars>"));
}

#[test]
fn rotation_legacy_invalid_and_offset_timestamps_are_explicit() {
    let (_dir, sources) = roots();
    let mut legacy = record("2026-11-16T01:00:00+01:00", "unused");
    legacy.line = None;
    write(&sources, Source::Engine, ROTATED_FILE_NAME, &[legacy]);
    write(
        &sources,
        Source::Engine,
        FILE_NAME,
        &[record("bad", "invalid")],
    );
    let bundle = unpack(
        &build(
            &sources,
            None,
            Destination::Maintainer,
            MAX_COMPRESSED_BYTES,
        )
        .unwrap(),
    );
    assert_eq!(bundle.timeline[0].ts, "2026-11-16T00:00:00.000Z");
    assert_eq!(bundle.timeline[0].message, "<unstructured line>");
    assert_eq!(bundle.manifest.sources[&Source::Engine].invalid_lines, 1);
}

#[test]
fn compression_cap_keeps_a_suffix_and_reports_every_trimmed_line() {
    let (_dir, sources) = roots();
    let records = (0..300)
        .map(|i| {
            record(
                &format!("2026-11-16T00:{:02}:{:02}Z", i / 60, i % 60),
                &format!("{i:08x} {}", (i as u64).wrapping_mul(0xafe1234321)),
            )
        })
        .collect::<Vec<_>>();
    write(&sources, Source::Engine, FILE_NAME, &records);
    let full = build(&sources, None, Destination::Owner, MAX_COMPRESSED_BYTES).unwrap();
    assert_eq!(
        build(&sources, None, Destination::Owner, full.len())
            .unwrap()
            .len(),
        full.len()
    );
    let cap = full.len() / 2;
    let bytes = build(&sources, None, Destination::Owner, cap).unwrap();
    assert!(bytes.len() <= cap);
    let bundle = unpack(&bytes);
    assert!(!bundle.timeline.is_empty());
    assert_eq!(
        bundle.manifest.trimmed_lines + bundle.timeline.len(),
        records.len()
    );
    assert_eq!(
        bundle.timeline.last().unwrap().message,
        records.last().unwrap().render(Destination::Owner)
    );
    assert!(build(&sources, None, Destination::Owner, 1).is_err());
}

#[test]
fn expanded_cap_trims_repetitive_lines_even_when_compressed_size_fits() {
    let (_dir, sources) = roots();
    let records = (0..100)
        .map(|i| {
            record(
                &format!("2026-11-16T00:{:02}:{:02}Z", i / 60, i % 60),
                &"repetitive text ".repeat(100),
            )
        })
        .collect::<Vec<_>>();
    write(&sources, Source::Engine, FILE_NAME, &records);
    let full = unpack(&build(&sources, None, Destination::Owner, MAX_COMPRESSED_BYTES).unwrap());
    let expanded = serde_json::to_vec(&full).unwrap().len() / 2;
    let bytes = compress_to_limits(full, MAX_COMPRESSED_BYTES, expanded).unwrap();
    let json = zstd::stream::decode_all(bytes.as_slice()).unwrap();
    assert!(json.len() <= expanded);
    let bundle: Bundle = serde_json::from_slice(&json).unwrap();
    assert!(bundle.manifest.trimmed_lines > 0);
    assert_eq!(
        bundle.manifest.trimmed_lines + bundle.timeline.len(),
        records.len()
    );
    assert!(compress_to_limits(bundle, MAX_COMPRESSED_BYTES, 1).is_err());
}

#[test]
fn snapshot_records_and_crash_leftovers_are_rendered_without_raw_fields() {
    let (_dir, sources) = roots();
    let root = sources.workspace.join(".lucidos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("engine.last-death.json"), r#"{"old_pid":7,"exit_code":139,"died_at":"2026-11-16T00:00:00Z","supervisor_pid":9,"unknown":"DEATH_PRIVATE_MARKER"}"#).unwrap();
    let snapshot = Snapshot {
        events: vec![record("2026-11-16T00:00:01Z", "EVENT_PRIVATE_MARKER")],
        faults: vec![record("2026-11-16T00:00:02Z", "FAULT_PRIVATE_MARKER")],
        ..Default::default()
    };
    let bytes = build(
        &sources,
        Some(snapshot),
        Destination::Maintainer,
        MAX_COMPRESSED_BYTES,
    )
    .unwrap();
    let text = String::from_utf8(zstd::stream::decode_all(bytes.as_slice()).unwrap()).unwrap();
    assert!(!text.contains("PRIVATE_MARKER"));
    let bundle = unpack(&bytes);
    assert_eq!(bundle.timeline.len(), 3);
    assert_eq!(bundle.manifest.sources[&Source::LastDeath].lines, 1);
    assert_eq!(bundle.manifest.sources[&Source::Events].missing, None);
}

#[test]
fn symlink_and_invalid_segments_never_disappear_from_the_manifest() {
    use std::os::unix::fs::symlink;
    let (dir, sources) = roots();
    let logdir = sources.logs[0].1.as_ref().unwrap();
    std::fs::create_dir_all(logdir).unwrap();
    let raw = dir.path().join("raw.log");
    std::fs::write(
        &raw,
        serde_json::to_vec(&record("2026-11-16T00:00:00Z", "RAW_PRIVATE_MARKER")).unwrap(),
    )
    .unwrap();
    symlink(&raw, logdir.join(FILE_NAME)).unwrap();
    let bundle = unpack(&build(&sources, None, Destination::Owner, MAX_COMPRESSED_BYTES).unwrap());
    assert!(bundle.timeline.is_empty());
    assert_eq!(
        bundle.manifest.sources[&Source::Engine].missing,
        Some(Missing::Unreadable)
    );
    assert_eq!(
        bundle.manifest.sources[&Source::Engine].unreadable_segments,
        1
    );
}

#[tokio::test]
async fn socket_snapshot_and_engine_down_fallback() {
    use axum::{routing::get, Json};
    let dir = crate::engine_hop::test_engine::scratch();
    let snapshot = Snapshot {
        events: vec![record("2026-11-16T00:00:00Z", "SOCKET_PRIVATE_MARKER")],
        ..Default::default()
    };
    let snapshot = std::sync::Arc::new(snapshot);
    let address = crate::engine_hop::test_engine::serve(
        &dir,
        axum::Router::new().route(
            "/api/v1/engine/diagnostic-snapshot",
            get(move || {
                let snapshot = snapshot.clone();
                async move { Json(serde_json::to_value(&*snapshot).unwrap()) }
            }),
        ),
    );
    let response = request_snapshot(&address, "test-token").await.unwrap();
    assert_eq!(response.events.len(), 1);
    let (_dir, sources) = roots();
    let bytes = build(
        &sources,
        Some(response),
        Destination::Maintainer,
        MAX_COMPRESSED_BYTES,
    )
    .unwrap();
    let text = String::from_utf8(zstd::stream::decode_all(bytes.as_slice()).unwrap()).unwrap();
    assert!(!text.contains("SOCKET_PRIVATE_MARKER"));
    let down = crate::engine_hop::test_engine::unreachable(&dir);
    assert!(request_snapshot(&down, "test-token").await.is_none());
}
