//! Dev-only frontend pinning (INV-A, `docs/plans/2026-07-02-serve-only-engine-compatible-client.md`).
//!
//! In dev the engine serves the built `dist/` (`LUCIDOS_STATIC_DIR`), but the
//! checkout-shared build-watch republishes `dist/` on any source change while the
//! engine binary rebuild lags — so the running engine would serve a NEWER client
//! than itself, which a hard reload then loads onto the still-old engine (new
//! endpoint / event / migration mismatch → breakage).
//!
//! To guarantee the engine only ever serves the client it was built against, this
//! snapshots `dist/` into a private per-workspace dir at boot and serves THAT.
//! The shared `dist/` keeps advancing; a *Switch to new version* respawns the
//! engine, and the new process snapshots the then-current `dist/`, so client and
//! engine advance together.
//!
//! **Frontend-only advance** (`docs/plans/2026-07-02-frontend-only-apply-served-in-dev.md`):
//! the boot pin is not frozen forever. On a *frontend-only* Apply the engine
//! binary is unchanged, so a newer client built from that diff is compatible —
//! the engine re-snapshots `dist/` into a fresh generation dir and swaps what it
//! serves IN-PROCESS (no respawn), so the client refresh badge/toast surface
//! without a manual restart. The served dir is therefore addressed by
//! *generation*: `<workspace>/.lucidos/served-frontend/<gen>/` (gen 0 at boot,
//! incremented per re-snapshot). See `engine::frontend_refresh`.
//!
//! Packaged ships engine + frontend as one immutable unit (no build-watch), so it
//! already satisfies the invariant — the snapshot is skipped and Resources are
//! served directly. Best-effort: any snapshot failure falls back to serving the
//! original dir (today's behavior), never a 404.

use std::path::{Path, PathBuf};

/// Parent dir holding the per-generation snapshots (under `.lucidos/`, gitignored,
/// ephemeral runtime that can be rebuilt). Each snapshot lives in a numbered
/// subdir so a runtime re-snapshot never clobbers the dir currently being served.
const SNAPSHOT_PARENT: &str = ".lucidos/served-frontend";

/// The parent dir holding all per-generation snapshots. The refresh path uses it
/// to guard cleanup — it only ever removes a dir *under* this parent, never the
/// live `dist/` (which the boot fail-safe may serve directly).
pub(crate) fn snapshot_parent(workspace_path: &Path) -> PathBuf {
    workspace_path.join(SNAPSHOT_PARENT)
}

/// Path of the snapshot for a given generation: `<parent>/<generation>`.
fn generation_dir(workspace_path: &Path, generation: u64) -> PathBuf {
    snapshot_parent(workspace_path).join(generation.to_string())
}

/// Resolve the directory the engine should actually serve the frontend from at
/// boot. Packaged → `static_dir` unchanged (bundled, immutable). Dev → a fresh
/// hardlink snapshot pinned as generation 0 (INV-A).
pub fn resolve_served_dir(static_dir: PathBuf, workspace_path: &Path) -> PathBuf {
    resolve_served_dir_inner(static_dir, workspace_path, crate::runtime::is_packaged())
}

/// Pure core of [`resolve_served_dir`] with the packaged decision passed in, so
/// it's unit-testable without mutating the process environment.
fn resolve_served_dir_inner(static_dir: PathBuf, workspace_path: &Path, packaged: bool) -> PathBuf {
    if packaged {
        return static_dir;
    }
    // Clear any leftover snapshots from a previous process before pinning gen 0,
    // so stale generation dirs don't accumulate across restarts.
    let parent = snapshot_parent(workspace_path);
    match std::fs::remove_dir_all(&parent) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::log!("[Frontend] failed to clear {}: {e}", parent.display()),
    }
    match pin_generation_inner(&static_dir, workspace_path, 0) {
        Ok(snapshot) => {
            crate::log!(
                "[Frontend] Serving pinned dist snapshot at {}",
                snapshot.display()
            );
            snapshot
        }
        Err(e) => {
            // Fail-safe: degrade to serving the live dir (today's behavior),
            // never a 404. The invariant is best-effort in this rare case.
            crate::log!(
                "[Frontend] dist snapshot failed ({e}); serving live dir {}",
                static_dir.display()
            );
            static_dir
        }
    }
}

/// Pin a fresh hardlink snapshot of `static_dir` as `<parent>/<generation>` and
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

/// Best-effort removal of a superseded snapshot dir. Called after a swap (with a
/// grace delay) so an in-flight request that already resolved the old path can
/// finish serving from it. A failure just leaves the dir on disk until the next
/// boot clears the parent. The caller must pass a dir *under* [`snapshot_parent`]
/// — never the live `dist/`.
pub(crate) fn remove_snapshot_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::log!(
                "[Frontend] failed to remove old snapshot {}: {e}",
                dir.display()
            );
        }
    }
}

/// The stamped `BUILD_ID` in `<dir>/sw.js`, or `None` if `sw.js` is missing /
/// unstamped (the `__…__` dev placeholder is returned as-is; the caller treats a
/// placeholder like any other value). Mirrors the frontend's `SW_BUILD_ID_RE`
/// (`hooks/sw-update.ts`): `const BUILD_ID = '<hex>';`.
pub(crate) fn read_build_id(dir: &Path) -> Option<String> {
    let sw = std::fs::read_to_string(dir.join("sw.js")).ok()?;
    // `BUILD_ID = '<value>'` / `"<value>"`. Small hand-parse via split_once — no
    // regex dep and no byte-index slicing (CLAUDE.md). Mirrors SW_BUILD_ID_RE.
    let after_eq = sw.split_once("BUILD_ID")?.1.split_once('=')?.1.trim_start();
    let mut chars = after_eq.chars();
    let quote = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    Some(chars.as_str().split_once(quote)?.0.to_string())
}

/// Hardlink-copy `static_dir` → `dst`, replacing any prior contents of `dst`.
///
/// Hardlinks first (`cp -al`: instant, zero disk, and — crucially — the frozen
/// view survives the build-watch's atomic republish, which removes/renames the
/// live `dist/` directory but leaves the old file inodes alive while this
/// snapshot links them). Falls back to a deep copy (`cp -a`) across filesystems.
fn pin_snapshot_into(
    static_dir: &Path,
    dst: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Guard: an empty/absent source (frontend not built yet) → let the caller
    // fall back to serving the original dir rather than pinning an empty tree.
    if !static_dir.join("index.html").exists() {
        return Err(format!("no index.html under {}", static_dir.display()).into());
    }
    // Clear any prior contents first — a stale dst makes `cp` nest as `dst/src`
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
    // Hardlinks failed (e.g. cross-filesystem). A partial `cp -al` may have left
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
        let tmp = std::env::temp_dir().join(format!("lucidos-fe-snap-pkg-{}", std::process::id()));
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>packaged</html>");
        let resolved = resolve_served_dir_inner(dist.clone(), &tmp, /* packaged */ true);
        assert_eq!(resolved, dist, "packaged must serve the bundled dir as-is");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn dev_pins_generation_zero_that_survives_source_republish() {
        let tmp = std::env::temp_dir().join(format!("lucidos-fe-snap-dev-{}", std::process::id()));
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>OLD</html>");
        write(&dist.join("assets/app.js"), "OLD-JS");

        let served = resolve_served_dir_inner(dist.clone(), &tmp, /* packaged */ false);
        assert_ne!(
            served, dist,
            "dev must serve a pinned snapshot, not the live dir"
        );
        assert_eq!(served, generation_dir(&tmp, 0), "boot pins generation 0");
        assert_eq!(
            std::fs::read_to_string(served.join("index.html")).unwrap(),
            "<html>OLD</html>"
        );

        // Simulate the build-watch's atomic republish: remove the live dist and
        // recreate it with NEW content (fresh inodes). The pinned snapshot must
        // still read the OLD content (INV-A: never serve a newer client).
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
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_source_falls_back_to_the_original_dir() {
        let tmp = std::env::temp_dir().join(format!("lucidos-fe-snap-miss-{}", std::process::id()));
        let dist = tmp.join("dist"); // never created — no index.html
        let resolved = resolve_served_dir_inner(dist.clone(), &tmp, /* packaged */ false);
        assert_eq!(
            resolved, dist,
            "a missing/unbuilt source must fall back to the original dir, not 404"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn re_snapshot_into_a_new_generation_leaves_the_old_one_intact() {
        let tmp = std::env::temp_dir().join(format!("lucidos-fe-snap-gen-{}", std::process::id()));
        let dist = tmp.join("dist");
        write(&dist.join("index.html"), "<html>v0</html>");
        write(&dist.join("sw.js"), "const BUILD_ID = 'aaaa';");

        let gen0 = resolve_served_dir_inner(dist.clone(), &tmp, false);
        assert_eq!(read_build_id(&gen0).as_deref(), Some("aaaa"));

        // Source rebuilt — the build-watch's atomic publish removes/renames the
        // live dir and writes fresh inodes, so `gen0`'s hardlinks keep the OLD
        // content. Simulate that (remove-then-recreate), then re-snapshot as gen 1.
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
        // The old generation is still fully readable (no remove-during-serve race).
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
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn read_build_id_handles_missing_and_unstamped() {
        let tmp = std::env::temp_dir().join(format!("lucidos-fe-snap-bid-{}", std::process::id()));
        assert_eq!(read_build_id(&tmp), None, "no sw.js → None");
        write(&tmp.join("sw.js"), "// no marker here");
        assert_eq!(read_build_id(&tmp), None, "no BUILD_ID marker → None");
        write(&tmp.join("sw.js"), "const BUILD_ID = '__DEV__';\n// rest");
        assert_eq!(read_build_id(&tmp).as_deref(), Some("__DEV__"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
