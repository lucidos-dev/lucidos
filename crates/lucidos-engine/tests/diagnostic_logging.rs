//! Exercise the real process sink, macro wrappers, panic hook and fd-2 tee.
use lucidos_diagnostic_log::{writer::Record, Destination};
use std::process::Command;

#[test]
fn engine_logging_wiring() {
    if let Some(root) = std::env::var_os("DIAGNOSTIC_WIRING_CHILD") {
        lucidos_engine::internal_faults::install(std::path::Path::new(&root));
        lucidos_engine::diagnostic_log::start(std::path::Path::new(&root)).unwrap();
        if std::env::var_os("DIAGNOSTIC_FATAL_PANIC").is_some() {
            panic!("PRIVATE_FATAL_PANIC_MARKER");
        }
        lucidos_engine::log!(@Wiring, "typed count={} private={}", 7, lucidos_diagnostic_log::private("PRIVATE_TYPED_MARKER"));
        lucidos_engine::log_legacy!(@Wiring, "PRIVATE_LEGACY_MARKER");
        lucidos_engine::log_legacy!(@Wiring, "credential {}", std::env::var("CRED_DIAGNOSTIC_TEST").unwrap());
        std::io::Write::write_all(&mut std::io::stderr(), b"PRIVATE_STDERR_MARKER\n").unwrap();
        let _ = std::panic::catch_unwind(|| panic!("PRIVATE_PANIC_MARKER"));
        let path = lucidos_engine::diagnostic_log::dir_for(std::path::Path::new(&root))
            .join(lucidos_diagnostic_log::writer::FILE_NAME);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains("PRIVATE_STDERR_MARKER") && text.contains("PRIVATE_PANIC_MARKER") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "missing captures: {text}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "engine_logging_wiring", "--nocapture"])
        .env("DIAGNOSTIC_WIRING_CHILD", tmp.path())
        .env("CRED_DIAGNOSTIC_TEST", "opaque-registered-credential")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("[Wiring] typed count=7 private=PRIVATE_TYPED_MARKER"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("PRIVATE_STDERR_MARKER"));
    let text = std::fs::read_to_string(
        lucidos_engine::diagnostic_log::dir_for(tmp.path())
            .join(lucidos_diagnostic_log::writer::FILE_NAME),
    )
    .unwrap();
    assert!(!text.contains("opaque-registered-credential"));
    let records: Vec<Record> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records
        .iter()
        .any(|r| r.label == "Panic" && r.line.is_some()));
    assert!(records.iter().any(|r| r.label == "Stderr"));
    let maintainer: String = records
        .iter()
        .map(|r| r.render(Destination::Maintainer))
        .collect();
    assert!(maintainer.contains("typed count=7 private=<text"));
    assert!(!maintainer.contains("PRIVATE_"), "{maintainer}");
    for record in records {
        assert!(record.ts.ends_with('Z'));
        chrono::DateTime::parse_from_rfc3339(&record.ts).unwrap();
    }
}

#[test]
fn fatal_panics_are_written_before_the_process_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "engine_logging_wiring", "--nocapture"])
        .env("DIAGNOSTIC_WIRING_CHILD", tmp.path())
        .env("CRED_DIAGNOSTIC_TEST", "opaque-registered-credential")
        .env("DIAGNOSTIC_FATAL_PANIC", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let faults = lucidos_diagnostic_log::fault::Inbox::new(tmp.path())
        .pending()
        .unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, lucidos_diagnostic_log::fault::FaultKind::Panic);
    let text = std::fs::read_to_string(
        lucidos_engine::diagnostic_log::dir_for(tmp.path())
            .join(lucidos_diagnostic_log::writer::FILE_NAME),
    )
    .unwrap();
    let panic = text
        .lines()
        .map(|line| serde_json::from_str::<Record>(line).unwrap())
        .find(|record| record.label == "Panic")
        .expect("panic record must survive process exit");
    assert!(panic.msg.contains("PRIVATE_FATAL_PANIC_MARKER"));
    assert!(!panic
        .render(Destination::Maintainer)
        .contains("PRIVATE_FATAL_PANIC_MARKER"));
}
