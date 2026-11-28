use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "build_support/paths.rs"]
mod build_paths;
use build_paths::{git_output, head_trigger_paths, manifest_dir, ENGINE_CRATE_INPUTS};
#[path = "../engine_crate_paths.rs"]
mod engine_crate_paths;
use engine_crate_paths::{engine_crates, ENGINE_INVENTORY_INPUTS};

fn main() {
    let manifest_dir = manifest_dir();
    let manifest_dir = manifest_dir.as_path();
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();
    let version_file = manifest_dir.join("VERSION");
    let stamp_file = manifest_dir.join(".version_stamp");

    // INVARIANT: CalVer compares commits. Watching worktree sources here would let
    // an edit in an upstream crate invalidate the library below it.
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    for path in head_trigger_paths(project_root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // INVARIANT: The engine executable embeds VERSION. Generate it before returning even
    // when git is unavailable, as can happen in the Linux packaging container.
    if !version_file.exists() {
        let fallback = format!("{}.0\n", chrono_date_today());
        fs::write(&version_file, &fallback).expect("Failed to create default VERSION");
    }

    let current_head = match git_output(project_root, &["rev-parse", "HEAD"]) {
        Some(h) => h,
        None => {
            return;
        }
    };

    let last_bumped = fs::read_to_string(&stamp_file).unwrap_or_default();
    let last_bumped = last_bumped.trim().to_string();

    if current_head == last_bumped {
        return;
    }

    if last_bumped.is_empty() {
        fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        return;
    }

    if !has_source_changes_between(&last_bumped, &current_head, project_root) {
        fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        return;
    }

    let current = fs::read_to_string(&version_file).unwrap_or_else(|_| "0000.00.00.0".into());
    let current = current.trim().to_string();

    let today = chrono_date_today();
    let (current_date, current_patch) = parse_calver(&current);

    let patch = if current_date == today {
        current_patch + 1
    } else {
        1
    };

    let new_version = format!("{}.{}", today, patch);
    fs::write(&version_file, format!("{}\n", new_version)).expect("Failed to write VERSION");
    fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
}

fn has_source_changes_between(old_commit: &str, new_commit: &str, project_root: &Path) -> bool {
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            old_commit,
            new_commit,
            "--",
            "Cargo.lock",
        ])
        .args(ENGINE_INVENTORY_INPUTS)
        .args(engine_crates().flat_map(|name| {
            ENGINE_CRATE_INPUTS
                .iter()
                .map(move |input| format!("crates/{name}/{input}"))
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_migrations_count_as_engine_source_changes() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        git(&["commit", "--quiet", "--allow-empty", "-m", "base"]);
        let migration = repo
            .path()
            .join("crates/lucidos-core/migrations/001_example.sql");
        fs::create_dir_all(migration.parent().unwrap()).unwrap();
        fs::write(migration, "SELECT 1;").unwrap();
        git(&["add", "crates/lucidos-core"]);
        git(&["commit", "--quiet", "-m", "migration"]);
        assert!(has_source_changes_between("HEAD^", "HEAD", repo.path()));
    }
}
