use lucidos_diagnostic_log::{line, private, Destination, Line};

#[test]
fn maintainer_rendering_never_uses_private_or_legacy_text() {
    let message = "PRIVATE_CONVERSATION_MARKER æ🙂";
    let record = line!("request {id} failed: {}", private(message), id = 42_u64);
    let maintainer = record.render(Destination::Maintainer);
    assert!(!maintainer.contains("PRIVATE_CONVERSATION_MARKER"));
    assert!(maintainer.contains(&format!("<text {} chars>", message.chars().count())));
    assert!(maintainer.contains("request 42 failed:"));
    assert!(record.render(Destination::Owner).contains(message));
    let legacy = Line::legacy("PRIVATE_LEGACY_MARKER".into());
    assert_eq!(
        legacy.render(Destination::Maintainer),
        "<unstructured line>"
    );
}

#[test]
fn formatted_and_repeated_captures_keep_their_class() {
    let message = private("PRIVATE_REPEAT_MARKER");
    let record = line!("{message:>40} {message:?} {{literal}} {n:04x}", n = 31_u16);
    assert!(!record
        .render(Destination::Maintainer)
        .contains("PRIVATE_REPEAT_MARKER"));
    assert!(record
        .render(Destination::Maintainer)
        .contains("{literal} 001f"));
}

#[test]
fn typed_and_legacy_records_round_trip_with_current_masking() {
    let record = line!("value {}", private("token=opaque-secret-marker"));
    let stored = serde_json::to_string(&record).unwrap();
    assert!(!stored.contains("opaque-secret-marker"));
    let restored: Line = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        record.render(Destination::Maintainer),
        restored.render(Destination::Maintainer)
    );
}

#[test]
fn writer_keeps_classification_and_never_exports_the_cached_message() {
    use lucidos_diagnostic_log::writer::{channel, drain, Record, Segments, FILE_NAME};
    let tmp = tempfile::tempdir().unwrap();
    let (sink, rx, dropped) = channel(8);
    sink.record_line(
        "Test",
        line!("request {}", private("PRIVATE_WRITER_MARKER")),
    );
    sink.record("Test", "PRIVATE_LEGACY_WRITER_MARKER".into());
    drop(sink);
    drain(rx, Segments::open(tmp.path()).unwrap(), &dropped);
    let raw = std::fs::read_to_string(tmp.path().join(FILE_NAME)).unwrap();
    let records: Vec<Record> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert!(records[0].line.is_some());
    for record in &records {
        assert!(!record.render(Destination::Maintainer).contains("PRIVATE_"));
        assert!(record.ts.ends_with('Z'));
        chrono::DateTime::parse_from_rfc3339(&record.ts).unwrap();
    }
    let old: Record = serde_json::from_str(
        r#"{"ts":"2026-01-01T00:00:00Z","pid":1,"label":"Test","msg":"PRIVATE_OLD_MARKER"}"#,
    )
    .unwrap();
    assert_eq!(old.render(Destination::Maintainer), "<unstructured line>");
}

#[test]
fn expressions_are_evaluated_once_and_dynamic_formats_match_rust() {
    let mut calls = 0;
    let record = line!(
        "{0:04} {0:>width$} {v:.precision$}",
        {
            calls += 1;
            7_u32
        },
        width = 6_usize,
        precision = 2_usize,
        v = 1.25_f64
    );
    assert_eq!(calls, 1);
    assert_eq!(record.render(Destination::Maintainer), "0007      7 1.25");
    assert_eq!(
        line!("{:.*}", 2_usize, 1.25_f64).render(Destination::Owner),
        "1.25"
    );
}

#[test]
fn zero_filled_dynamic_widths_match_rust() {
    assert_eq!(
        line!("{n:0width$}", n = 7_u32, width = 4_usize).render(Destination::Owner),
        "0007"
    );
    assert_eq!(
        line!("{0:01$}", 7_u32, 4_usize).render(Destination::Owner),
        "0007"
    );
}

#[test]
fn read_masking_runs_over_the_complete_rendered_line() {
    let record = line!("sk-{number}", number = 1234567890123456789012_u128);
    assert_eq!(record.render(Destination::Maintainer), "sk-[REDACTED]");
    assert_eq!(record.render(Destination::Owner), "sk-[REDACTED]");
}

#[test]
fn structured_queue_entries_are_bounded_across_all_fragments() {
    use lucidos_diagnostic_log::writer::{channel, drain, Record, Segments, FILE_NAME};
    let tmp = tempfile::tempdir().unwrap();
    let (sink, rx, dropped) = channel(1);
    let value = private("PRIVATE_LARGE_MARKER ".repeat(1000));
    sink.record_line(
        "Test",
        line!("{value}{value}{value}{value}{value}{value}{value}{value}"),
    );
    drop(sink);
    drain(rx, Segments::open(tmp.path()).unwrap(), &dropped);
    let raw = std::fs::read_to_string(tmp.path().join(FILE_NAME)).unwrap();
    assert!(raw.len() < 64 * 1024, "{} bytes queued", raw.len());
    let record: Record = serde_json::from_str(raw.trim()).unwrap();
    assert!(!record
        .render(Destination::Maintainer)
        .contains("PRIVATE_LARGE_MARKER"));
}
