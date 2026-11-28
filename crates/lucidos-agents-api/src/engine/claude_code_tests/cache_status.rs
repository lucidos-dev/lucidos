use super::*;
use crate::core::changes::ChangeStatus;

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

#[test]
fn hardening_succeeded_requires_marker_and_no_cancel() {
    assert!(hardening_succeeded(false, true));
    assert!(!hardening_succeeded(true, true));
    assert!(!hardening_succeeded(false, false));
    assert!(!hardening_succeeded(true, false));
}

#[test]
fn change_applied_concurrently_only_true_for_applied_status() {
    for status in ChangeStatus::ALL {
        assert_eq!(
            change_applied_concurrently(Some(status)),
            status == ChangeStatus::Applied,
            "{status}"
        );
    }
    assert!(!change_applied_concurrently(None));
}
