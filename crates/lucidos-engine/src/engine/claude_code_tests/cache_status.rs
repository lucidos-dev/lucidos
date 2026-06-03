use super::*;


fn cache_with(entries: &[(&str, &[&str])]) -> HashMap<String, CcCommandsInfo> {
    entries
        .iter()
        .map(|(path, skills)| {
            (
                (*path).to_string(),
                CcCommandsInfo {
                    builtin_commands: vec![],
                    skill_commands: skills.iter().map(|s| (*s).to_string()).collect(),
                },
            )
        })
        .collect()
}

#[test]
fn lookup_returns_cached_entry_for_matching_repo() {
    let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
    let info = lookup_repo_commands_in_cache(&cache, "/repo/a");
    assert_eq!(info.skill_commands, vec!["skill-a".to_string()]);
}

/// Regression test for the bug where the compose-view menu returned
/// `cache.values().next()` — i.e., an arbitrary other repo's skills —
/// when the requested repo had no cache entry. Skills from a non-selected
/// repo must NEVER surface.
#[test]
fn lookup_returns_empty_for_unknown_repo_never_falls_back_to_other_repos() {
    let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
    let info = lookup_repo_commands_in_cache(&cache, "/repo/never-cached");
    assert!(
        info.skill_commands.is_empty(),
        "must not leak other repos' skills"
    );
    assert!(info.builtin_commands.is_empty());
}

#[test]
fn lookup_returns_empty_for_empty_cache() {
    let cache: HashMap<String, CcCommandsInfo> = HashMap::new();
    let info = lookup_repo_commands_in_cache(&cache, "/any/path");
    assert!(info.skill_commands.is_empty());
    assert!(info.builtin_commands.is_empty());
}

/// Cancellation produces `Ok(_)` from the run loop, so the predicate must
/// gate on the marker (single source of truth that `/harden` finished)
/// regardless of how the session ended.
#[test]
fn hardening_succeeded_requires_marker_and_no_cancel() {
    assert!(hardening_succeeded(false, true));
    assert!(!hardening_succeeded(true, true));
    assert!(!hardening_succeeded(false, false));
    assert!(!hardening_succeeded(true, false));
}

/// Race regression: when another device clicks Apply while CC is running
/// `/harden`, the change row flips to `"applied"` and the marker file is
/// consumed. `spawn_hardening_session`'s post-CC `branch_is_hardened` check
/// then returns false and would emit a spurious `ChangeApplyFailed` ~15s
/// after the user already saw `ChangeApplied` — surfacing as a "hindsight
/// failure" in the UI. The predicate gates the bail-out: only `"applied"`
/// triggers the skip; missing rows and any other status preserve the
/// existing failure path.
#[test]
fn change_applied_concurrently_only_true_for_applied_status() {
    assert!(change_applied_concurrently(Some("applied")));
    assert!(!change_applied_concurrently(Some("pending")));
    assert!(!change_applied_concurrently(Some("discarded")));
    assert!(!change_applied_concurrently(None));
}

