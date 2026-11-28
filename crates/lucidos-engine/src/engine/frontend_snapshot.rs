use crate::codegen_inputs::SOURCE_COMMIT_FILE;
use std::path::{Path, PathBuf};

const SNAPSHOT_PARENT: &str = ".lucidos/served-frontend";

/// INVARIANT: The parent dir holding all per-generation snapshots. The refresh path uses it
/// to guard cleanup — it only ever removes a dir *under* this parent, never the
/// live `dist/` (which the boot fail-safe may serve directly).
pub(crate) fn snapshot_parent(workspace_path: &Path) -> PathBuf {
    workspace_path.join(SNAPSHOT_PARENT)
}

fn generation_dir(workspace_path: &Path, generation: u64) -> PathBuf {
    snapshot_parent(workspace_path).join(generation.to_string())
}

pub fn resolve_served_dir(static_dir: PathBuf, workspace_path: &Path) -> PathBuf {
    resolve_served_dir_inner(static_dir, workspace_path, crate::runtime::is_packaged())
}

fn resolve_served_dir_inner(static_dir: PathBuf, workspace_path: &Path, packaged: bool) -> PathBuf {
    if packaged {
        return static_dir;
    }
    let parent = snapshot_parent(workspace_path);
    match std::fs::remove_dir_all(&parent) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::log_legacy!("[Frontend] failed to clear {}: {e}", parent.display()),
    }
    match pin_generation_inner(&static_dir, workspace_path, 0) {
        Ok(snapshot) => {
            crate::log_legacy!(
                "[Frontend] Serving pinned dist snapshot at {}",
                snapshot.display()
            );
            snapshot
        }
        Err(e) => {
            crate::log_legacy!(
                "[Frontend] dist snapshot failed ({e}); serving live dir {}",
                static_dir.display()
            );
            static_dir
        }
    }
}

/// INVARIANT: Pin a fresh hardlink snapshot of `static_dir` as `<parent>/<generation>` and
/// return its path. Used at boot (generation 0) and by the frontend-only Apply
/// re-snapshot (`engine::frontend_refresh`), which picks a NEW generation so the
/// currently-served dir is untouched until the caller atomically swaps to this one.
pub(crate) fn pin_generation(
    static_dir: &Path,
    workspace_path: &Path,
    generation: u64,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    pin_generation_inner(static_dir, workspace_path, generation)
}

fn pin_generation_inner(
    static_dir: &Path,
    workspace_path: &Path,
    generation: u64,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let dst = generation_dir(workspace_path, generation);
    pin_snapshot_into(static_dir, &dst)?;
    Ok(dst)
}

/// INVARIANT: Best-effort removal of a superseded snapshot dir. Called after a swap (with a
/// grace delay) so an in-flight request that already resolved the old path can
/// finish serving from it. A failure just leaves the dir on disk until the next
/// boot clears the parent. The caller must pass a dir *under* [`snapshot_parent`]
/// — never the live `dist/`.
pub(crate) fn remove_snapshot_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::log_legacy!(
                "[Frontend] failed to remove old snapshot {}: {e}",
                dir.display()
            );
        }
    }
}

pub(crate) fn read_build_id(dir: &Path) -> Option<String> {
    let sw = std::fs::read_to_string(dir.join("sw.js")).ok()?;
    let after_eq = sw.split_once("BUILD_ID")?.1.split_once('=')?.1.trim_start();
    let mut chars = after_eq.chars();
    let quote = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    Some(chars.as_str().split_once(quote)?.0.to_string())
}

pub(crate) fn read_source_commit(dir: &Path) -> Option<String> {
    let stamp = std::fs::read_to_string(dir.join(SOURCE_COMMIT_FILE)).ok()?;
    let sha = stamp.trim();
    (sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha.to_string())
}

/// INVARIANT: Hardlink-copy `static_dir` → `dst`, replacing any prior contents of `dst`.
///
/// Hardlinks first (`cp -al`: instant, zero disk, and — crucially — the frozen
/// view survives the build-watch's atomic republish, which removes/renames the
/// live `dist/` directory but leaves the old file inodes alive while this
/// snapshot links them). Falls back to a deep copy (`cp -a`) across filesystems.
fn pin_snapshot_into(
    static_dir: &Path,
    dst: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !static_dir.join("index.html").exists() {
        return Err(format!("no index.html under {}", static_dir.display()).into());
    }
    // INVARIANT: Clear any prior contents first — a stale dst makes `cp` nest as `dst/src`
    // (same reason as the node_modules linker in spawn_context.rs).
    match std::fs::remove_dir_all(dst) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to clear {}: {e}", dst.display()).into()),
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let src = static_dir.to_string_lossy();
    let dst_str = dst.to_string_lossy();
    let hardlink = std::process::Command::new("cp")
        .args(["-al", &src, &dst_str])
        .output();
    if matches!(&hardlink, Ok(o) if o.status.success()) {
        return Ok(());
    }
    // INVARIANT: Hardlinks failed (e.g. cross-filesystem). A partial `cp -al` may have left
    // a half-populated dst; clear it again so the deep-copy fallback creates
    // dst=copy-of-src rather than the nested `dst/src` `cp` produces when dst
    // already exists (which would 404 the whole app).
    let _ = std::fs::remove_dir_all(dst);
    let copy = std::process::Command::new("cp")
        .args(["-a", &src, &dst_str])
        .output()?;
    if copy.status.success() {
        Ok(())
    } else {
        Err(format!(
            "cp -a failed: {}",
            String::from_utf8_lossy(&copy.stderr).trim()
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn packaged_serves_the_original_dir_unchanged() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>packaged</html>");
        let resolved = resolve_served_dir_inner(dist.clone(), &tmp, true);
        assert_eq!(resolved, dist, "packaged must serve the bundled dir as-is");
    }

    #[test]
    fn dev_pins_generation_zero_that_survives_source_republish() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>OLD</html>");
        write(&dist.join("assets/app.js"), "OLD-JS");

        let served = resolve_served_dir_inner(dist.clone(), &tmp, false);
        assert_ne!(
            served, dist,
            "dev must serve a pinned snapshot, not the live dir"
        );
        assert_eq!(served, generation_dir(&tmp, 0), "boot pins generation 0");
        assert_eq!(
            std::fs::read_to_string(served.join("index.html")).unwrap(),
            "<html>OLD</html>"
        );

        std::fs::remove_dir_all(&dist).unwrap();
        write(&dist.join("index.html"), "<html>NEW</html>");
        write(&dist.join("assets/app.js"), "NEW-JS");

        assert_eq!(
            std::fs::read_to_string(served.join("index.html")).unwrap(),
            "<html>OLD</html>",
            "snapshot must be frozen at boot content across a republish"
        );
        assert_eq!(
            std::fs::read_to_string(served.join("assets/app.js")).unwrap(),
            "OLD-JS"
        );
    }

    #[test]
    fn missing_source_falls_back_to_the_original_dir() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        let dist = tmp.join("dist");
        let resolved = resolve_served_dir_inner(dist.clone(), &tmp, false);
        assert_eq!(
            resolved, dist,
            "a missing/unbuilt source must fall back to the original dir, not 404"
        );
    }

    #[test]
    fn re_snapshot_into_a_new_generation_leaves_the_old_one_intact() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>v0</html>");
        write(&dist.join("sw.js"), "const BUILD_ID = 'aaaa';");

        let gen0 = resolve_served_dir_inner(dist.clone(), &tmp, false);
        assert_eq!(read_build_id(&gen0).as_deref(), Some("aaaa"));

        std::fs::remove_dir_all(&dist).unwrap();
        write(&dist.join("index.html"), "<html>v1</html>");
        write(&dist.join("sw.js"), "const BUILD_ID = \"bbbb\";");
        let gen1 = pin_generation(&dist, &tmp, 1).unwrap();
        assert_ne!(gen0, gen1, "a re-snapshot must use a fresh generation dir");
        assert_eq!(
            std::fs::read_to_string(gen1.join("index.html")).unwrap(),
            "<html>v1</html>"
        );
        assert_eq!(read_build_id(&gen1).as_deref(), Some("bbbb"));
        assert_eq!(
            std::fs::read_to_string(gen0.join("index.html")).unwrap(),
            "<html>v0</html>",
            "the previous generation must survive until the caller cleans it up"
        );

        remove_snapshot_dir(&gen0);
        assert!(
            !gen0.exists(),
            "remove_snapshot_dir drops the superseded snapshot"
        );
        assert!(gen1.exists(), "and leaves the current one");
    }

    #[test]
    fn read_build_id_handles_missing_and_unstamped() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        assert_eq!(read_build_id(&tmp), None, "no sw.js → None");
        write(&tmp.join("sw.js"), "// no marker here");
        assert_eq!(read_build_id(&tmp), None, "no BUILD_ID marker → None");
        write(&tmp.join("sw.js"), "const BUILD_ID = '__DEV__';\n// rest");
        assert_eq!(read_build_id(&tmp).as_deref(), Some("__DEV__"));
    }

    #[test]
    fn read_source_commit_takes_only_a_full_sha() {
        let scratch = tempfile::tempdir().unwrap();
        let tmp = scratch.path().to_path_buf();
        assert_eq!(read_source_commit(&tmp), None, "no stamp is unknown");
        let stamp = tmp.join(SOURCE_COMMIT_FILE);
        for bad in ["", "\n", "not a commit\n", "abc123\n", &"g".repeat(40)] {
            write(&stamp, bad);
            assert_eq!(read_source_commit(&tmp), None, "{bad:?} is unknown");
        }
        let sha = "0123456789abcdef0123456789abcdef01234567";
        write(&stamp, &format!("{sha}\n"));
        assert_eq!(read_source_commit(&tmp).as_deref(), Some(sha));
    }
}
