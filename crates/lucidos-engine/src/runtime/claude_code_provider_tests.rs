use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;

fn inputs<'a>(
    binary: &'a str,
    env: &'a [(String, String)],
    pin: Option<&'a AccountPin>,
) -> ProviderInputs<'a> {
    ProviderInputs {
        binary_override: Some(binary),
        user_env_vars: env,
        account_pin: pin,
        worktree_path: std::path::Path::new("/tmp"),
        thread_id: Uuid::new_v4(),
    }
}

/// A `claude` stand-in that runs `body` as a shell script.
fn fake_claude(dir: &std::path::Path, body: &str) -> String {
    let path = dir.join("claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

fn answers(provider: &str) -> String {
    format!("echo '{{\"loggedIn\":true,\"apiProvider\":\"{provider}\",\"email\":\"user@example.com\"}}'")
}

fn key(binary: &str) -> CheckKey {
    CheckKey {
        binary: binary.into(),
        env_digest: digest_of_env(&[]),
        pin: None,
        worktree_path: "/tmp".into(),
    }
}

// ── Invariant 1: the allowlist ─────────────────────────────────────────────

#[test]
fn only_the_two_proven_routes_show_the_notes() {
    use NotesVisibility::{Hidden, Shown};
    let cases = [
        (Some("firstParty"), false, Shown),
        (Some("firstParty"), true, Shown),
        (Some("vertex"), true, Shown),
        (Some("vertex"), false, Hidden),
        (Some("gateway"), true, Hidden),
        (Some("bedrock"), true, Hidden),
        (Some("foundry"), true, Hidden),
        (Some("somethingNew"), true, Hidden),
        (Some(""), true, Hidden),
        (None, true, Hidden),
    ];
    for (provider, relay, expected) in cases {
        assert_eq!(
            notes_visibility_for(provider, relay),
            expected,
            "{provider:?} with relay {relay}"
        );
    }
}

#[test]
fn the_reply_is_read_for_its_provider_alone() {
    let full = br#"{"loggedIn":true,"apiProvider":"vertex","email":"user@example.com"}"#;
    assert_eq!(parse_api_provider(full).as_deref(), Some("vertex"));
    for bad in [
        &b"not json"[..],
        b"",
        b"{}",
        br#"{"apiProvider":7}"#,
        br#"["apiProvider"]"#,
    ] {
        assert_eq!(
            parse_api_provider(bad),
            None,
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

// ── Invariant 2: the session's own binary and environment ──────────────────

#[test]
fn the_check_runs_the_sessions_binary_under_its_provider_environment() {
    let env = vec![("CLAUDE_CODE_USE_VERTEX".to_string(), "1".to_string())];
    let pin = AccountPin::ExplicitConfigDir {
        dir: "/tmp/profile-a".to_string(),
    };
    let binary = OsString::from("/opt/example/claude");
    let cmd = provider_check_command(&inputs("/opt/example/claude", &env, Some(&pin)), &binary);
    let std = cmd.as_std();
    assert_eq!(std.get_program(), "/opt/example/claude");
    assert_eq!(std.get_args().collect::<Vec<_>>(), ["auth", "status"]);
    let envs: HashMap<_, _> = std.get_envs().collect();
    let var = |name: &str| envs.get(std::ffi::OsStr::new(name)).copied().flatten();
    assert_eq!(var("CLAUDE_CODE_USE_VERTEX"), Some("1".as_ref()));
    assert_eq!(var("CLAUDE_CONFIG_DIR"), Some("/tmp/profile-a".as_ref()));
}

#[test]
fn a_default_profile_pin_unsets_the_config_dir_for_the_check() {
    let pin = AccountPin::DefaultConfigDir {
        dir: "/tmp/default".to_string(),
    };
    let cmd = provider_check_command(&inputs("claude", &[], Some(&pin)), &"claude".into());
    let envs: HashMap<_, _> = cmd.as_std().get_envs().collect();
    assert_eq!(
        envs.get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR")),
        Some(&None),
        "removed, as a session's command removes it"
    );
}

#[test]
fn the_cache_key_names_the_binary_the_variables_and_the_pin() {
    let env_a = vec![
        ("A".to_string(), "1".to_string()),
        ("B".to_string(), "2".to_string()),
    ];
    let env_b = vec![
        ("B".to_string(), "2".to_string()),
        ("A".to_string(), "1".to_string()),
    ];
    let pin = AccountPin::ExplicitConfigDir {
        dir: "/x".to_string(),
    };
    let (_, key_a) = check_target(&inputs("/bin/claude", &env_a, None));
    let (_, key_b) = check_target(&inputs("/bin/claude", &env_b, None));
    assert_eq!(key_a, key_b, "the order of the variables is no difference");
    let elsewhere = ProviderInputs {
        worktree_path: std::path::Path::new("/tmp/other-worktree"),
        ..inputs("/bin/claude", &env_a, None)
    };
    for other in [
        check_target(&elsewhere).1,
        check_target(&inputs("/bin/other", &env_a, None)).1,
        check_target(&inputs("/bin/claude", &[], None)).1,
        check_target(&inputs("/bin/claude", &env_a, Some(&pin))).1,
    ] {
        assert_ne!(key_a, other);
    }
}

// ── Invariant 3: never blocks, never fails ─────────────────────────────────

#[tokio::test]
async fn a_check_reads_the_provider_the_binary_names() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_claude(dir.path(), &answers("gateway"));
    let cmd = provider_check_command(&inputs(&binary, &[], None), &binary.clone().into());
    assert_eq!(
        run_check(cmd, Duration::from_secs(5)).await.as_deref(),
        Some("gateway")
    );
}

#[tokio::test]
async fn a_hung_check_ends_at_the_limit_with_no_provider() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_claude(dir.path(), "sleep 30");
    let cmd = provider_check_command(&inputs(&binary, &[], None), &binary.clone().into());
    let started = Instant::now();
    assert_eq!(run_check(cmd, Duration::from_millis(200)).await, None);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_missing_binary_or_garbled_reply_gives_no_provider() {
    let missing = provider_check_command(
        &inputs("/nonexistent/claude", &[], None),
        &"/nonexistent/claude".into(),
    );
    assert_eq!(run_check(missing, Duration::from_secs(5)).await, None);

    let dir = tempfile::tempdir().unwrap();
    let binary = fake_claude(dir.path(), "echo 'Not logged in'; exit 1");
    let garbled = provider_check_command(&inputs(&binary, &[], None), &binary.clone().into());
    assert_eq!(run_check(garbled, Duration::from_secs(5)).await, None);
}

#[tokio::test]
async fn the_verdict_follows_the_binary_and_a_failed_check_is_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let first_party = fake_claude(dir.path(), &answers("firstParty"));
    assert_eq!(
        notes_visibility(&inputs(&first_party, &[], None)).await,
        NotesVisibility::Shown
    );

    let other = tempfile::tempdir().unwrap();
    let gateway = fake_claude(other.path(), &answers("gateway"));
    assert_eq!(
        notes_visibility(&inputs(&gateway, &[], None)).await,
        NotesVisibility::Hidden
    );

    assert_eq!(
        notes_visibility(&inputs("/nonexistent/claude-for-verdict", &[], None)).await,
        NotesVisibility::Hidden
    );
}

// ── Invariant 5: the cache ─────────────────────────────────────────────────

#[tokio::test]
async fn a_key_is_checked_once_within_the_window() {
    let cache = ProviderCache::default();
    let calls = Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        async { Some("firstParty".to_string()) }
    };
    let start = Instant::now();
    let first = cached_provider(&cache, key("a"), start, check).await;
    let again = cached_provider(
        &cache,
        key("a"),
        start + VERDICT_TTL - Duration::from_secs(1),
        check,
    )
    .await;
    assert_eq!(
        (first, again),
        (Some("firstParty".into()), Some("firstParty".into()))
    );
    assert_eq!(calls.get(), 1);
}

#[tokio::test]
async fn a_verdict_expires_so_a_re_login_is_picked_up() {
    let cache = ProviderCache::default();
    let start = Instant::now();
    let first = cached_provider(&cache, key("a"), start, || async {
        Some("gateway".to_string())
    })
    .await;
    let later = cached_provider(&cache, key("a"), start + VERDICT_TTL, || async {
        Some("firstParty".to_string())
    })
    .await;
    assert_eq!(first.as_deref(), Some("gateway"));
    assert_eq!(later.as_deref(), Some("firstParty"));
}

#[tokio::test]
async fn another_key_has_its_own_check_and_a_failure_is_kept_too() {
    let cache = ProviderCache::default();
    let calls = Cell::new(0);
    let failing = || {
        calls.set(calls.get() + 1);
        async { None }
    };
    let now = Instant::now();
    cached_provider(&cache, key("a"), now, failing).await;
    cached_provider(&cache, key("a"), now, failing).await;
    assert_eq!(
        calls.get(),
        1,
        "a failed check is not re-run for every spawn"
    );
    cached_provider(&cache, key("b"), now, failing).await;
    assert_eq!(calls.get(), 2);
}

#[tokio::test]
async fn an_expired_entry_leaves_the_cache_on_the_next_write() {
    let cache = ProviderCache::default();
    let start = Instant::now();
    cached_provider(&cache, key("old"), start, || async { None }).await;
    cached_provider(&cache, key("new"), start + VERDICT_TTL, || async { None }).await;
    assert_eq!(cache.lock().len(), 1);
}

/// A timeout says nothing about the provider, so it must not read as hidden for
/// ten minutes.
#[tokio::test]
async fn a_failed_check_is_retried_soon_while_a_verdict_is_not() {
    let cache = ProviderCache::default();
    let start = Instant::now();
    cached_provider(&cache, key("a"), start, || async { None }).await;
    let soon = start + FAILURE_TTL + Duration::from_secs(1);
    let retried = cached_provider(&cache, key("a"), soon, || async {
        Some("firstParty".to_string())
    })
    .await;
    assert_eq!(retried.as_deref(), Some("firstParty"));
    let later = soon + FAILURE_TTL + Duration::from_secs(1);
    let kept = cached_provider(&cache, key("a"), later, || async {
        panic!("a verdict outlives the failure window")
    })
    .await;
    assert_eq!(kept.as_deref(), Some("firstParty"));
}

#[test]
fn the_check_runs_in_the_sessions_directory() {
    let dir = std::path::Path::new("/tmp/session-worktree");
    let session = ProviderInputs {
        worktree_path: dir,
        ..inputs("claude", &[], None)
    };
    let cmd = provider_check_command(&session, &"claude".into());
    assert_eq!(cmd.as_std().get_current_dir(), Some(dir));
}

#[test]
fn the_cache_key_never_holds_a_variable_value() {
    let env = vec![("API_KEY".to_string(), "secret-value-123".to_string())];
    let (_, key) = check_target(&inputs("/bin/claude", &env, None));
    assert!(!format!("{key:?}").contains("secret-value-123"));
}
