use super::*;

/// A registry file with `body` as its `providers` array.
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

// ── Degradation (INV-2) ──────────────────────────────────────────────────
//
// Every one of these is a state a shipped install can genuinely be in, so each
// must yield an empty registry rather than an error: the Connect form's manual
// path is unaffected, and autofill is only ever an accelerator.

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

// ── Parsing ──────────────────────────────────────────────────────────────

#[test]
fn an_omitted_optional_field_parses_as_absent_not_empty() {
    // Absent must stay absent all the way to the credential: an empty string
    // written into `userinfo_method` would be a value the flow then has to
    // second-guess, where `None` already means "the engine default".
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
    // The file ships publicly and is hand-edited. One future key must not take
    // autofill out for every provider.
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

// ── Lookup ───────────────────────────────────────────────────────────────

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
    // A derived name is deliberately a miss: the form asks which base provider
    // it runs on rather than guessing from the spelling.
    assert!(find_provider(Some(tmp.path()), "acme-health").is_none());
    assert!(find_provider(Some(tmp.path()), "").is_none());
    assert!(find_provider(Some(tmp.path()), "   ").is_none());
}

// ── The shipped file (INV-3, INV-9) ──────────────────────────────────────

fn shipped_dir() -> std::path::PathBuf {
    crate::paths::repo_root()
        .expect("repo root resolves under cargo test")
        .join("system-knowhow")
}

/// The shipped rows, loaded from the real `system-knowhow/` directory.
fn shipped_rows() -> Vec<OAuthProviderRow> {
    load_providers(Some(shipped_dir().as_path()))
}

#[test]
fn the_shipped_registry_parses_and_every_row_is_usable() {
    // A typo in the data file is invisible until someone presses Connect, and
    // the symptom (an empty form, or an endpoint-less credential) reads as a
    // code bug rather than a bad row.
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
fn the_knowhow_markdown_restates_no_registry_row() {
    // The markdown is the prose beside the registry, not a second copy of it.
    // A table that came back would be free to drift from the rows the engine
    // actually serves, which is the whole failure the split removed.
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
    // CLAUDE.md bans provider-specific instructions in code. The registry is the
    // mechanism that lets this module know five providers without naming one, so
    // a hardcoded fallback creeping in here would quietly undo it.
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
