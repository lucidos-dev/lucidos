use std::fs;
use std::path::Path;
use std::process::Command;

/// Auto-bump CalVer version (YYYY.MM.DD.patch) at build time.
/// Only bumps when the git HEAD has changed AND engine source files differ
/// since the last bump. Uses a local stamp file (gitignored) to track state.
fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();
    let version_file = manifest_dir.join("VERSION");
    let stamp_file = manifest_dir.join(".version_stamp");

    // Tell cargo to re-run this build script when source files change
    // (but NOT when VERSION changes — that would create a feedback loop).
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=Cargo.toml");
    // Rebuild when the umbrella Lucidos RELEASE bumps (LUCIDOS_RELEASE is
    // baked in via include_str! in lib.rs).
    println!("cargo:rerun-if-changed=../../RELEASE");

    check_migration_versions(&manifest_dir.join("migrations"));

    // Get current git HEAD commit
    let current_head = match git_head(project_root) {
        Some(h) => h,
        None => {
            // No git available (e.g. shipped install) — assume clean release.
            println!("cargo:rustc-env=LUCIDOS_RELEASE_DIRTY=false");
            return;
        }
    };

    // Mark the build "release-dirty" if HEAD has moved past the commit that
    // last bumped RELEASE — i.e. there are local commits not yet captured in
    // the published v<RELEASE> snapshot. Surfaced as `0.7*` in the UI.
    let release_dirty = is_release_dirty(project_root, &current_head);
    println!("cargo:rustc-env=LUCIDOS_RELEASE_DIRTY={release_dirty}");

    // Read last-bumped commit hash
    let last_bumped = fs::read_to_string(&stamp_file).unwrap_or_default();
    let last_bumped = last_bumped.trim().to_string();

    // Same commit → no bump needed
    if current_head == last_bumped {
        return;
    }

    // VERSION is gitignored — it only exists in the main working directory,
    // not in worktrees. Generate a real CalVer version so builds always
    // have a meaningful version (not "0.0.0").
    if !version_file.exists() {
        let today = chrono_date_today();
        let new_version = format!("{}.0\n", today);
        fs::write(&version_file, &new_version).expect("Failed to create default VERSION");
        fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        return;
    }

    // First build in this checkout — stamp file is gitignored so it won't
    // exist. Just record HEAD without bumping VERSION.
    if last_bumped.is_empty() {
        fs::write(&stamp_file, &current_head).expect("Failed to write version stamp");
        return;
    }

    if !has_source_changes_between(&last_bumped, &current_head, project_root) {
        // Source unchanged — just update stamp
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

/// Get current git HEAD commit hash.
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

/// True if HEAD is not the commit that last modified RELEASE — meaning there
/// are commits since the umbrella release was bumped. Returns false when git
/// can't tell us (no history for RELEASE, command failure) so we default to
/// "clean" rather than displaying a misleading asterisk.
fn is_release_dirty(project_root: &Path, head: &str) -> bool {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%H", "--", "RELEASE"])
        .current_dir(project_root)
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let release_commit = String::from_utf8_lossy(&o.stdout).trim().to_string();
            !release_commit.is_empty() && release_commit != head
        }
        _ => false,
    }
}

/// Check if engine source files changed between two commits.
fn has_source_changes_between(old_commit: &str, new_commit: &str, project_root: &Path) -> bool {
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            old_commit,
            new_commit,
            "--",
            "crates/lucidos-engine/src/",
            "crates/lucidos-engine/migrations/",
            "crates/lucidos-engine/Cargo.toml",
            "Cargo.lock",
        ])
        .current_dir(project_root)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let changes = String::from_utf8_lossy(&o.stdout);
            changes.trim().lines().any(|l| !l.is_empty())
        }
        _ => true, // If git fails (e.g., force-pushed away old commit), bump to be safe
    }
}

fn chrono_date_today() -> String {
    let output = Command::new("date")
        .args(["+%Y.%m.%d"])
        .output()
        .expect("Failed to run date command");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Fail the build if two migration files share the same `YYYYMMDDHHMMSS` version
/// prefix. sqlx uses that prefix as the primary key in `_sqlx_migrations`, so a
/// collision crashes the engine on startup with a non-obvious "duplicate key"
/// error. Catching it at build time means the bad merge never makes it past CI.
fn check_migration_versions(migrations_dir: &Path) {
    let entries = match fs::read_dir(migrations_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut by_version: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        let version = match name.split_once('_') {
            Some((v, _)) => v.to_string(),
            None => continue,
        };
        by_version.entry(version).or_default().push(name);
    }
    let dupes: Vec<_> = by_version.iter().filter(|(_, v)| v.len() > 1).collect();
    if !dupes.is_empty() {
        let mut msg = String::from("Duplicate migration version prefix(es) detected:\n");
        for (version, files) in dupes {
            msg.push_str(&format!("  {}: {}\n", version, files.join(", ")));
        }
        msg.push_str(
            "Rename one to a unique YYYYMMDDHHMMSS timestamp (use scripts/new-migration.sh).",
        );
        panic!("{}", msg);
    }
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
