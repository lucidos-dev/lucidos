use std::path::{Path, PathBuf};
use std::process::Command;

/// INVARIANT: The crate directory, read from the environment cargo sets when it RUNS this
/// build script.
///
/// Never `env!`, which bakes the value at compile time. Two checkouts of one
/// package share a `-C metadata` hash, so a shared `CARGO_TARGET_DIR` hands
/// this compiled binary to whichever checkout builds next. A baked path then
/// names somebody else's tree, or a deleted one.
/// See docs/plans/2026-08-14-build-script-paths-and-actionable-build-failure.md.
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo when it runs a build script"),
    )
}

pub fn head_trigger_paths(project_root: &Path) -> Vec<PathBuf> {
    let git_path = |name: &str| {
        git_output(project_root, &["rev-parse", "--git-path", name])
            .map(|p| Path::new("../..").join(p))
    };

    let mut paths: Vec<PathBuf> = git_path("HEAD").into_iter().collect();
    if let Some(branch_ref) = git_output(project_root, &["symbolic-ref", "-q", "HEAD"]) {
        paths.extend(git_path(&branch_ref));
    }
    paths.extend(git_path("packed-refs").filter(|p| p.exists()));
    paths
}

pub fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}
pub const ENGINE_CRATE_INPUTS: &[&str] = &[
    "src",
    "migrations",
    "Cargo.toml",
    "build.rs",
    "build_support",
];
