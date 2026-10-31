//! Symlink containment for a path joined under a mount root.
//!
//! [`super::is_path_traversal`] is a string check. It refuses `..` and an
//! absolute path, but it cannot see a symlink, so `artifacts/leak -> ~/.ssh/id`
//! passes it. These helpers resolve the path on disk and refuse one that lands
//! outside its root.

use std::path::{Path, PathBuf};

/// `root.join(rel)`, or `None` when that path resolves outside `root`.
pub fn join_contained(root: &Path, rel: &str) -> Option<PathBuf> {
    let path = root.join(rel);
    (!resolves_outside(root, &path)).then_some(path)
}

/// `data_dir.join(rel)`, or `None` when a symlink carries it out of the
/// top-level tree it is filed under. A link in `artifacts/` must reach neither
/// a host file nor `data/.env`. The tree itself must not link out of `data/`:
/// only the whole `data/` moves, never one tree alone.
pub fn join_data_contained(data_dir: &Path, rel: &str) -> Option<PathBuf> {
    let (tree, within) = rel.split_once('/').unwrap_or((rel, ""));
    let tree_root = data_dir.join(tree);
    if resolves_outside(data_dir, &tree_root) {
        return None;
    }
    if within.is_empty() {
        return Some(tree_root);
    }
    join_contained(&tree_root, within)
}

/// How many dangling links one check follows before it calls the path an
/// escape. A loop of links never resolves, so it needs a bound.
const MAX_DANGLING_HOPS: u8 = 8;

/// Whether `path`, a path under `root`, reaches outside `root` once every
/// symlink on it resolves.
///
/// The deepest part of `path` that exists decides. So a write target with
/// missing directories is judged by the directory it would land in. A link
/// that does not resolve is judged by where it points, since a write would
/// follow it there.
///
/// Both sides resolve, because the workspace itself may sit behind a symlink
/// (`/var` is `/private/var` on macOS). A root that does not exist holds
/// nothing to escape through, unless the root itself is a dangling link.
pub fn resolves_outside(root: &Path, path: &Path) -> bool {
    let Ok(canonical_root) = root.canonicalize() else {
        return std::fs::symlink_metadata(root).is_ok();
    };
    escapes(&canonical_root, path, MAX_DANGLING_HOPS)
}

fn escapes(canonical_root: &Path, path: &Path, hops_left: u8) -> bool {
    // A trailing `/` or `/.` would make the lstat below follow a final link.
    let path: PathBuf = path.components().collect();
    let path = path.as_path();
    if climbs_out_of_missing_dir(path) {
        return true;
    }
    for candidate in path.ancestors() {
        let Ok(meta) = std::fs::symlink_metadata(candidate) else {
            continue;
        };
        if let Ok(resolved) = candidate.canonicalize() {
            return !resolved.starts_with(canonical_root);
        }
        if !meta.file_type().is_symlink() || hops_left == 0 {
            return true;
        }
        let Ok(target) = std::fs::read_link(candidate) else {
            return true;
        };
        let rest = path.strip_prefix(candidate).unwrap_or(Path::new(""));
        let base = candidate.parent().unwrap_or(Path::new("/"));
        return escapes(canonical_root, &base.join(target).join(rest), hops_left - 1);
    }
    true
}

/// Whether a `..` follows a directory that does not exist yet. Nothing can
/// resolve it now, and `create_dir_all` would build the missing part and
/// then climb out of it. Only a link target carries `..` this far.
fn climbs_out_of_missing_dir(path: &Path) -> bool {
    let mut prefix = PathBuf::new();
    let mut missing = false;
    for component in path.components() {
        prefix.push(component);
        if component == std::path::Component::ParentDir && missing {
            return true;
        }
        if !missing && std::fs::symlink_metadata(&prefix).is_err() {
            missing = true;
        }
    }
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data/artifacts");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::fs::write(root.join("note.md"), "note").unwrap();
        Fixture {
            _tmp: tmp,
            root,
            outside,
        }
    }

    #[test]
    fn a_plain_file_and_a_missing_one_stay_inside() {
        let f = fixture();
        assert!(join_contained(&f.root, "note.md").is_some());
        assert!(join_contained(&f.root, "missing.md").is_some());
        assert!(join_contained(&f.root, "new/dir/shot.png").is_some());
    }

    #[test]
    fn a_file_link_out_of_the_root_is_refused() {
        let f = fixture();
        symlink(f.outside.join("secret.txt"), f.root.join("leak")).unwrap();
        assert!(join_contained(&f.root, "leak").is_none());
    }

    #[test]
    fn a_directory_link_out_of_the_root_is_refused_for_reads_and_writes() {
        let f = fixture();
        symlink(&f.outside, f.root.join("dir")).unwrap();
        assert!(join_contained(&f.root, "dir/secret.txt").is_none());
        assert!(join_contained(&f.root, "dir/not-yet/shot.png").is_none());
    }

    #[test]
    fn a_dangling_link_out_of_the_root_is_refused() {
        let f = fixture();
        symlink(f.outside.join("not-yet.png"), f.root.join("shot.png")).unwrap();
        assert!(join_contained(&f.root, "shot.png").is_none());
    }

    /// A missing file is not a security error, even behind a link.
    #[test]
    fn a_dangling_link_that_stays_inside_the_root_is_allowed() {
        let f = fixture();
        std::fs::create_dir_all(f.root.join("docs")).unwrap();
        symlink("../build/out.md", f.root.join("docs/latest")).unwrap();
        assert!(join_contained(&f.root, "docs/latest").is_some());
    }

    #[test]
    fn a_dangling_link_that_climbs_out_of_a_missing_dir_is_refused() {
        let f = fixture();
        symlink("missing/../../../outside/x", f.root.join("sneaky")).unwrap();
        assert!(join_contained(&f.root, "sneaky").is_none());
    }

    #[test]
    fn a_trailing_slash_never_hides_a_dangling_link() {
        let f = fixture();
        symlink(f.outside.join("made"), f.root.join("ts")).unwrap();
        symlink("ts/", f.root.join("tl")).unwrap();
        symlink("ts/.", f.root.join("td")).unwrap();
        assert!(join_contained(&f.root, "ts/").is_none());
        assert!(join_contained(&f.root, "tl").is_none());
        assert!(join_contained(&f.root, "td").is_none());
    }

    #[test]
    fn a_dangling_link_loop_is_refused() {
        let f = fixture();
        symlink("b", f.root.join("a")).unwrap();
        symlink("a", f.root.join("b")).unwrap();
        assert!(join_contained(&f.root, "a").is_none());
    }

    #[test]
    fn a_link_that_stays_inside_the_root_is_served() {
        let f = fixture();
        symlink(f.root.join("note.md"), f.root.join("alias.md")).unwrap();
        assert!(join_contained(&f.root, "alias.md").is_some());
    }

    /// The root resolving elsewhere must not make every file look outside.
    #[test]
    fn a_root_behind_a_symlink_still_serves_its_own_files() {
        let f = fixture();
        let linked_root = f.outside.join("linked-root");
        symlink(&f.root, &linked_root).unwrap();
        assert!(join_contained(&linked_root, "note.md").is_some());
    }

    #[test]
    fn a_data_path_stays_inside_its_own_tree() {
        let f = fixture();
        let data = f.root.parent().unwrap();
        std::fs::write(data.join(".env"), "KEY=1").unwrap();
        symlink(data.join(".env"), f.root.join("env")).unwrap();

        assert!(join_data_contained(data, "artifacts/note.md").is_some());
        assert!(join_data_contained(data, "artifacts/env").is_none());
        assert!(join_data_contained(data, "artifacts").is_some());
        let linked_out = f.outside.join("linked-out");
        std::fs::create_dir_all(&linked_out).unwrap();
        symlink(&linked_out, data.join("knowhow")).unwrap();
        assert!(join_data_contained(data, "knowhow").is_none());
        assert!(join_data_contained(data, "knowhow/how-to.md").is_none());
    }

    #[test]
    fn a_missing_root_holds_nothing_to_escape_through() {
        let f = fixture();
        assert!(join_contained(&f.root.join("absent"), "x.md").is_some());
        let dangling_root = f.root.join("dangling-root");
        symlink(f.outside.join("nowhere"), &dangling_root).unwrap();
        assert!(join_contained(&dangling_root, "x.md").is_none());
    }
}
