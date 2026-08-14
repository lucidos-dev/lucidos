use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The crate directory, read from the environment cargo sets when it RUNS this
/// build script. Hand-synced with the engine and app build scripts.
///
/// Never `env!`, which bakes the value at compile time. Two checkouts of one
/// package share a `-C metadata` hash, so a shared `CARGO_TARGET_DIR` hands
/// this compiled binary to whichever checkout builds next. A baked path then
/// names somebody else's tree, or a deleted one. Here that failure is SILENT:
/// git cannot run in a missing directory, so every build id collapses to one
/// constant and the picker's new-gateway badge stops firing.
/// See docs/plans/2026-08-14-build-script-paths-and-actionable-build-failure.md.
fn manifest_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo when it runs a build script"),
    )
}

/// Files outside this crate that are compiled INTO the gateway binary, so they
/// are gateway source for build-id purposes even though they live elsewhere.
/// Repo-root-relative (the `git diff` pathspec); the rerun triggers in `main`
/// say the same paths relative to this crate.
///
/// `crates/lucidos-app/index.html` is `include_str!`d by `proxy.rs`, which lifts
/// the boot splash out of it. Leave it out and an UNCOMMITTED edit rebuilds the
/// binary with a new splash but the same id, so the picker never offers the
/// reload and the running gateway keeps serving the old one with no signal.
const EMBEDDED_SOURCES: &[&str] = &["crates/lucidos-app/index.html"];

/// Bake a `GATEWAY_BUILD_ID` into the binary so a running gateway can tell whether
/// the on-disk binary it was launched from has since been rebuilt with different
/// source (the workspace picker's "new gateway available" badge — see
/// `docs/plans/2026-06-18-gateway-reload-control.md`).
///
/// The id is **deterministic for identical source** (a no-op rebuild must NOT
/// raise the badge): git short SHA, plus — when the working tree has uncommitted
/// gateway-source changes — a short hash of that diff so local edits produce a
/// distinct id too. When git is unavailable (a shipped install built outside a
/// repo) we fall back to a hash of the compiled-in source so the id is at least
/// stable per build tree.
fn main() {
    let manifest_dir = manifest_dir();
    let manifest_dir = manifest_dir.as_path();
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
    // Declaring any trigger opts out of cargo's default "rerun on any change in
    // the package", so an embedded file needs its own: rustc's own dep-info
    // recompiles the crate when it changes, but only this brings the id along.
    for rel in EMBEDDED_SOURCES {
        println!("cargo:rerun-if-changed=../../{rel}");
    }

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
        // No git (shipped build) → hash the source so it's stable per tree.
        None => format!("src-{:016x}", hash_build_inputs(manifest_dir, project_root)),
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
    let mut args = vec!["diff", "HEAD", "--", "crates/lucidos-gateway", "Cargo.lock"];
    args.extend_from_slice(EMBEDDED_SOURCES);
    let out = Command::new("git")
        .args(&args)
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

/// Hash everything compiled into the binary: every file under the crate's `src`
/// (sorted for determinism) plus the embedded sources. The no-git fallback.
fn hash_build_inputs(manifest_dir: &Path, project_root: &Path) -> u64 {
    let mut entries: Vec<_> = walk(&manifest_dir.join("src"));
    entries.sort();
    entries.extend(EMBEDDED_SOURCES.iter().map(|rel| project_root.join(rel)));
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
