use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo when it runs a build script"),
    )
}

fn main() {
    let manifest_dir = manifest_dir();
    let manifest_dir = manifest_dir.as_path();
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();
    let version_file = manifest_dir.join("VERSION");
    let stamp_file = manifest_dir.join(".version_stamp");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=../../RELEASE");

    let current_head = match git_head(project_root) {
        Some(h) => h,
        None => {
            let version =
                fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
            println!("cargo:rustc-env=LUCIDOS_APP_VERSION={}", version.trim());
            tauri_build::build();
            return;
        }
    };

    let last_bumped = fs::read_to_string(&stamp_file).unwrap_or_default();
    let last_bumped = last_bumped.trim().to_string();

    if !version_file.exists() {
        let today = chrono_date_today();
        let new_version = format!("{}.0\n", today);
        fs::write(&version_file, &new_version).expect("Failed to create default VERSION");
    }

    if current_head != last_bumped {
        if last_bumped.is_empty() {
            fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        } else if has_source_changes_between(&last_bumped, &current_head, project_root) {
            let current =
                fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
            let current = current.trim();
            let today = chrono_date_today();
            let (current_date, current_patch) = parse_calver(current);

            let patch = if current_date == today {
                current_patch + 1
            } else {
                1
            };

            let new_version = format!("{}.{}", today, patch);
            fs::write(&version_file, format!("{}\n", new_version))
                .expect("Failed to write VERSION");
            fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        } else {
            fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        }
    }

    let version = fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
    println!("cargo:rustc-env=LUCIDOS_APP_VERSION={}", version.trim());

    tauri_build::build()
}

fn git_head(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn has_source_changes_between(old_commit: &str, new_commit: &str, project_root: &Path) -> bool {
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            old_commit,
            new_commit,
            "--",
            "crates/lucidos-app/src/lib.rs",
            "crates/lucidos-app/src/main.rs",
            "crates/lucidos-app/Cargo.toml",
            "crates/lucidos-app/tauri.conf.json",
        ])
        .current_dir(project_root)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let changes = String::from_utf8_lossy(&o.stdout);
            changes.trim().lines().any(|l| !l.is_empty())
        }
        _ => true,
    }
}

fn chrono_date_today() -> String {
    let output = Command::new("date")
        .args(["+%Y.%m.%d"])
        .output()
        .expect("Failed to run date command");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn parse_calver(v: &str) -> (String, u32) {
    match v.rfind('.') {
        Some(pos) => {
            let date = &v[..pos];
            let patch = v[pos + 1..].parse::<u32>().unwrap_or(0);
            (date.to_string(), patch)
        }
        None => (v.to_string(), 0),
    }
}
