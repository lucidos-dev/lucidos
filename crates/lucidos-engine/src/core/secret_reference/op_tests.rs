//! `op` resolver tests. A fake `op` script stands in for the real tool, so no
//! test needs a 1Password account.

use super::*;
use crate::core::AuthType;
use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};

/// An obviously fake token, shaped like a real one so the masking applies.
const TOKEN: &str = "ops_FAKETOKENFORTESTSONLY0123456789";

/// A fake `op` in its own temporary folder running `body` as `/bin/sh`.
pub(crate) fn fake_op(body: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("op");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

fn account(cli: &Path) -> ServiceAccount {
    ServiceAccount {
        token: TOKEN.to_string(),
        cli_path: Some(cli.display().to_string()),
    }
}

fn reference() -> SecretReference {
    SecretReference::parse("op://Engineering/Stripe/credential").unwrap()
}

#[tokio::test]
async fn op_reads_the_reference_and_its_child_holds_only_the_token_path_and_home() {
    let env_out = tempfile::NamedTempFile::new().unwrap();
    let (_dir, cli) = fake_op(&format!(
        "env > '{}'\nprintf 'resolved-value'",
        env_out.path().display()
    ));

    let value = run_op_read(&account(&cli), &reference(), OP_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(value, "resolved-value");

    let env = std::fs::read_to_string(env_out.path()).unwrap();
    let names: Vec<&str> = env
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        // The shell adds these itself; none came from the engine.
        .filter(|name| !matches!(*name, "PWD" | "SHLVL" | "_" | "OLDPWD"))
        .collect();
    assert!(
        env.contains(&format!("OP_SERVICE_ACCOUNT_TOKEN={TOKEN}")),
        "op gets the token"
    );
    for name in &names {
        assert!(
            matches!(*name, "OP_SERVICE_ACCOUNT_TOKEN" | "PATH" | "HOME"),
            "op inherited {name}"
        );
    }
}

#[tokio::test]
async fn a_missing_item_is_definitive_and_its_reason_never_holds_the_token() {
    let (_dir, cli) = fake_op(&format!(
        "echo '[ERROR] \"Stripe\" isn'\"'\"'t an item in the \"Engineering\" vault ({TOKEN})' >&2\nexit 1"
    ));
    let failure = run_op_read(&account(&cli), &reference(), OP_TIMEOUT)
        .await
        .unwrap_err();
    let ResolveFailure::Definitive(reason) = failure else {
        panic!("a missing item must be definitive: {failure:?}");
    };
    assert!(reason.contains("isn't an item"), "{reason}");
    assert!(!reason.contains(TOKEN), "the token leaked into {reason}");
}

#[tokio::test]
async fn a_network_error_is_a_transport_failure() {
    let (_dir, cli) = fake_op("echo 'dial tcp: connection refused' >&2\nexit 1");
    let failure = run_op_read(&account(&cli), &reference(), OP_TIMEOUT)
        .await
        .unwrap_err();
    assert!(
        matches!(failure, ResolveFailure::Transport(_)),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_hanging_op_times_out_as_a_transport_failure() {
    let (_dir, cli) = fake_op("sleep 30");
    let started = std::time::Instant::now();
    let failure = run_op_read(&account(&cli), &reference(), Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(
        matches!(failure, ResolveFailure::Transport(_)),
        "{failure:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the timeout did not fire"
    );
}

#[tokio::test]
async fn an_op_that_is_not_there_is_a_transport_failure() {
    let missing = ServiceAccount {
        token: TOKEN.to_string(),
        cli_path: Some("/nonexistent/op".to_string()),
    };
    let failure = run_op_read(&missing, &reference(), OP_TIMEOUT)
        .await
        .unwrap_err();
    assert!(
        matches!(failure, ResolveFailure::Transport(_)),
        "{failure:?}"
    );
}

#[test]
fn a_service_account_is_a_json_object_or_the_bare_token() {
    let json = ServiceAccount::parse(r#"{"token":"ops_abc","cli_path":"/usr/local/bin/op"}"#);
    assert_eq!(json.token, "ops_abc");
    assert_eq!(json.cli_path.as_deref(), Some("/usr/local/bin/op"));

    let bare = ServiceAccount::parse(" ops_abc ");
    assert_eq!(bare.token, "ops_abc");
    assert_eq!(bare.cli_path, None);
}

#[test]
fn the_token_shape_is_masked_in_any_output() {
    let masked =
        crate::core::subprocess_secrets::redact_known_token_shapes(&format!("token is {TOKEN}"));
    assert!(!masked.contains(TOKEN), "{masked}");
}

#[tokio::test]
async fn with_no_service_account_a_reference_fails_definitively() {
    let (pool, db) = setup_test_db().await;
    let failure = OpResolver::new(pool.clone())
        .resolve(&reference())
        .await
        .unwrap_err();
    assert!(
        matches!(failure, ResolveFailure::Definitive(_)),
        "{failure:?}"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_stored_service_account_names_the_token_and_the_op_to_run() {
    let (pool, db) = setup_test_db().await;
    let (_dir, cli) = fake_op(
        "[ \"$OP_SERVICE_ACCOUNT_TOKEN\" = 'ops_FAKETOKENFORTESTSONLY0123456789' ] || exit 3\n\
         printf 'from-the-vault'",
    );
    let payload = serde_json::json!({ "token": TOKEN, "cli_path": cli.display().to_string() });
    seed_credential(
        &pool,
        "1password",
        "",
        AuthType::OnepasswordServiceAccount,
        &payload.to_string(),
    )
    .await;

    let value = OpResolver::with_timeout(pool.clone(), OP_TIMEOUT)
        .resolve(&reference())
        .await
        .unwrap();
    assert_eq!(value, "from-the-vault");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_vault_check_names_the_op_that_ran_and_the_vaults_it_reaches() {
    let (_dir, cli) = fake_op(
        r#"[ "$1 $2 $3 $4" = "vault list --format json" ] || exit 9
printf '[{"id":"a1","name":"Engineering"},{"id":"b2","name":"Shared"}]'"#,
    );

    let check = list_vaults(&account(&cli), OP_TIMEOUT).await.unwrap();

    assert_eq!(check.cli, cli.display().to_string());
    assert_eq!(check.vaults, ["Engineering", "Shared"]);
}

#[tokio::test]
async fn a_refused_vault_check_is_definitive_and_never_holds_the_token() {
    let (_dir, cli) = fake_op(&format!(
        "echo '[ERROR] authentication failed for {TOKEN}' >&2\nexit 1"
    ));

    let Err(ResolveFailure::Definitive(reason)) = list_vaults(&account(&cli), OP_TIMEOUT).await
    else {
        panic!("a refused token is a definitive failure");
    };
    assert!(!reason.contains(TOKEN), "{reason}");
}

/// Every way `op` words a missing item, vault or field drops the cached value,
/// and a network failure does not.
#[test]
fn a_missing_field_in_any_wording_is_definitive() {
    for said in [
        "the specified field cannot be found within the item",
        "\"op://v/i/f\" isn't a field in the \"i\" item",
        "no item found",
        "vault does not exist",
    ] {
        assert!(
            matches!(classify_op_failure(said), ResolveFailure::Definitive(_)),
            "{said}"
        );
    }
    assert!(matches!(
        classify_op_failure("dial tcp: lookup my.1password.com: no such host"),
        ResolveFailure::Transport(_)
    ));
}
