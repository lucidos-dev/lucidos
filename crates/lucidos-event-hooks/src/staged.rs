use std::path::Path;

use lucidos_core::core::event_hooks::{Refusal, RefusalCause};
use lucidos_core::core::slug::slugify_kebab;

pub(crate) const HOOK_RUNS_SUBPATH: &str = ".lucidos/hook-runs";

pub(crate) struct StagedHook {
    run: tempfile::TempDir,
}

impl StagedHook {
    pub(crate) fn stage(workspace: &Path, slug: &str) -> Result<Self, Refusal> {
        let changed = |why: String| refusal(RefusalCause::Changed, slug, why);
        if slugify_kebab(slug) != slug {
            return Err(changed(format!("'{slug}' is not a kebab-case slug.")));
        }
        let live = crate::hooks_root(workspace)
            .and_then(|root| crate::contained(&root, slug))
            .map_err(changed)?;
        let runs = workspace.join(HOOK_RUNS_SUBPATH);
        let run = std::fs::create_dir_all(&runs)
            .and_then(|()| tempfile::Builder::new().prefix("ask-").tempdir_in(&runs))
            .map_err(|e| {
                refusal(
                    RefusalCause::Unavailable,
                    slug,
                    format!("{HOOK_RUNS_SUBPATH} cannot hold a copy of it: {e}"),
                )
            })?;
        copy_folder(&live, &run.path().join(slug))
            .map_err(|(cause, why)| refusal(cause, slug, why))?;
        Ok(Self { run })
    }

    pub(crate) fn root(&self) -> &Path {
        self.run.path()
    }
}

pub(crate) fn clear_stale(workspace: &Path) {
    match std::fs::remove_dir_all(workspace.join(HOOK_RUNS_SUBPATH)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            lucidos_core::log_legacy!("[EventHooks] Stale copies in {HOOK_RUNS_SUBPATH} stay: {e}")
        }
    }
}

fn refusal(cause: RefusalCause, slug: &str, why: String) -> Refusal {
    Refusal {
        cause,
        reason: format!("The hook '{slug}' could not be asked: {why}"),
        detail: None,
    }
}

fn copy_folder(src: &Path, dest: &Path) -> Result<(), (RefusalCause, String)> {
    let unreadable = |e: std::io::Error| {
        (
            RefusalCause::Changed,
            format!("its folder cannot be read: {e}"),
        )
    };
    let unwritable = |e: std::io::Error| {
        (
            RefusalCause::Unavailable,
            format!("its copy cannot be written: {e}"),
        )
    };
    std::fs::create_dir(dest).map_err(unwritable)?;
    for entry in std::fs::read_dir(src).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        if entry.file_name() == crate::content_hash::FINDER_METADATA {
            continue;
        }
        let kind = entry.file_type().map_err(unreadable)?;
        let to = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_folder(&entry.path(), &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to).map_err(unwritable)?;
        } else {
            return Err((
                RefusalCause::Changed,
                format!(
                    "'{}' is a link or a special file, and a hook's folder holds plain files only.",
                    entry.file_name().to_string_lossy()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_core::core::event_hooks::{HOOKS_DIR, HOOK_DEFINITION_FILE};
    use lucidos_core::core::DATA_DIR;

    fn workspace_with_hook() -> tempfile::TempDir {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join(DATA_DIR).join(HOOKS_DIR).join("gate");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join(HOOK_DEFINITION_FILE), "name = \"Gate\"\n").unwrap();
        std::fs::write(dir.join("lib/rules.py"), "ALLOW = True\n").unwrap();
        workspace
    }

    #[test]
    fn the_copy_holds_the_whole_folder_and_leaves_with_its_ask() {
        let workspace = workspace_with_hook();
        let staged = StagedHook::stage(workspace.path(), "gate").unwrap();
        let copy = staged.root().join("gate");
        assert!(staged
            .root()
            .starts_with(workspace.path().join(HOOK_RUNS_SUBPATH)));
        assert_eq!(
            std::fs::read_to_string(copy.join("lib/rules.py")).unwrap(),
            "ALLOW = True\n"
        );
        let root = staged.root().to_path_buf();
        drop(staged);
        assert!(!root.exists(), "{root:?}");
    }

    #[test]
    fn a_later_edit_to_the_live_folder_does_not_reach_the_copy() {
        let workspace = workspace_with_hook();
        let staged = StagedHook::stage(workspace.path(), "gate").unwrap();
        let live = workspace.path().join(DATA_DIR).join(HOOKS_DIR).join("gate");
        std::fs::write(live.join("lib/rules.py"), "ALLOW = False\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(staged.root().join("gate/lib/rules.py")).unwrap(),
            "ALLOW = True\n"
        );
    }

    #[test]
    fn the_copy_holds_no_finder_metadata() {
        let workspace = workspace_with_hook();
        let live = workspace.path().join(DATA_DIR).join(HOOKS_DIR).join("gate");
        let finder = crate::content_hash::FINDER_METADATA;
        std::fs::write(live.join(finder), "print('unarmed')\n").unwrap();
        std::fs::write(live.join("lib").join(finder), "print('unarmed')\n").unwrap();
        let staged = StagedHook::stage(workspace.path(), "gate").unwrap();
        let copy = staged.root().join("gate");
        assert!(!copy.join(finder).exists());
        assert!(!copy.join("lib").join(finder).exists());
        assert!(copy.join("lib/rules.py").exists());
    }

    #[test]
    fn a_hook_that_left_refuses_as_changed() {
        let workspace = workspace_with_hook();
        let refused = StagedHook::stage(workspace.path(), "gone").err().unwrap();
        assert_eq!(refused.cause, RefusalCause::Changed, "{refused:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_refuses_as_changed() {
        let workspace = workspace_with_hook();
        let live = workspace.path().join(DATA_DIR).join(HOOKS_DIR).join("gate");
        std::os::unix::fs::symlink("/etc/hosts", live.join("lib/hosts")).unwrap();
        let refused = StagedHook::stage(workspace.path(), "gate").err().unwrap();
        assert_eq!(refused.cause, RefusalCause::Changed, "{refused:?}");
        assert!(refused.reason.contains("hosts"), "{refused:?}");
    }

    #[test]
    fn a_stopped_engines_copies_are_cleared() {
        let workspace = workspace_with_hook();
        let left = StagedHook::stage(workspace.path(), "gate").unwrap();
        let root = left.root().to_path_buf();
        std::mem::forget(left);
        clear_stale(workspace.path());
        assert!(!root.exists(), "{root:?}");
        clear_stale(workspace.path());
    }
}
