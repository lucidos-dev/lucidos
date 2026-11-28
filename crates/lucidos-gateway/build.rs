use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo when it runs a build script"),
    )
}

const EMBEDDED_SOURCES: &[&str] = &["crates/lucidos-app/index.html"];

fn main() {
    let manifest_dir = manifest_dir();
    let manifest_dir = manifest_dir.as_path();
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    for path in head_trigger_paths(project_root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
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
            _ => sha,
        },
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

fn head_trigger_paths(project_root: &Path) -> Vec<PathBuf> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    };
    let git_path =
        |name: &str| git(&["rev-parse", "--git-path", name]).map(|p| Path::new("../..").join(p));

    let mut paths: Vec<PathBuf> = git_path("HEAD").into_iter().collect();
    if let Some(branch_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        paths.extend(git_path(&branch_ref));
    }
    paths.extend(git_path("packed-refs").filter(|p| p.exists()));
    paths
}

fn gateway_diff(project_root: &Path) -> Option<String> {
    let mut args = vec![
        "--no-optional-locks",
        "diff",
        "HEAD",
        "--",
        "crates/lucidos-gateway",
        "Cargo.lock",
    ];
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
