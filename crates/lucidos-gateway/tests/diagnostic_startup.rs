use lucidos_diagnostic_log::{writer::Record, Destination};
use std::process::Command;

fn isolated(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lucidos-gateway"));
    cmd.env_clear()
        .current_dir(root)
        .env("HOME", root)
        .env("SHELL", "/bin/sh")
        .env("PATH", "/usr/bin:/bin")
        .env("LUCIDOS_PRIVATE_DIR", root.join("private"))
        .env("LUCIDOS_GATEWAY_DATA", root.join("gateway"));
    cmd
}

#[test]
fn a_real_boot_failure_is_recorded_without_starting_an_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let output = isolated(tmp.path())
        .env(
            "LUCIDOS_ENGINE_BIN",
            tmp.path().join("PRIVATE_MISSING_ENGINE_MARKER"),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("[gateway] boot failed:"));
    let text = std::fs::read_to_string(
        tmp.path()
            .join("gateway/diagnostic-log")
            .join(lucidos_diagnostic_log::writer::FILE_NAME),
    )
    .unwrap();
    let records: Vec<Record> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records
        .iter()
        .any(|r| r.msg.contains("PRIVATE_MISSING_ENGINE_MARKER")));
    for record in records {
        assert!(record.ts.ends_with('Z'));
        chrono::DateTime::parse_from_rfc3339(&record.ts).unwrap();
        assert!(!record
            .render(Destination::Maintainer)
            .contains("PRIVATE_MISSING_ENGINE_MARKER"));
    }
}

#[test]
fn metadata_probes_are_bare_and_open_no_data_directory() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["--version", "-V", "--build-id"] {
        let output = isolated(tmp.path()).arg(flag).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.lines().count(), 1);
        assert!(!stdout.contains("[pid:"));
        assert!(!stdout.trim().is_empty());
    }
    assert!(!tmp.path().join("gateway").exists());
    assert!(!tmp.path().join("private").exists());
}
