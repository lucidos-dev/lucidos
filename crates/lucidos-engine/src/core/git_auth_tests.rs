//! Tests for the shared git credential helper.
//!
//! Nothing here reaches a network. The store-backed tests use the disposable
//! database `setup_test_db` provisions; everything else is pure.

use super::*;
use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};

const HTTPS: CredentialType = CredentialType::USER_PASS_PLAINTEXT;

/// A store row, built by hand so a test can name one field at a time.
fn credential(auth_type: AuthType, auth_value: &str) -> Credential {
    let now = chrono::Utc::now();
    Credential {
        id: uuid::Uuid::new_v4(),
        service_name: "example-git".to_string(),
        base_url: "https://github.com".to_string(),
        auth_type,
        auth_value: auth_value.to_string(),
        auth_header: "Authorization".to_string(),
        env_var_name: None,
        created_at: now,
        updated_at: now,
    }
}

fn stored(base_url: &str, username: Option<&str>, secret: &str) -> StoredGitCredential {
    StoredGitCredential {
        service_name: "example-git".to_string(),
        base_url: base_url.to_string(),
        username: username.map(str::to_string),
        secret: secret.to_string(),
    }
}

/// Entries in the order [`GitCredentials::resolve_many`] leaves them: most
/// specific `base_url` first.
fn resolved(entries: Vec<StoredGitCredential>) -> GitCredentials {
    let mut entries = entries;
    entries.sort_by_key(|e| std::cmp::Reverse(e.base_url.len()));
    GitCredentials { entries }
}

#[test]
fn ssh_round_offers_only_the_agent() {
    let plan = credential_plan(CredentialType::SSH_KEY, StoredKind::Token);
    assert_eq!(plan, vec![CredentialSource::SshAgent]);
}

#[test]
fn username_round_is_answered_before_the_key_round() {
    let plan = credential_plan(CredentialType::USERNAME, StoredKind::None);
    assert_eq!(plan, vec![CredentialSource::Username]);
}

/// A bare token is offered in both forms, password first. GitHub reads it from
/// the password field. A host that refuses one form usually accepts the other,
/// so offering one alone can lose a working credential.
#[test]
fn a_stored_token_is_offered_in_both_forms_then_the_helper() {
    let plan = credential_plan(HTTPS, StoredKind::Token);
    assert_eq!(
        plan,
        vec![
            CredentialSource::StoredAsPassword,
            CredentialSource::StoredAsUsername,
            CredentialSource::Helper,
        ]
    );
}

/// A username and password pair only fits the password form. Sending the
/// password as the username would drop the username the user stored.
#[test]
fn a_stored_pair_is_offered_only_as_a_password() {
    let plan = credential_plan(HTTPS, StoredKind::UserPass);
    assert_eq!(
        plan,
        vec![CredentialSource::StoredAsPassword, CredentialSource::Helper]
    );
}

#[test]
fn https_round_with_nothing_stored_goes_straight_to_the_helper() {
    let plan = credential_plan(HTTPS, StoredKind::None);
    assert_eq!(plan, vec![CredentialSource::Helper]);
}

#[test]
fn default_round_offers_anonymous_last() {
    let plan = credential_plan(HTTPS | CredentialType::DEFAULT, StoredKind::Token);
    assert_eq!(
        plan,
        vec![
            CredentialSource::StoredAsPassword,
            CredentialSource::StoredAsUsername,
            CredentialSource::Helper,
            CredentialSource::Anonymous,
        ]
    );
}

#[test]
fn a_source_the_remote_did_not_ask_for_is_never_offered() {
    // An SSH-only round must not offer a stored secret, whatever is stored.
    let plan = credential_plan(CredentialType::SSH_KEY, StoredKind::Token);
    assert!(!plan.contains(&CredentialSource::StoredAsPassword));
    assert!(!plan.contains(&CredentialSource::StoredAsUsername));
    assert!(!plan.contains(&CredentialSource::Helper));
}

#[test]
fn every_source_is_offered_once_and_then_the_plan_is_spent() {
    let plan = credential_plan(HTTPS | CredentialType::DEFAULT, StoredKind::Token);
    let mut tried = 0u8;
    let offered: Vec<_> = std::iter::from_fn(|| next_untried(&plan, &mut tried)).collect();
    assert_eq!(offered, plan);
    assert_eq!(next_untried(&plan, &mut tried), None);
}

/// The retry guard proper: libgit2 re-invokes the callback on every rejection,
/// so a consumer that refuses everything must still terminate.
#[test]
fn a_consumer_that_rejects_everything_terminates() {
    let plan = credential_plan(
        HTTPS | CredentialType::SSH_KEY | CredentialType::USERNAME | CredentialType::DEFAULT,
        StoredKind::Token,
    );
    let mut tried = 0u8;
    let mut rounds = 0;
    // Bounded so the test fails rather than hangs if the guard ever regresses.
    while rounds < 100 {
        match next_untried(&plan, &mut tried) {
            Some(_) => rounds += 1,
            None => break,
        }
    }
    assert_eq!(rounds, plan.len(), "each source is offered exactly once");
    assert_eq!(next_untried(&plan, &mut tried), None, "and never again");
}

/// A source that cannot build a credential locally is skipped, not fatal.
/// Without this, one missing ssh-agent or credential helper aborts a clone the
/// next source could have finished.
#[test]
fn a_source_that_fails_locally_falls_through_to_the_next() {
    let plan = credential_plan(HTTPS, StoredKind::Token);
    let mut tried = 0u8;
    let mut offered = Vec::new();
    let picked = first_working(&plan, &mut tried, |source| {
        offered.push(source);
        match source {
            CredentialSource::Helper => Ok("helper"),
            _ => Err(()),
        }
    })
    .expect("the helper answers");
    assert_eq!(picked, "helper");
    assert_eq!(offered, plan, "each earlier source was tried first");
}

#[test]
fn every_source_failing_locally_ends_in_one_auth_error() {
    let plan = credential_plan(HTTPS | CredentialType::DEFAULT, StoredKind::Token);
    let mut tried = 0u8;
    let err =
        first_working(&plan, &mut tried, |_| Err::<(), ()>(())).expect_err("nothing can answer");
    assert_eq!(err.code(), git2::ErrorCode::Auth);
    assert!(is_auth_failure(&err), "so the friendly mapping applies");
    assert_eq!(
        next_untried(&plan, &mut tried),
        None,
        "and the plan is spent, so libgit2 cannot loop"
    );
}

#[test]
fn an_empty_plan_is_spent_immediately() {
    let mut tried = 0u8;
    assert_eq!(next_untried(&[], &mut tried), None);
}

#[test]
fn a_bearer_or_api_key_credential_is_a_bare_token() {
    for auth_type in [AuthType::Bearer, AuthType::ApiKey] {
        let entry = StoredGitCredential::from_credential(credential(auth_type, "ghp-secret"))
            .expect("a usable credential");
        assert_eq!(entry.username, None, "{auth_type}");
        assert_eq!(entry.secret, "ghp-secret");
        assert_eq!(entry.kind(), StoredKind::Token);
    }
}

/// A remote compares the secret byte for byte, so trimming one that really
/// carries a boundary space turns a valid credential into a failed clone.
#[test]
fn a_stored_secret_keeps_the_whitespace_the_user_stored() {
    let entry =
        StoredGitCredential::from_credential(credential(AuthType::Bearer, " pw with ends "))
            .expect("a usable credential");
    assert_eq!(entry.secret, " pw with ends ");

    let entry =
        StoredGitCredential::from_credential(credential(AuthType::Basic, "alice: pw with ends "))
            .expect("a usable credential");
    assert_eq!(entry.username.as_deref(), Some("alice"));
    assert_eq!(entry.secret, " pw with ends ");
}

#[test]
fn a_basic_credential_splits_on_the_first_colon() {
    let entry = StoredGitCredential::from_credential(credential(AuthType::Basic, "alice:pw:with"))
        .expect("a usable credential");
    assert_eq!(entry.username.as_deref(), Some("alice"));
    assert_eq!(entry.secret, "pw:with", "a colon in the password survives");
    assert_eq!(entry.kind(), StoredKind::UserPass);
}

/// A `basic` value with no colon is a bare token someone picked the wrong type
/// for. Reading it as a token still clones, where refusing it would not.
#[test]
fn a_basic_credential_without_a_colon_reads_as_a_token() {
    let entry = StoredGitCredential::from_credential(credential(AuthType::Basic, "ghp-secret"))
        .expect("a usable credential");
    assert_eq!(entry.username, None);
    assert_eq!(entry.kind(), StoredKind::Token);
}

#[test]
fn a_password_credential_reads_its_json_pair() {
    let entry = StoredGitCredential::from_credential(credential(
        AuthType::Password,
        r#"{"username":"alice","password":"pw"}"#,
    ))
    .expect("a usable credential");
    assert_eq!(entry.username.as_deref(), Some("alice"));
    assert_eq!(entry.secret, "pw");
    assert_eq!(entry.kind(), StoredKind::UserPass);
}

#[test]
fn a_credential_carrying_nothing_a_clone_can_present_is_skipped() {
    for (auth_type, auth_value) in [
        (AuthType::OauthClient, r#"{"client_id":"x"}"#),
        (AuthType::EmailPassword, "alice@example.com:pw"),
        (AuthType::Unknown, "whatever"),
        // Well-typed but empty, so there is no secret to send.
        (AuthType::Bearer, "   "),
        (AuthType::Basic, "alice:"),
        (AuthType::Password, "not json"),
        (AuthType::Password, r#"{"username":"alice"}"#),
    ] {
        assert_eq!(
            StoredGitCredential::from_credential(credential(auth_type, auth_value)),
            None,
            "{auth_type} / {auth_value}"
        );
    }
}

#[test]
fn the_narrowest_scope_wins_when_two_credentials_match() {
    let credentials = resolved(vec![
        stored("https://github.com", None, "host-wide"),
        stored("https://github.com/example-org", None, "org-only"),
    ]);
    assert_eq!(
        credentials
            .for_url("https://github.com/example-org/example-repo.git")
            .map(|c| c.secret.as_str()),
        Some("org-only")
    );
    assert_eq!(
        credentials
            .for_url("https://github.com/other-org/example-repo.git")
            .map(|c| c.secret.as_str()),
        Some("host-wide")
    );
}

/// libgit2 passes the URL it is authenticating against, which a redirect can
/// move to another host mid-clone. Re-matching per invocation is what stops the
/// secret following it there.
#[test]
fn a_redirect_to_another_host_is_offered_no_stored_credential() {
    let credentials = resolved(vec![stored("https://github.com", None, "ghp-secret")]);

    let (plan, entry) = plan_for(
        "https://github.com/example-org/example-repo.git",
        HTTPS,
        &credentials,
    );
    assert_eq!(entry.map(|e| e.secret.as_str()), Some("ghp-secret"));
    assert!(plan.contains(&CredentialSource::StoredAsPassword));

    let (plan, entry) = plan_for(
        "https://redirect-target.example/example-org/example-repo.git",
        HTTPS,
        &credentials,
    );
    assert_eq!(entry, None, "the redirected host was never scoped");
    assert_eq!(plan, vec![CredentialSource::Helper]);
}

/// A stored secret must not reach a server the user never scoped it to. A
/// hostile URL that challenges for basic auth would otherwise collect it, and
/// `git_clone` takes its URL from the model.
#[test]
fn a_stored_credential_is_never_offered_to_another_host() {
    let credentials = resolved(vec![stored("https://github.com", None, "ghp-secret")]);
    for url in [
        "https://gitlab.com/example-org/example-repo.git",
        "https://bitbucket.org/example-org/example-repo.git",
        // A hostname merely mentioning github is a different host.
        "https://example-github-mirror.example.com/example-org/example-repo.git",
        "https://notgithub.com/example-org/example-repo.git",
        // Same host, wrong scheme: a plaintext hop must not carry it.
        "http://github.com/example-org/example-repo.git",
        // An SSH clone presents a key, never a stored secret.
        "ssh://git@github.com/example-org/example-repo.git",
        "git@github.com:example-org/example-repo.git",
    ] {
        assert_eq!(credentials.for_url(url), None, "{url}");
    }
}

#[test]
fn no_credential_at_all_matches_nothing() {
    assert_eq!(
        GitCredentials::none().for_url("https://github.com/example-org/example-repo.git"),
        None
    );
}

#[test]
fn the_base_url_hint_keeps_the_host_and_drops_the_rest() {
    for (url, expected) in [
        (
            "https://github.com/example-org/example-repo.git",
            Some("https://github.com"),
        ),
        (
            "https://GitHub.Example.IO:8443/example-org/repo.git",
            Some("https://github.example.io:8443"),
        ),
        (
            // An inline credential names no scope, so it is dropped too.
            "https://x-access-token:secret-value@github.com/example-org/repo.git",
            Some("https://github.com"),
        ),
        ("git@github.com:example-org/example-repo.git", None),
    ] {
        assert_eq!(credential_base_url_hint(url).as_deref(), expected, "{url}");
    }
}

fn auth_error() -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Auth,
        git2::ErrorClass::Http,
        "remote authentication required but no callback set",
    )
}

#[test]
fn an_auth_failure_names_the_url_and_the_fix() {
    let url = "https://github.com/example-org/example-repo.git";
    let message = describe_clone_failure(url, &auth_error(), None);
    assert!(message.contains(url), "{message}");
    assert!(message.contains("private or internal"), "{message}");
    assert!(message.contains("Settings, Credentials"), "{message}");
    assert!(
        message.contains("Base URL https://github.com"),
        "the exact scope to register: {message}"
    );
    assert!(message.contains("ssh-agent"), "{message}");
    assert!(
        !message.contains("class=Http"),
        "the raw libgit2 text must not survive: {message}"
    );
}

#[test]
fn a_rejected_credential_says_which_one_was_presented() {
    let message = describe_clone_failure(
        "https://github.com/example-org/example-repo.git",
        &auth_error(),
        Some("example-git"),
    );
    assert!(message.contains("example-git"), "{message}");
    assert!(message.contains("rejected"), "{message}");
}

/// An SSH clone presents a key. Advice about a stored token would be a dead
/// end, so the remedy follows the transport.
#[test]
fn an_ssh_failure_recommends_ssh_agent_and_no_credential() {
    for url in [
        "git@github.com:example-org/example-repo.git",
        "ssh://git@github.example.io/example-org/example-repo.git",
    ] {
        let message = describe_clone_failure(url, &auth_error(), None);
        assert!(message.contains("ssh-agent"), "{message}");
        assert!(!message.contains("Settings"), "{message}");
    }
}

/// A URL may carry its own credential, and the failure message is shown in the
/// Plugins panel and returned to the model. Neither may repeat the secret.
#[test]
fn an_inline_credential_is_stripped_from_the_failure() {
    let message = describe_clone_failure(
        "https://x-access-token:secret-value@github.com/example-org/example-repo.git",
        &auth_error(),
        None,
    );
    assert!(!message.contains("secret-value"), "{message}");
    assert!(
        message.contains("github.com/example-org/example-repo.git"),
        "the user still has to recognise which repo failed: {message}"
    );
}

#[test]
fn a_url_carrying_no_secret_is_left_alone() {
    for url in [
        "https://github.com/example-org/example-repo.git",
        // An scp-style username is an SSH account, not a credential.
        "git@github.com:example-org/example-repo.git",
        "ssh://git@github.com/example-org/example-repo.git",
        "file:///tmp/example-repo",
    ] {
        assert_eq!(redacted(url), url);
    }
}

/// A stored secret reaches libgit2 and nothing else. This drives the real
/// failure path rather than the mapper alone, so it catches a leak the
/// signature does not rule out.
#[test]
fn a_clone_failure_never_echoes_a_stored_secret() {
    let scratch = tempfile::TempDir::new().unwrap();
    let url = format!("file://{}", scratch.path().join("no-such-repo").display());
    let credentials = resolved(vec![stored("file://", None, "secret-value")]);
    // `Repository` has no `Debug`, so unwrap the error by hand.
    let Err(message) = shallow_clone(&url, None, &scratch.path().join("clone"), &credentials)
    else {
        panic!("cloning a path that does not exist must fail");
    };
    assert!(!message.contains("secret-value"), "{message}");
}

#[test]
fn a_non_auth_failure_keeps_its_own_text() {
    let err = git2::Error::new(
        git2::ErrorCode::NotFound,
        git2::ErrorClass::Repository,
        "repository 'https://github.com/example-org/nope.git' not found",
    );
    let message = describe_clone_failure(
        "https://github.com/example-org/nope.git",
        &err,
        Some("example-git"),
    );
    assert!(message.contains("not found"), "{message}");
    assert!(
        !message.contains("Authentication required"),
        "a missing repo must not be reported as an auth problem: {message}"
    );
}

#[test]
fn an_ssh_auth_failure_is_recognised_too() {
    let err = git2::Error::new(
        git2::ErrorCode::Auth,
        git2::ErrorClass::Ssh,
        "authentication required",
    );
    let message = describe_clone_failure("git@github.com:example-org/example-repo.git", &err, None);
    assert!(message.contains("Authentication required"), "{message}");
}

/// A local clone must stay deep. libgit2's local transport rejects a shallow
/// fetch, so a `depth(1)` leaking onto this path fails the clone outright.
#[test]
fn a_local_clone_stays_deep_and_still_succeeds() {
    let scratch = tempfile::TempDir::new().unwrap();
    let origin = scratch.path().join("origin");
    let repo = git2::Repository::init(&origin).unwrap();
    std::fs::write(origin.join("README.md"), "hello").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Lucidos Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
        .unwrap();
    drop(tree);
    drop(index);
    drop(repo);

    let url = format!("file://{}", origin.display());
    let target = scratch.path().join("clone");
    let cloned =
        shallow_clone(&url, None, &target, &GitCredentials::none()).expect("local clone succeeds");
    drop(cloned);
    assert!(target.join("README.md").is_file());
}

/// The credential store is the only home for a git secret. An env read here
/// would reintroduce the path this module deliberately dropped: the
/// environment-variable store broadcasts its values to every device.
#[test]
fn no_secret_is_read_from_the_process_environment() {
    let source = include_str!("git_auth.rs");
    assert!(
        !source.contains("std::env::var") && !source.contains("env::var("),
        "core/git_auth.rs must resolve secrets through the credential store alone"
    );
}

#[tokio::test]
async fn a_stored_credential_is_resolved_for_the_url_it_scopes() {
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "example-git",
        "https://github.com",
        AuthType::Bearer,
        "ghp-secret",
    )
    .await;

    let credentials =
        GitCredentials::resolve_one(&pool, "https://github.com/example-org/example-repo.git").await;
    let entry = credentials
        .for_url("https://github.com/example-org/example-repo.git")
        .expect("the seeded credential");
    assert_eq!(entry.service_name, "example-git");
    assert_eq!(entry.secret, "ghp-secret");
    assert_eq!(entry.kind(), StoredKind::Token);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_url_no_credential_scopes_resolves_to_nothing() {
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "example-git",
        "https://github.com",
        AuthType::Bearer,
        "ghp-secret",
    )
    .await;

    let credentials =
        GitCredentials::resolve_one(&pool, "https://gitlab.com/example-org/example-repo.git").await;
    assert_eq!(credentials, GitCredentials::none());

    teardown_test_db(&db).await;
}

/// The marketplace scan resolves every registered clone URL in one pass, so two
/// marketplaces on one host must not stack the same credential twice.
#[tokio::test]
async fn many_urls_resolve_once_per_credential() {
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "example-git",
        "https://github.com",
        AuthType::Bearer,
        "ghp-secret",
    )
    .await;
    seed_credential(
        &pool,
        "example-internal",
        "https://github.example.io",
        AuthType::Bearer,
        "internal-secret",
    )
    .await;

    let credentials = GitCredentials::resolve_many(
        &pool,
        &[
            "https://github.com/example-org/one.git",
            "https://github.com/example-org/two.git",
            "https://github.example.io/example-org/three.git",
            "https://gitlab.com/example-org/four.git",
        ],
    )
    .await;

    assert_eq!(credentials.entries.len(), 2, "one per matching credential");
    assert_eq!(
        credentials
            .for_url("https://github.example.io/example-org/three.git")
            .map(|e| e.service_name.as_str()),
        Some("example-internal")
    );
    assert_eq!(
        credentials.for_url("https://gitlab.com/example-org/four.git"),
        None
    );

    teardown_test_db(&db).await;
}

/// An `oauth_client` row's `base_url` is the provider's API host, and its value
/// is a client registration rather than a usable secret. The store excludes it
/// from URL matching, so a clone never sees it.
#[tokio::test]
async fn an_oauth_client_credential_is_never_resolved_for_a_clone() {
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "example-oauth",
        "https://github.com",
        AuthType::OauthClient,
        r#"{"client_id":"abc","client_secret":"shh"}"#,
    )
    .await;

    let credentials =
        GitCredentials::resolve_one(&pool, "https://github.com/example-org/example-repo.git").await;
    assert_eq!(credentials, GitCredentials::none());

    teardown_test_db(&db).await;
}
