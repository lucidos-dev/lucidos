//! The engine's hot copy of `artifacts/user_profile.md`.
//!
//! Every chat turn renders the profile into its context and memory extraction
//! reads it, both from memory rather than from disk, so a writer that updates
//! the file without updating this cache serves a stale profile (usually the
//! empty one the section is omitted for) until the engine restarts. That was
//! real: the data API's write route refreshed nothing, so an app writing
//! through the SDK, a trigger script, or `lucidos data write` landed a profile
//! the running engine never saw.
//!
//! The rule is that every route which lands text at that exact path calls one
//! of the three methods below, which is why the path rule and the stored value
//! live in one type rather than at each call site. Today that is the data API's
//! PUT / DELETE / edit routes (so also `lucidos data write` and the SDK), the
//! `write_file` / `edit_file` / `copy_file` / `delete_file` tools, `run_python`'s
//! staged output, and `fetch_url`'s `output_path`.
//!
//! It cannot be pushed down into `ArtifactManager`, the shared `data/` store,
//! for two reasons: it is built from a workspace path alone with no route back
//! to the engine, and it is not the only sink anyway, since the file tools
//! write with `std::fs` plus `commit_file_change` and never touch it.

use std::path::Path;
use tokio::sync::RwLock;

/// The one artifact the engine keeps a live in-memory copy of, relative to
/// `data/artifacts/`.
///
/// The match against it is EXACT at every call site: `imported/user_profile.md`
/// is a different artifact (an imported copy of somebody else's profile, say)
/// and a suffix match would let it overwrite the real one.
pub(crate) const USER_PROFILE_ARTIFACT: &str = "user_profile.md";

/// The read-cache for `artifacts/user_profile.md`, owned by the engine.
pub(crate) struct UserProfileCache {
    content: RwLock<String>,
}

impl UserProfileCache {
    /// Load the profile from a workspace the way engine startup does: a missing
    /// or unreadable file leaves the cache empty, which renders as no profile
    /// section at all.
    ///
    /// This is the definition every other method is measured against, since
    /// "what the engine would serve after a restart" is what a stale cache
    /// disagrees with.
    pub(crate) fn load_from_workspace(workspace_path: &Path) -> Self {
        let content = read_from_disk(workspace_path);
        if !content.is_empty() {
            log!("[Memory] Loaded user profile ({} chars)", content.len());
        }
        Self {
            content: RwLock::new(content),
        }
    }

    /// The current profile, empty when there is none.
    pub(crate) async fn snapshot(&self) -> String {
        self.content.read().await.clone()
    }

    /// Apply an artifact write that has LANDED. `artifact_path` is relative to
    /// `data/artifacts/`; anything but the profile is a no-op.
    ///
    /// **When to call it: after the write and commit succeeded, and before any
    /// fallible announcement.** After, because a failed write must not publish
    /// content that never hit disk. Before the emit, because the cache tracks
    /// the file and not the bus: an `emit(...).await?` that trips on a
    /// transient pool timeout would otherwise carry the refresh out of the
    /// function with it, leaving exactly the stale profile this type exists to
    /// prevent.
    pub(crate) async fn artifact_written(&self, artifact_path: &str, content: &[u8]) {
        if artifact_path != USER_PROFILE_ARTIFACT {
            return;
        }
        // Stored verbatim, the same raw bytes that landed on disk: the section
        // renderer takes the file as-is, so trimming or normalising here would
        // make the running engine disagree with a restarted one. Non-UTF-8
        // content stores as empty for that same reason, since that is what
        // `load_from_workspace`'s `read_to_string` would yield next boot.
        let text = std::str::from_utf8(content).unwrap_or_default();
        *self.content.write().await = text.to_string();
    }

    /// [`Self::artifact_written`] for a writer that does not hold the content:
    /// `run_python` copies its staged output straight into `data/` and commits
    /// the paths afterwards, so the file is the only place the new profile
    /// exists. Reads the same way a restart would.
    pub(crate) async fn artifact_written_on_disk(
        &self,
        workspace_path: &Path,
        artifact_path: &str,
    ) {
        if artifact_path != USER_PROFILE_ARTIFACT {
            return;
        }
        *self.content.write().await = read_from_disk(workspace_path);
    }

    /// Apply an artifact delete that has LANDED. Deleting the profile clears
    /// the cache, matching the empty profile a restart would load.
    pub(crate) async fn artifact_deleted(&self, artifact_path: &str) {
        if artifact_path != USER_PROFILE_ARTIFACT {
            return;
        }
        self.content.write().await.clear();
    }
}

/// The profile as the filesystem holds it. A missing or unreadable file (a
/// directory in its place, non-UTF-8 bytes) reads as empty, which is the whole
/// definition of "what the engine serves after a restart".
fn read_from_disk(workspace_path: &Path) -> String {
    let path = workspace_path
        .join(crate::core::ARTIFACTS_DIR)
        .join(USER_PROFILE_ARTIFACT);
    std::fs::read_to_string(&path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace whose profile holds `content`, or none at all for `None`.
    fn workspace_with_profile(content: Option<&str>) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let artifacts = tmp.path().join(crate::core::ARTIFACTS_DIR);
        std::fs::create_dir_all(&artifacts).unwrap();
        if let Some(content) = content {
            std::fs::write(artifacts.join(USER_PROFILE_ARTIFACT), content).unwrap();
        }
        tmp
    }

    #[tokio::test]
    async fn load_from_workspace_reads_the_profile_verbatim() {
        let ws = workspace_with_profile(Some("  # Profile\n\nLikes tea.\n\n"));
        let cache = UserProfileCache::load_from_workspace(ws.path());
        assert_eq!(cache.snapshot().await, "  # Profile\n\nLikes tea.\n\n");
    }

    #[tokio::test]
    async fn load_from_workspace_is_empty_without_a_profile() {
        let ws = workspace_with_profile(None);
        let cache = UserProfileCache::load_from_workspace(ws.path());
        assert_eq!(cache.snapshot().await, "");
    }

    #[tokio::test]
    async fn writing_the_profile_replaces_the_cached_content() {
        let ws = workspace_with_profile(Some("old"));
        let cache = UserProfileCache::load_from_workspace(ws.path());

        cache
            .artifact_written(USER_PROFILE_ARTIFACT, b"# Profile\n\nNew facts.\n")
            .await;

        assert_eq!(cache.snapshot().await, "# Profile\n\nNew facts.\n");
    }

    #[tokio::test]
    async fn writing_another_artifact_leaves_the_profile_untouched() {
        let ws = workspace_with_profile(Some("mine"));
        let cache = UserProfileCache::load_from_workspace(ws.path());

        // Every near-miss the match has to reject: a nested copy, a longer
        // name ending in the profile's, and a sibling with a suffix.
        for path in [
            "notes.md",
            "imported/user_profile.md",
            "imported/contacts/user_profile.md",
            "old_user_profile.md",
            "user_profile.md.bak",
        ] {
            cache.artifact_written(path, b"theirs").await;
            assert_eq!(cache.snapshot().await, "mine", "{} touched the cache", path);
        }
    }

    /// The writer that does not hold the content (`copy_file`, `run_python`)
    /// has to end up with exactly what a restart would read.
    #[tokio::test]
    async fn a_write_seen_only_on_disk_is_reread_into_the_cache() {
        let ws = workspace_with_profile(Some("old"));
        let cache = UserProfileCache::load_from_workspace(ws.path());
        std::fs::write(
            ws.path()
                .join(crate::core::ARTIFACTS_DIR)
                .join(USER_PROFILE_ARTIFACT),
            "copied in",
        )
        .unwrap();

        cache
            .artifact_written_on_disk(ws.path(), USER_PROFILE_ARTIFACT)
            .await;

        assert_eq!(cache.snapshot().await, "copied in");
    }

    #[tokio::test]
    async fn a_write_seen_only_on_disk_ignores_another_artifact() {
        let ws = workspace_with_profile(Some("mine"));
        let cache = UserProfileCache::load_from_workspace(ws.path());
        std::fs::write(
            ws.path()
                .join(crate::core::ARTIFACTS_DIR)
                .join(USER_PROFILE_ARTIFACT),
            "changed behind our back",
        )
        .unwrap();

        // The profile file did change, but this write was not of the profile,
        // so it is not this call's business to publish it.
        cache
            .artifact_written_on_disk(ws.path(), "imported/user_profile.md")
            .await;

        assert_eq!(cache.snapshot().await, "mine");
    }

    #[tokio::test]
    async fn deleting_the_profile_clears_the_cache() {
        let ws = workspace_with_profile(Some("mine"));
        let cache = UserProfileCache::load_from_workspace(ws.path());

        cache.artifact_deleted(USER_PROFILE_ARTIFACT).await;

        assert_eq!(cache.snapshot().await, "");
    }

    #[tokio::test]
    async fn deleting_another_artifact_leaves_the_profile_untouched() {
        let ws = workspace_with_profile(Some("mine"));
        let cache = UserProfileCache::load_from_workspace(ws.path());

        cache.artifact_deleted("imported/user_profile.md").await;

        assert_eq!(cache.snapshot().await, "mine");
    }

    #[tokio::test]
    async fn a_non_utf8_write_caches_what_a_restart_would_load() {
        let ws = workspace_with_profile(Some("mine"));
        let cache = UserProfileCache::load_from_workspace(ws.path());
        let invalid = [0x23, 0x20, 0xff, 0xfe];

        cache
            .artifact_written(USER_PROFILE_ARTIFACT, &invalid)
            .await;

        std::fs::write(
            ws.path()
                .join(crate::core::ARTIFACTS_DIR)
                .join(USER_PROFILE_ARTIFACT),
            invalid,
        )
        .unwrap();
        let after_restart = UserProfileCache::load_from_workspace(ws.path());
        assert_eq!(cache.snapshot().await, after_restart.snapshot().await);
    }
}
