//! Test-only helpers shared across the client's modules.

use std::path::{Path, PathBuf};

/// A throwaway directory that removes itself, so a test can build a real tree
/// on disk without depending on the machine's own install.
///
/// Rolled by hand because the crate has no dev-dependency on `tempfile`, and one
/// helper is cheaper than pulling that tree in for it.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// A directory named for `tag`, unique per call.
    pub(crate) fn new(tag: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lucidos-{tag}-{unique}"));
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
