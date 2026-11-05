use super::*;

fn key(byte: u8) -> SecretKey {
    SecretKey::from_bytes(&[byte; 32]).unwrap()
}

#[test]
fn a_sealed_value_opens_to_what_was_sealed_and_names_its_key() {
    let k = key(1);
    let sealed = k.seal(&CredentialAuthValue::SPEC, "sk-the-key").unwrap();
    assert!(sealed.starts_with(SEALED_PREFIX));
    assert!(!sealed.contains("sk-the-key"));
    assert_eq!(sealed_key_id(&sealed), Some(k.id().as_str()));
    assert_eq!(
        k.open(&CredentialAuthValue::SPEC, &sealed).unwrap(),
        "sk-the-key"
    );
}

#[test]
fn sealing_twice_gives_two_ciphertexts() {
    let k = key(1);
    let a = k.seal(&CredentialAuthValue::SPEC, "same").unwrap();
    let b = k.seal(&CredentialAuthValue::SPEC, "same").unwrap();
    assert_ne!(a, b, "a fresh nonce per value");
}

#[test]
fn a_value_moved_to_another_column_does_not_open() {
    let k = key(1);
    let sealed = k.seal(&OAuthAccessToken::SPEC, "token").unwrap();
    let err = k.open(&OAuthRefreshToken::SPEC, &sealed).unwrap_err();
    assert!(err.to_string().contains("altered or moved"), "{err}");
}

#[test]
fn another_key_is_named_rather_than_tried() {
    let sealed = key(1).seal(&CredentialAuthValue::SPEC, "x").unwrap();
    let err = key(2)
        .open(&CredentialAuthValue::SPEC, &sealed)
        .unwrap_err();
    assert!(err.to_string().contains(&key(1).id()), "{err}");
}

#[test]
fn a_plain_value_is_refused_never_passed_through() {
    let err = key(1)
        .open(&CredentialAuthValue::SPEC, "sk-plain")
        .unwrap_err();
    assert!(err.to_string().contains("not sealed"), "{err}");
}

#[test]
fn empty_means_none_only_where_the_column_says_so() {
    let k = key(1);
    assert_eq!(k.seal(&EmailPassword::SPEC, "").unwrap(), "");
    assert_eq!(k.open(&EmailPassword::SPEC, "").unwrap(), "");
    let sealed = k.seal(&CredentialAuthValue::SPEC, "").unwrap();
    assert!(sealed.starts_with(SEALED_PREFIX));
    assert_eq!(k.open(&CredentialAuthValue::SPEC, &sealed).unwrap(), "");
}

#[test]
fn the_key_id_is_short_hex_and_debug_never_prints_the_key() {
    let k = key(3);
    assert_eq!(k.id().len(), 16);
    assert!(k.id().chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(format!("{k:?}"), format!("SecretKey({})", k.id()));
    assert_eq!(SecretKey::from_base64(&k.to_base64()).unwrap(), k);
}

#[test]
fn a_key_is_exactly_32_bytes() {
    assert!(SecretKey::from_bytes(&[0; 31]).is_err());
    assert!(SecretKey::from_bytes(&[0; 33]).is_err());
}

#[test]
fn every_secret_column_is_listed_once() {
    let mut names: Vec<String> = SECRET_COLUMNS.iter().map(SecretColumnSpec::name).collect();
    let count = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), count);
}

#[test]
fn json_values_round_trip_and_keep_their_names_readable() {
    let plain = std::collections::HashMap::from([("API_KEY".to_string(), "v".to_string())]);
    let sealed = seal_json_values(&McpServerEnv::SPEC, &plain).unwrap();
    assert!(sealed["API_KEY"]
        .as_str()
        .unwrap()
        .starts_with(SEALED_PREFIX));
    assert_eq!(
        open_json_values(&McpServerEnv::SPEC, sealed).unwrap(),
        plain
    );
}

#[test]
fn a_secret_redacts_itself() {
    let secret = Secret::<CredentialAuthValue>::new("sk-the-key");
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
}

#[tokio::test]
async fn a_secret_seals_as_it_binds_and_opens_as_it_decodes() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let stored: String = sqlx::query_scalar("SELECT $1::text")
        .bind(Secret::<CredentialAuthValue>::new("sk-the-key"))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(stored.starts_with(SEALED_PREFIX), "{stored}");
    let opened: Secret<CredentialAuthValue> = sqlx::query_scalar("SELECT $1::text")
        .bind(&stored)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(opened.as_str(), "sk-the-key");
    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// Columns whose name looks like a secret's and whose value is not one.
const NOT_SECRETS: &[(&str, &str)] = &[
    // A header name such as `Authorization`, never its value.
    ("credentials", "auth_header"),
    ("credentials", "auth_type"),
    ("credentials", "env_var_name"),
    ("claude_gateway_accounts", "token_expiry"),
    ("email_accounts", "oauth_account_id"),
    // Variable name to credential name: a pointer, never the value.
    ("mcp_servers", "env_credentials"),
    ("oauth_accounts", "token_expiry"),
    // A SHA-256 digest: the token is shown once and never stored.
    ("webhooks", "token_hash"),
];

/// A new secret column must join [`SECRET_COLUMNS`], or be named here as not
/// one. The name heuristic is what catches a table added on another branch.
#[tokio::test]
async fn every_column_named_like_a_secret_is_sealed_or_declared_not_one() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text FROM information_schema.columns \
         WHERE table_schema = 'public' \
           AND column_name ~ '(token|secret|password|passwd|auth|private|credential|api_key|env)'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let unaccounted: Vec<String> = columns
        .iter()
        .filter(|(table, column)| {
            !SECRET_COLUMNS
                .iter()
                .chain(MOVED_SECRET_COLUMNS)
                .any(|spec| spec.table == table && spec.column == column)
                && !NOT_SECRETS.contains(&(table.as_str(), column.as_str()))
        })
        .map(|(table, column)| format!("{table}.{column}"))
        .collect();
    assert!(unaccounted.is_empty(), "unaccounted: {unaccounted:#?}");
    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}
