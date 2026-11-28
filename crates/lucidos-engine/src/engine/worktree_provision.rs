use std::path::{Path, PathBuf};

pub const NPM_WORKSPACE_MEMBERS: &[&str] = &["crates/lucidos-app", "packages/lucidos-sdk"];

/// INVARIANT: True if the dir looks like a real npm install — i.e. `npm install` /
/// `npm ci` ran here and wrote `.package-lock.json`. Plain directory
/// existence isn't enough: Vite creates `node_modules/.vite/` and
/// `.vite-temp/` cache dirs at runtime, which previously caused the CC
/// worktree setup to be skipped even though no packages were installed.
///
/// This marker only exists at the npm INSTALL ROOT (the repo root here). npm
/// never writes `.package-lock.json` into a workspace MEMBER's nested
/// `node_modules`. Member trees are gated on a non-dot entry instead, by
/// `member_node_modules_installed`, not on this marker.
pub fn has_install_marker(node_modules_dir: &Path) -> bool {
    node_modules_dir.join(".package-lock.json").exists()
}

/// INVARIANT: `(src, dst)` `node_modules` pairs for every workspace MEMBER that needs
/// linking into the worktree: the nested deps the root-only hardlink misses.
/// A member qualifies when it holds its own nested install in `repo_root` AND
/// the worktree copy is not already installed. The dst check makes
/// provisioning independent of the ROOT install marker: a worktree created by
/// the old root-only provisioning (root marker present, member tree absent)
/// still gets its member tree on the next spawn/resume. Members that fully
/// hoist are skipped.
///
/// Judge both sides with `member_node_modules_installed`, never with plain
/// directory existence. A member that hoists fully still grows a
/// `node_modules` holding only Vite's cache, which existence alone reads as a
/// nested install.
pub fn member_node_modules_links(repo_root: &Path, wt_path: &Path) -> Vec<(PathBuf, PathBuf)> {
    NPM_WORKSPACE_MEMBERS
        .iter()
        .filter_map(|member| {
            let src = repo_root.join(member).join("node_modules");
            let dst = wt_path.join(member).join("node_modules");
            (member_node_modules_installed(&src) && !member_node_modules_installed(&dst))
                .then_some((src, dst))
        })
        .collect()
}

fn member_node_modules_installed(node_modules_dir: &Path) -> bool {
    std::fs::read_dir(node_modules_dir)
        .map(|mut entries| {
            entries.any(|entry| {
                entry
                    .ok()
                    .map(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

async fn link_node_modules_tree(src: &Path, dst: &Path, label: &str) {
    match tokio::fs::remove_dir_all(dst).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log_legacy!(
                "[AgentSession] Skipping {} link — failed to clear stale dir: {}",
                label,
                e
            );
            return;
        }
    }
    log_legacy!("[AgentSession] Linking {} to worktree...", label);
    let hl = tokio::process::Command::new("cp")
        .args(["-al", &src.to_string_lossy(), &dst.to_string_lossy()])
        .output()
        .await;
    if matches!(&hl, Ok(o) if o.status.success()) {
        log_legacy!("[AgentSession] {} hardlinked to worktree", label);
        return;
    }
    match tokio::process::Command::new("cp")
        .args(["-a", &src.to_string_lossy(), &dst.to_string_lossy()])
        .output()
        .await
    {
        Ok(o) if o.status.success() => log_legacy!("[AgentSession] {} copied to worktree", label),
        Ok(o) => log_legacy!(
            "[AgentSession] cp {} failed: {}",
            label,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_legacy!("[AgentSession] Failed to copy {}: {}", label, e),
    }
}

pub async fn provision_generated_files(wt_path: &Path) {
    let script = wt_path.join("scripts/codegen.sh");
    if !script.is_file() {
        return;
    }
    match tokio::process::Command::new("bash")
        .arg(&script)
        .arg("--cached-only")
        .current_dir(wt_path)
        .output()
        .await
    {
        Ok(o) if o.status.success() => log_legacy!(
            "[AgentSession] generated files provisioned: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Ok(o) => log_legacy!(
            "[AgentSession] codegen --cached-only failed in worktree: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_legacy!("[AgentSession] Failed to run codegen in worktree: {}", e),
    }
}

pub async fn provision_node_modules(repo_root: &Path, wt_path: &Path) {
    let wt_node_modules = wt_path.join("node_modules");
    if !has_install_marker(&wt_node_modules) && !has_install_marker(&repo_root.join("node_modules"))
    {
        // INVARIANT: At the workspace root, so every member installs. `npm ci` wipes
        // node_modules itself, so a stale Vite cache here does no harm.
        log_legacy!("[AgentSession] No node_modules in main repo to copy, running npm ci...");
        match tokio::process::Command::new("npm")
            .args(["ci", "--prefer-offline"])
            .current_dir(wt_path)
            .output()
            .await
        {
            Ok(o) if o.status.success() => {
                log_legacy!("[AgentSession] node_modules installed in worktree");
            }
            Ok(o) => log_legacy!(
                "[AgentSession] npm ci failed in worktree: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => log_legacy!("[AgentSession] Failed to run npm ci in worktree: {}", e),
        }
    }
    link_node_modules(repo_root, wt_path).await;
}

/// INVARIANT: Hardlink the main checkout's installed `node_modules` into a worktree.
/// Links nothing the main checkout lacks or the worktree already has.
///
/// The repo is an npm workspace, so dependencies hoist to the repo root. The
/// root tree goes first: its member symlinks are relative, and Node resolution
/// walks up to `<wt>/node_modules`. Pointing the root link at a member once
/// sent every worktree to a cold `npm ci` that saturated the disk
/// (docs/plans/2026-06-27-mobile-webkit-shard-contention.md). A member tree
/// holds only what npm could not hoist. It links independently of the root
/// marker, so an older root-only worktree still gets it.
///
/// Never a symlink to the tree itself: an `npm install` in the worktree would
/// follow it and corrupt the main checkout's real `node_modules`.
pub(crate) async fn link_node_modules(repo_root: &Path, wt_path: &Path) {
    let wt_node_modules = wt_path.join("node_modules");
    let src_node_modules = repo_root.join("node_modules");
    if !has_install_marker(&wt_node_modules) && has_install_marker(&src_node_modules) {
        link_node_modules_tree(&src_node_modules, &wt_node_modules, "node_modules").await;
    }
    for (member_src, member_dst) in member_node_modules_links(repo_root, wt_path) {
        let label = member_dst
            .strip_prefix(wt_path)
            .unwrap_or(member_dst.as_path())
            .to_string_lossy()
            .into_owned();
        link_node_modules_tree(&member_src, &member_dst, &label).await;
    }
}
