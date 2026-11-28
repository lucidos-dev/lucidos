use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../lucidos-engine/build_support/paths.rs"]
mod build_paths;
use build_paths::{git_output, head_trigger_paths, manifest_dir, ENGINE_CRATE_INPUTS};
#[path = "../engine_crate_paths.rs"]
mod engine_crate_paths;
use engine_crate_paths::{engine_crates, ENGINE_INVENTORY_INPUTS};

fn main() {
    let manifest_dir = manifest_dir();
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    for path in ENGINE_INVENTORY_INPUTS {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../RELEASE");
    for name in engine_crates() {
        for input in ENGINE_CRATE_INPUTS {
            let path = root.join("crates").join(name).join(input);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    for path in head_trigger_paths(root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let build_id = compute_build_id(root);
    let release_dirty =
        git_output(root, &["rev-parse", "HEAD"]).is_some_and(|head| is_release_dirty(root, &head));
    println!("cargo:rustc-env=ENGINE_BUILD_ID={build_id}");
    println!("cargo:rustc-env=LUCIDOS_RELEASE_DIRTY={release_dirty}");
}

fn compute_build_id(project_root: &Path) -> String {
    match git_output(project_root, &["rev-parse", "--short", "HEAD"]) {
        Some(sha) => match engine_diff(project_root) {
            Some(diff) if !diff.trim().is_empty() => format!("{sha}-{:016x}", hash_str(&diff)),
            _ => sha,
        },
        None => format!("src-{:016x}", hash_sources(project_root)),
    }
}

fn engine_diff(project_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["--no-optional-locks", "diff", "HEAD", "--", "Cargo.lock"])
        .args(ENGINE_INVENTORY_INPUTS)
        .args(engine_crates().map(|name| format!("crates/{name}")))
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

fn hash_sources(root: &Path) -> u64 {
    let mut entries: Vec<_> = ENGINE_INVENTORY_INPUTS
        .iter()
        .map(|path| root.join(path))
        .collect();
    for name in engine_crates() {
        let crate_dir = root.join("crates").join(name);
        for input in ENGINE_CRATE_INPUTS {
            let path = crate_dir.join(input);
            if path.is_dir() {
                entries.extend(walk(&path));
            } else {
                entries.push(path);
            }
        }
    }
    entries.sort();
    let mut h = DefaultHasher::new();
    for path in entries {
        if let Ok(bytes) = std::fs::read(&path) {
            path.strip_prefix(root)
                .expect("source is under the repo root")
                .hash(&mut h);
            bytes.hash(&mut h);
        }
    }
    h.finish()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
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

fn is_release_dirty(project_root: &Path, head: &str) -> bool {
    git_output(project_root, &["log", "-1", "--format=%H", "--", "RELEASE"])
        .is_some_and(|commit| commit != head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_inventory_feeds_the_source_hash() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("crates")).unwrap();
        for input in ENGINE_INVENTORY_INPUTS {
            let before = hash_sources(repo.path());
            std::fs::write(repo.path().join(input), "engine inventory").unwrap();
            assert_ne!(hash_sources(repo.path()), before, "{input}");
        }
    }

    #[test]
    fn every_engine_crate_feeds_the_source_hash() {
        let repo = tempfile::tempdir().unwrap();
        for name in engine_crates() {
            let asset = repo
                .path()
                .join("crates")
                .join(name)
                .join("src/embedded.css");
            std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
            std::fs::write(&asset, "body {}").unwrap();
            let before = hash_sources(repo.path());
            std::fs::write(&asset, "html {}").unwrap();
            assert_ne!(hash_sources(repo.path()), before, "{name}");
        }
    }

    #[test]
    fn core_migrations_change_the_build_identity_without_git() {
        let repo = tempfile::tempdir().unwrap();
        let migration = repo
            .path()
            .join("crates/lucidos-core/migrations/001_example.sql");
        std::fs::create_dir_all(migration.parent().unwrap()).unwrap();
        std::fs::write(&migration, "SELECT 1;").unwrap();
        let before = hash_sources(repo.path());
        std::fs::write(&migration, "SELECT 2;").unwrap();
        assert_ne!(hash_sources(repo.path()), before);
    }

    #[test]
    fn agent_assets_change_the_build_identity_without_git() {
        let repo = tempfile::tempdir().unwrap();
        let asset = repo
            .path()
            .join("crates/lucidos-agents-api/src/api/sdk_iframe.css");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, "body {}").unwrap();
        let before = hash_sources(repo.path());
        std::fs::write(&asset, "html {}").unwrap();
        assert_ne!(hash_sources(repo.path()), before);
    }
}
