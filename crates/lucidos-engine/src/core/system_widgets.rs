//! *Built-in widgets*: the reusable widgets Lucidos ships in `system-widgets/`
//! (ADR 0415). Read-only from every surface, resolved and staged like
//! `system-knowhow/`, and updated with each release.

use std::path::{Path, PathBuf};

use crate::core::shipped_dir::{resolve_shipped_dir, ShippedDir};

/// The literal behind [`BUILT_IN_WIDGET_PREFIX`], for messages built with
/// `concat!`.
macro_rules! built_in_widget_prefix {
    () => {
        "lucidos-"
    };
}
pub(crate) use built_in_widget_prefix;

/// Every built-in widget's id starts with this, and no workspace app may take
/// a new id that does.
pub const BUILT_IN_WIDGET_PREFIX: &str = built_in_widget_prefix!();

/// The shipped `system-widgets/` directory, as a bundle resource.
pub const SYSTEM_WIDGETS_DIR: ShippedDir = ShippedDir {
    name: "system-widgets",
    env_var: "LUCIDOS_SYSTEM_WIDGETS_DIR",
    log_tag: "[Apps]",
    degrades: "every built-in widget is missing (a lucidos- widget answers \"No widget exists\")",
};

/// Resolve the shipped `system-widgets/` directory at boot. See
/// [`resolve_shipped_dir`] for the order and the warnings.
pub fn resolve_system_widgets_dir(
    env_value: Option<&str>,
    repo_root: &Path,
    is_packaged: bool,
) -> (Option<PathBuf>, Option<String>) {
    resolve_shipped_dir(&SYSTEM_WIDGETS_DIR, env_value, repo_root, is_packaged)
}

/// Whether an id has the shape of a built-in widget's.
pub fn has_built_in_prefix(app_id: &str) -> bool {
    app_id.starts_with(BUILT_IN_WIDGET_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AppKind, AppManager};

    fn repo_system_widgets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../system-widgets")
    }

    /// What ships must load as what the ADR promises: a reusable widget under
    /// a `lucidos-` id, with an `index.html` to serve.
    #[test]
    fn every_shipped_built_in_is_a_reusable_widget_with_a_prefixed_id() {
        let ws = tempfile::tempdir().unwrap();
        let manager = AppManager::new(ws.path())
            .unwrap()
            .with_system_widgets(Some(repo_system_widgets()));
        let built_ins: Vec<_> = manager
            .list_reusable_widgets()
            .unwrap()
            .into_iter()
            .filter(|w| w.built_in)
            .collect();
        assert!(
            built_ins.iter().any(|w| w.id == "lucidos-sound-player"),
            "the sound player ships"
        );
        let on_disk = std::fs::read_dir(repo_system_widgets()).unwrap().count();
        assert_eq!(built_ins.len(), on_disk, "every folder loads as a built-in");
        for widget in built_ins {
            assert!(has_built_in_prefix(&widget.id), "{}", widget.id);
            assert_eq!(widget.kind, AppKind::Widget, "{}", widget.id);
            assert!(widget.reusable, "{}", widget.id);
            assert!(
                manager.get_app_path(&widget.id).is_file(),
                "{} has an index.html",
                widget.id
            );
        }
    }

    #[test]
    fn a_missing_staged_dir_warns_naming_its_env_var() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("resources/system-widgets");
        let (dir, warning) =
            resolve_system_widgets_dir(Some(missing.to_str().unwrap()), tmp.path(), true);
        assert_eq!(dir, None);
        assert!(warning.unwrap().contains("LUCIDOS_SYSTEM_WIDGETS_DIR"));
    }
}
