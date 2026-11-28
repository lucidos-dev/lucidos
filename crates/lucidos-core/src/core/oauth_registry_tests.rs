use super::*;

fn registry_dir(body: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join(REGISTRY_FILE),
        format!(r#"{{"providers": {body}}}"#),
    )
    .unwrap();
    tmp
}

const ONE_ROW: &str = r#"[{
    "id": "acme",
    "label": "Acme",
    "base_url": "https://api.acme.test",
    "auth_url": "https://acme.test/authorize",
    "token_url": "https://api.acme.test/token"
}]"#;

#[test]
fn an_unavailable_knowhow_dir_yields_no_providers() {
    assert!(load_providers(None).is_empty());
}

#[test]
fn an_absent_registry_file_yields_no_providers() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(load_providers(Some(tmp.path())).is_empty());
}

#[test]
fn a_malformed_registry_file_yields_no_providers() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(REGISTRY_FILE), "{ not json").unwrap();
    assert!(load_providers(Some(tmp.path())).is_empty());
}

#[test]
fn a_registry_file_with_no_providers_key_yields_no_providers() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(REGISTRY_FILE), r#"{"note": "hi"}"#).unwrap();
    assert!(load_providers(Some(tmp.path())).is_empty());
}

#[test]
fn an_omitted_optional_field_parses_as_absent_not_empty() {
    let tmp = registry_dir(ONE_ROW);
    let rows = load_providers(Some(tmp.path()));
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, "acme");
    assert_eq!(row.userinfo_url, None);
    assert_eq!(row.userinfo_method, None);
    assert_eq!(row.authorize_params, None);
    assert_eq!(row.redirect_uri, None);
}

#[test]
fn an_unknown_key_does_not_fail_the_whole_file() {
    let tmp = registry_dir(
        r#"[{
            "id": "acme", "label": "Acme",
            "base_url": "https://api.acme.test",
            "auth_url": "https://acme.test/authorize",
            "token_url": "https://api.acme.test/token",
            "some_future_key": "value"
        }]"#,
    );
    assert_eq!(load_providers(Some(tmp.path())).len(), 1);
}

#[test]
fn find_provider_matches_case_insensitively_and_trims() {
    let tmp = registry_dir(ONE_ROW);
    for spelling in ["acme", "ACME", "  Acme  "] {
        assert!(
            find_provider(Some(tmp.path()), spelling).is_some(),
            "{spelling} should resolve"
        );
    }
}

#[test]
fn find_provider_misses_an_unknown_or_blank_name() {
    let tmp = registry_dir(ONE_ROW);
    assert!(find_provider(Some(tmp.path()), "acme-health").is_none());
    assert!(find_provider(Some(tmp.path()), "").is_none());
    assert!(find_provider(Some(tmp.path()), "   ").is_none());
}

const HINT_MAX_CHARS: usize = 400;

#[test]
fn a_row_matches_its_auth_url_host_and_not_a_lookalike() {
    let rows = load_providers(Some(registry_dir(ONE_ROW).path()));
    assert_eq!(
        row_for_auth_url(&rows, "https://acme.test/authorize?x=1").map(|r| r.id.as_str()),
        Some("acme")
    );
    assert!(row_for_auth_url(&rows, "https://acme.test.evil.example/authorize").is_none());
    assert!(row_for_auth_url(&rows, "https://evil.example/?next=acme.test").is_none());
    assert!(row_for_auth_url(&rows, "not a url").is_none());
}

#[test]
fn a_row_may_declare_its_own_sign_in_scopes() {
    let body = ONE_ROW.replace(
        r#""id": "acme","#,
        r#""id": "acme", "sign_in_scopes": "identity.read","#,
    );
    let rows = load_providers(Some(registry_dir(&body).path()));
    assert_eq!(rows[0].sign_in_scopes.as_deref(), Some("identity.read"));
}

mod guard_tests {
    use super::*;

    fn shipped_dir() -> std::path::PathBuf {
        crate::paths::repo_root()
            .expect("repo root resolves under cargo test")
            .join("system-knowhow")
    }

    fn shipped_rows() -> Vec<OAuthProviderRow> {
        load_providers(Some(shipped_dir().as_path()))
    }

    #[test]
    fn the_shipped_registry_parses_and_every_row_is_usable() {
        let rows = shipped_rows();
        assert!(
            !rows.is_empty(),
            "the shipped oauth-providers.json must list providers"
        );
        for row in &rows {
            assert_eq!(
                row.id,
                row.id.to_lowercase(),
                "{} id must be lowercase",
                row.id
            );
            for (field, value) in [
                ("label", &row.label),
                ("base_url", &row.base_url),
                ("auth_url", &row.auth_url),
                ("token_url", &row.token_url),
            ] {
                assert!(
                    !value.trim().is_empty(),
                    "{} is missing {field}, which the Connect form cannot prefill around",
                    row.id
                );
            }
            if let Some(method) = &row.userinfo_method {
                assert!(
                    matches!(method.as_str(), "GET" | "POST"),
                    "{} declares userinfo_method {method}, which UserinfoMethod::parse reads as GET",
                    row.id
                );
            }
        }
    }

    #[test]
    fn every_shipped_row_names_only_secure_endpoints() {
        for row in shipped_rows() {
            let as_credential = serde_json::to_string(&row).unwrap();
            assert_eq!(
                crate::core::oauth::check_client_endpoints(&row.id, &as_credential),
                Ok(()),
                "{} would prefill a credential the save refuses",
                row.id
            );
        }
    }

    #[test]
    fn every_console_hint_stays_short_enough_to_render_beside_a_button() {
        for row in shipped_rows() {
            for (field, hint) in [
                ("setup_hint", row.setup_hint.as_deref()),
                ("permissions_hint", row.permissions_hint.as_deref()),
            ] {
                let Some(hint) = hint else { continue };
                assert!(
                    hint.chars().count() <= HINT_MAX_CHARS,
                    "{}'s {field} is {} characters, over the {HINT_MAX_CHARS} a hint may be. \
                 It renders verbatim next to a button; the long form belongs in \
                 oauth-providers.md.",
                    row.id,
                    hint.chars().count()
                );
            }
        }
    }

    #[test]
    fn the_knowhow_markdown_restates_no_registry_row() {
        let markdown = std::fs::read_to_string(shipped_dir().join("oauth-providers.md"))
            .expect("oauth-providers.md ships beside the registry");
        for row in shipped_rows() {
            for (field, url) in [
                ("auth_url", Some(row.auth_url.clone())),
                ("token_url", Some(row.token_url.clone())),
                ("userinfo_url", row.userinfo_url.clone()),
            ] {
                let Some(url) = url else { continue };
                assert!(
                    !markdown.contains(&url),
                    "oauth-providers.md restates {}'s {field} ({url}). The rows live in \
                 oauth-providers.json; the markdown carries the prose only.",
                    row.id
                );
            }
        }
    }

    #[test]
    fn oauth_registry_names_no_provider() {
        let source = include_str!("oauth_registry.rs");
        for row in shipped_rows().iter() {
            assert!(
                !source.to_lowercase().contains(&row.id.to_lowercase()),
                "core/oauth_registry.rs names the provider '{}'. Rows belong in \
             system-knowhow/oauth-providers.json, never in engine code.",
                row.id
            );
        }
    }
}
