use std::fs;
use std::path::Path;
use std::process::Command;

/// Auto-bump CalVer version (YYYY.MM.DD.patch) at build time for the Tauri app.
/// Only bumps when the git HEAD has changed AND app source files differ
/// since the last bump.
fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();
    let version_file = manifest_dir.join("VERSION");
    let stamp_file = manifest_dir.join(".version_stamp");

    // Tell cargo to re-run when app source files change
    // (but NOT when VERSION changes — that would create a feedback loop).
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    // Rebuild when the umbrella Lucidos RELEASE bumps so the engine binary
    // bundled with the app picks up the new release string.
    println!("cargo:rerun-if-changed=../../RELEASE");

    // Get current git HEAD commit
    let current_head = match git_head(project_root) {
        Some(h) => h,
        None => {
            // Embed current version even without git
            let version =
                fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
            println!("cargo:rustc-env=COGNOS_APP_VERSION={}", version.trim());
            tauri_build::build();
            return;
        }
    };

    // Read last-bumped commit hash
    let last_bumped = fs::read_to_string(&stamp_file).unwrap_or_default();
    let last_bumped = last_bumped.trim().to_string();

    // VERSION is gitignored — it only exists in the main working directory,
    // not in worktrees. Generate a real CalVer version so builds always
    // have a meaningful version (not "0.0.0").
    if !version_file.exists() {
        let today = chrono_date_today();
        let new_version = format!("{}.0\n", today);
        fs::write(&version_file, &new_version).expect("Failed to create default VERSION");
    }

    if current_head != last_bumped {
        if last_bumped.is_empty() {
            // First build — just record HEAD without bumping VERSION.
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
            // Source unchanged — just update stamp so we don't re-check these commits
            fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        }
    }

    // Embed version AFTER bumping so the compiled binary gets the new version
    let version = fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
    println!("cargo:rustc-env=COGNOS_APP_VERSION={}", version.trim());

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

/// Check if Tauri app Rust/config files changed between two commits.
/// Must match the rerun-if-changed directives above — only files that affect
/// the compiled binary, not frontend TS/CSS (served as static files).
fn has_source_changes_between(old_commit: &str, new_commit: &str, project_root: &Path) -> bool {
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            old_commit,
            new_commit,
            "--",
            "crates/cognos-app/src/lib.rs",
            "crates/cognos-app/src/main.rs",
            "crates/cognos-app/Cargo.toml",
            "crates/cognos-app/tauri.conf.json",
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
