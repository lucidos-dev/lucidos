use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;

/// Bake a `GATEWAY_BUILD_ID` into the binary so a running gateway can tell whether
/// the on-disk binary it was launched from has since been rebuilt with different
/// source (the workspace picker's "new gateway available" badge — see
/// `docs/plans/2026-06-18-gateway-reload-control.md`).
///
/// The id is **deterministic for identical source** (a no-op rebuild must NOT
/// raise the badge): git short SHA, plus — when the working tree has uncommitted
/// gateway-source changes — a short hash of that diff so local edits produce a
/// distinct id too. When git is unavailable (a shipped install built outside a
/// repo) we fall back to a hash of the crate's own source so the id is at least
/// stable per build tree.
fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();

    // Re-run when gateway source, the manifest, or git state changes. `.git/HEAD`
    // covers commits/checkouts; `.git/index` covers staging so the dirty-diff
    // component stays fresh.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    // `Cargo.lock` feeds the dirty-diff component below, so watch it too — else an
    // uncommitted lock-only change wouldn't recompute the build id.
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let build_id = compute_build_id(project_root, manifest_dir);
    println!("cargo:rustc-env=GATEWAY_BUILD_ID={build_id}");
}

fn compute_build_id(project_root: &Path, manifest_dir: &Path) -> String {
    match git_short_head(project_root) {
        Some(sha) => match gateway_diff(project_root) {
            Some(diff) if !diff.trim().is_empty() => {
                format!("{sha}-{:016x}", hash_str(&diff))
            }
            // Clean tree (or git couldn't diff) → the commit alone identifies it.
            _ => sha,
        },
        // No git (shipped build) → hash the crate source so it's stable per tree.
        None => format!("src-{:016x}", hash_dir_sources(&manifest_dir.join("src"))),
    }
}

fn git_short_head(project_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    } else {
        None
    }
}

/// Uncommitted changes (staged + unstaged) to gateway-relevant paths. `None` when
/// git fails — the caller then treats the tree as clean rather than inventing a
/// dirty marker.
fn gateway_diff(project_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args([
            "diff",
            "HEAD",
            "--",
            "crates/lucidos-gateway",
            "Cargo.lock",
        ])
        .current_dir(project_root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn hash_str(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Hash every file under `dir` (sorted for determinism) — the no-git fallback.
fn hash_dir_sources(dir: &Path) -> u64 {
    let mut entries: Vec<_> = walk(dir);
    entries.sort();
    let mut h = DefaultHasher::new();
    for path in entries {
        if let Ok(bytes) = std::fs::read(&path) {
            path.to_string_lossy().hash(&mut h);
            bytes.hash(&mut h);
        }
    }
    h.finish()
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}
