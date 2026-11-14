use super::*;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn parsed(raw: &str) -> Query {
    parse(Some(raw), now()).unwrap_or_else(|e| panic!("{raw}: {e}"))
}

fn refused(raw: &str) -> String {
    parse(Some(raw), now()).expect_err(raw)
}

#[test]
fn an_empty_query_reads_the_last_hour_with_the_default_budgets() {
    let q = parse(None, now()).unwrap();
    assert_eq!(q.until, now());
    assert_eq!(q.since, now() - Duration::hours(1));
    assert_eq!((q.limit, q.byte_limit), (DEFAULT_LIMIT, DEFAULT_BYTE_LIMIT));
}

#[test]
fn times_take_rfc_3339_or_an_age() {
    let q = parsed("since=90m&until=2026-11-12T11%3A59%3A00%2B01%3A00");
    assert_eq!(q.since, now() - Duration::minutes(90));
    assert_eq!(q.until, now() - Duration::minutes(61));
    assert_eq!(parsed("since=3d").since, now() - Duration::days(3));
    assert_eq!(parsed("since=30s").since, now() - Duration::seconds(30));
    for bad in [
        "since=yesterday",
        "since=5w",
        "since=-2h",
        "since=h",
        "since=",
        // A multibyte last character must refuse, not split mid-character.
        "since=2%C2%B5",
        "since=5%EF%BD%8D",
    ] {
        assert!(refused(bad).contains("RFC 3339"), "{bad}");
    }
}

#[test]
fn the_window_is_at_most_seven_days_and_never_inverted() {
    parsed("since=7d");
    assert!(refused("since=8d").contains("7 days"));
    assert!(refused("since=1h&until=2h").contains("after"));
}

#[test]
fn contains_takes_at_most_four_filters_of_256_bytes() {
    let q = parsed("contains=a&contains=b&contains=c&contains=d");
    assert_eq!(q.contains, ["a", "b", "c", "d"]);
    assert!(refused("contains=a&contains=b&contains=c&contains=d&contains=e").contains("at most"));
    parsed(&format!("contains={}", "x".repeat(MAX_FILTER_BYTES)));
    assert!(refused(&format!("contains={}", "x".repeat(MAX_FILTER_BYTES + 1))).contains("256"));
    assert!(refused("contains=").contains("empty"));
}

#[test]
fn a_regex_is_bounded_in_length_and_compiled_size() {
    parsed("regex=%5BVoice%5D.*pid");
    parsed(&format!("regex={}", "a".repeat(MAX_FILTER_BYTES)));
    assert!(refused(&format!("regex={}", "a".repeat(MAX_FILTER_BYTES + 1))).contains("256"));
    assert!(refused("regex=(unclosed").contains("compile"));
    assert!(refused("regex=(%5Cw%7B100%7D)%7B100%7D").contains("compile"));
    assert!(refused("regex=a&regex=b").contains("twice"));
}

#[test]
fn label_and_pid_are_checked() {
    parsed(&format!("label={}", "L".repeat(MAX_LABEL_BYTES)));
    assert!(refused(&format!("label={}", "L".repeat(MAX_LABEL_BYTES + 1))).contains("64"));
    assert_eq!(parsed("pid=77210").pid, Some(77210));
    assert!(refused("pid=abc").contains("process id"));
}

#[test]
fn budgets_clamp_to_their_maximum_and_refuse_zero() {
    let q = parsed("limit=5000&byte_limit=999999999");
    assert_eq!((q.limit, q.byte_limit), (MAX_LIMIT, MAX_BYTE_LIMIT));
    let q = parsed("limit=2000&byte_limit=262144");
    assert_eq!((q.limit, q.byte_limit), (MAX_LIMIT, MAX_BYTE_LIMIT));
    assert!(refused("limit=0").contains("positive"));
    assert!(refused("byte_limit=lots").contains("positive"));
}

#[test]
fn an_unknown_or_repeated_parameter_refuses() {
    assert!(refused("path=/etc/passwd").contains("unknown parameter `path`"));
    assert!(refused("since=1h&since=2h").contains("twice"));
}

/// A record as an older or a raw writer could have left it.
fn line(ts: &str, pid: u32, label: &str, msg: &str) -> String {
    serde_json::json!({ "ts": ts, "pid": pid, "label": label, "msg": msg }).to_string()
}

fn segment(dir: &Path, name: &str, lines: &[String]) {
    std::fs::write(dir.join(name), lines.join("\n") + "\n").unwrap();
}

const GH_TOKEN: &str = "ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d";

#[test]
fn a_raw_secret_on_disk_is_masked_when_read() {
    let tmp = tempfile::tempdir().unwrap();
    segment(
        tmp.path(),
        writer::FILE_NAME,
        &[line(
            "2026-11-12T11:30:00.000Z",
            7,
            "Engine",
            &format!("[Voice] token {GH_TOKEN} at https://x.example/cb?code=abc\nforged line"),
        )],
    );
    let result = run(tmp.path(), &parsed("")).unwrap();
    assert_eq!(result.returned, 1);
    assert_eq!(
        result.lines[0].msg,
        "[Voice] token ghp_[REDACTED] at https://x.example/cb?[query omitted]\\nforged line"
    );
}

#[test]
fn a_filter_cannot_probe_a_masked_value() {
    let tmp = tempfile::tempdir().unwrap();
    segment(
        tmp.path(),
        writer::FILE_NAME,
        &[line("2026-11-12T11:30:00.000Z", 7, "Engine", GH_TOKEN)],
    );
    for probe in ["contains=ghp_Ab3d", "regex=ghp_A"] {
        assert_eq!(
            run(tmp.path(), &parsed(probe)).unwrap().returned,
            0,
            "{probe}"
        );
    }
}

#[test]
fn the_newest_matches_come_back_oldest_first_across_both_segments() {
    let tmp = tempfile::tempdir().unwrap();
    segment(
        tmp.path(),
        writer::ROTATED_FILE_NAME,
        &[
            line("2026-11-12T11:10:00.000Z", 7, "Engine", "[Voice] one"),
            line("2026-11-12T11:20:00.000Z", 7, "Engine", "[Voice] two"),
        ],
    );
    segment(
        tmp.path(),
        writer::FILE_NAME,
        &[
            line("2026-11-12T11:30:00.000Z", 8, "Engine", "[Voice] three"),
            line("2026-11-12T11:35:00.000Z", 8, "Push", "other"),
            "not json".to_string(),
            line("2026-11-12T11:40:00.000Z", 8, "Engine", "[Voice] four"),
        ],
    );
    let result = run(tmp.path(), &parsed("label=voice&limit=2")).unwrap();
    let msgs: Vec<&str> = result.lines.iter().map(|r| r.msg.as_str()).collect();
    assert_eq!(msgs, ["[Voice] three", "[Voice] four"]);
    assert_eq!(result.earlier_matches, 2);
    assert!(!result.truncated);
    assert!(result.hint.unwrap().contains("limit"));
    assert_eq!(
        result.coverage_starts_at.as_deref(),
        Some("2026-11-12T11:10:00+00:00")
    );

    let by_label = run(tmp.path(), &parsed("label=push")).unwrap();
    assert_eq!(by_label.lines[0].msg, "other");
    let by_pid = run(tmp.path(), &parsed("pid=7&contains=two")).unwrap();
    assert_eq!(by_pid.lines[0].msg, "[Voice] two");
    let outside = run(tmp.path(), &parsed("since=10m")).unwrap();
    assert_eq!(outside.returned, 0);
}

/// A rotation between the two opens leaves both names on one file. That file
/// is read once, not twice and not skipped.
#[cfg(unix)]
#[test]
fn a_rotation_mid_scan_reads_the_rotated_file_once() {
    let tmp = tempfile::tempdir().unwrap();
    segment(
        tmp.path(),
        writer::FILE_NAME,
        &[line("2026-11-12T11:30:00.000Z", 7, "Engine", "only once")],
    );
    std::fs::hard_link(
        tmp.path().join(writer::FILE_NAME),
        tmp.path().join(writer::ROTATED_FILE_NAME),
    )
    .unwrap();
    let result = run(tmp.path(), &parsed("")).unwrap();
    let msgs: Vec<&str> = result.lines.iter().map(|r| r.msg.as_str()).collect();
    assert_eq!(msgs, ["only once"]);
}

#[test]
fn byte_limit_keeps_the_newest_lines_that_fit() {
    let tmp = tempfile::tempdir().unwrap();
    let lines: Vec<String> = (0..10)
        .map(|i| {
            line(
                &format!("2026-11-12T11:3{i}:00.000Z"),
                7,
                "E",
                &format!("{i}{}", "x".repeat(200)),
            )
        })
        .collect();
    segment(tmp.path(), writer::FILE_NAME, &lines);
    let result = run(tmp.path(), &parsed("byte_limit=700")).unwrap();
    assert!(result.byte_size <= 700, "{}", result.byte_size);
    assert!(result.truncated);
    assert_eq!(result.returned + result.earlier_matches, 10);
    assert!(result.lines.last().unwrap().msg.starts_with('9'));
}

#[test]
fn an_empty_window_before_retention_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    segment(
        tmp.path(),
        writer::FILE_NAME,
        &[line("2026-11-12T11:59:00.000Z", 7, "E", "late")],
    );
    let result = run(tmp.path(), &parsed("since=2h&until=90m")).unwrap();
    assert_eq!(result.returned, 0);
    assert!(result.hint.unwrap().contains("oldest retained"));
}

#[cfg(unix)]
#[test]
fn the_reader_refuses_a_symlinked_dir_or_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    segment(
        &outside,
        writer::FILE_NAME,
        &[line("2026-11-12T11:30:00.000Z", 7, "E", "outside secret")],
    );
    let linked_dir = tmp.path().join("linked-dir");
    std::os::unix::fs::symlink(&outside, &linked_dir).unwrap();
    assert!(run(&linked_dir, &parsed("")).is_err());

    let real_dir = tmp.path().join("real-dir");
    std::fs::create_dir(&real_dir).unwrap();
    std::os::unix::fs::symlink(
        outside.join(writer::FILE_NAME),
        real_dir.join(writer::FILE_NAME),
    )
    .unwrap();
    assert!(run(&real_dir, &parsed("")).is_err());
}

#[test]
fn a_missing_dir_is_an_error_and_an_empty_dir_is_an_empty_log() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(run(&tmp.path().join("absent"), &parsed("")).is_err());
    let result = run(tmp.path(), &parsed("")).unwrap();
    assert_eq!((result.returned, result.coverage_starts_at), (0, None));
}

/// The whole write path, then a query: each secret class is masked in the
/// file and in the answer.
#[test]
fn every_secret_class_is_masked_in_the_file_and_the_answer() {
    sanitize::register_secret_values(["registered-known-credential".to_string()]);
    let classes = [
        ("known value", "registered-known-credential"),
        ("token shape", GH_TOKEN),
        ("postgres password", "postgres://u:pgsecretpw@h/d"),
        ("bearer value", "Bearer opaque.bearer.value"),
        ("secret pair", "client_secret=opaque-pair-value"),
        (
            "url query",
            "https://x.example/p?private=opaque-query-value",
        ),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let (sink, rx, dropped) = writer::channel(16);
    for (class, text) in classes {
        sink.record("Engine", format!("{class}: {text}"));
    }
    drop(sink);
    writer::drain(rx, writer::Segments::open(tmp.path()).unwrap(), &dropped);
    let file = std::fs::read_to_string(tmp.path().join(writer::FILE_NAME)).unwrap();
    let answer =
        serde_json::to_string(&run(tmp.path(), &parse(None, Utc::now()).unwrap()).unwrap())
            .unwrap();
    for planted in [
        "registered-known-credential",
        "Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d",
        "pgsecretpw",
        "opaque.bearer.value",
        "opaque-pair-value",
        "opaque-query-value",
    ] {
        assert!(
            !file.contains(planted),
            "{planted} reached the file: {file}"
        );
        assert!(
            !answer.contains(planted),
            "{planted} reached the answer: {answer}"
        );
    }
    assert!(answer.contains("\"returned\":6"), "{answer}");
}
