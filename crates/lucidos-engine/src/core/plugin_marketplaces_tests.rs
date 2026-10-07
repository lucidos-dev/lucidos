use super::*;

fn write_plugin(root: &Path, id: &str, version: &str, name: &str) {
    std::fs::create_dir_all(root.join("knowhow")).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        format!(
            r#"
id = "{id}"
version = "{version}"
name = "{name}"
description = "Test plugin"
"#
        ),
    )
    .unwrap();
    std::fs::write(root.join("knowhow/guide.md"), "---\nname: Guide\n---\nBody").unwrap();
}

fn commit_all(repo_dir: &Path) {
    let repo = git2::Repository::init(repo_dir).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Lucidos Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .unwrap();
}

fn add_local_marketplace(registry: &mut PluginMarketplaceRegistry, repo_dir: &Path) -> String {
    let source = format!("file://{}", repo_dir.join(".git").display());
    add_marketplace(registry, &source, Some("Local Marketplace")).unwrap();
    source
}

#[test]
fn add_marketplace_dedupes_by_stable_source_id() {
    let mut registry = PluginMarketplaceRegistry::default();
    let source = "https://github.com/lucidos-dev/plugins";

    let (first, created) = add_marketplace(&mut registry, source, Some("Lucidos Plugins")).unwrap();
    assert!(created);

    let (second, created) = add_marketplace(
        &mut registry,
        "https://github.com/lucidos-dev/plugins.git",
        Some("Core Plugins"),
    )
    .unwrap();
    assert!(!created);
    assert_eq!(first.id, second.id);
    assert_eq!(registry.marketplaces.len(), 1);
    assert_eq!(registry.marketplaces[0].name, "Core Plugins");
}

#[test]
fn install_source_appends_plugin_path_to_github_tree_marketplace() {
    let parsed =
        parse_marketplace_source("https://github.com/lucidos-dev/plugins/tree/main/community")
            .unwrap();

    assert_eq!(
        install_source(&parsed, &Some("main".to_string()), "browser-learning").as_deref(),
        Some("https://github.com/lucidos-dev/plugins/tree/main/community/browser-learning")
    );
}

#[test]
fn add_marketplace_rejects_unsafe_github_tree_subpath() {
    let mut registry = PluginMarketplaceRegistry::default();
    let err = add_marketplace(
        &mut registry,
        "https://github.com/lucidos-dev/plugins/tree/main/../../outside",
        None,
    )
    .unwrap_err();

    assert!(err.contains("subpath must stay inside"));
}

#[test]
fn scan_catalog_reports_marketplace_with_only_invalid_manifests() {
    let workspace = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo_dir.path().join("knowhow")).unwrap();
    std::fs::write(
        repo_dir.path().join("manifest.toml"),
        r#"id = "broken-plugin""#,
    )
    .unwrap();
    std::fs::write(
        repo_dir.path().join("knowhow/guide.md"),
        "---\nname: Guide\n---\nBody",
    )
    .unwrap();
    commit_all(repo_dir.path());

    let mut registry = PluginMarketplaceRegistry::default();
    add_local_marketplace(&mut registry, repo_dir.path());

    let catalog = scan_catalog(workspace.path(), &registry, &[], &GitCredentials::none());

    assert!(catalog.plugins.is_empty());
    assert_eq!(catalog.errors.len(), 1);
    assert!(catalog.errors[0].error.contains("no valid plugins"));
}

#[test]
fn scan_catalog_discovers_root_plugin_from_git_marketplace() {
    let workspace = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    write_plugin(
        repo_dir.path(),
        "browser-learning",
        "0.1.0",
        "Browser Learning",
    );
    commit_all(repo_dir.path());

    let mut registry = PluginMarketplaceRegistry::default();
    let source = add_local_marketplace(&mut registry, repo_dir.path());

    let catalog = scan_catalog(workspace.path(), &registry, &[], &GitCredentials::none());

    assert!(catalog.errors.is_empty(), "errors: {:?}", catalog.errors);
    assert_eq!(catalog.plugins.len(), 1);
    let plugin = &catalog.plugins[0];
    assert_eq!(plugin.id, "browser-learning");
    assert_eq!(plugin.name, "Browser Learning");
    assert_eq!(plugin.version, "0.1.0");
    assert_eq!(plugin.source, source);
    assert_eq!(plugin.content, vec!["knowhow"]);
    assert_eq!(plugin.status, MarketplacePluginStatus::Available);
}

#[test]
fn find_manifest_roots_skips_build_output_dirs() {
    let repo_dir = tempfile::tempdir().unwrap();
    write_plugin(&repo_dir.path().join("real"), "real", "0.1.0", "Real");
    for vendored in ["node_modules", "dist", "__pycache__", "venv"] {
        write_plugin(
            &repo_dir.path().join(vendored).join("pkg"),
            "vendored",
            "0.1.0",
            "Vendored",
        );
    }

    let roots = find_manifest_roots(repo_dir.path()).unwrap();

    assert_eq!(roots, vec![repo_dir.path().join("real")]);
}

fn catalog_plugin(
    id: &str,
    version: &str,
    status: MarketplacePluginStatus,
    marketplace_id: &str,
) -> MarketplacePlugin {
    MarketplacePlugin {
        marketplace_id: marketplace_id.to_string(),
        marketplace_name: marketplace_id.to_string(),
        id: id.to_string(),
        name: id.to_string(),
        description: "Test plugin".to_string(),
        version: version.to_string(),
        source: format!("file:///{}.git", marketplace_id),
        manifest: serde_json::json!({
            "id": id,
            "version": version,
            "name": id,
            "description": "Test plugin"
        }),
        content: vec!["knowhow".to_string()],
        categories: vec![],
        files_count: 1,
        status,
        installed_version: Some("0.1.0".to_string()),
        setup_thread_id: None,
        setup_complete: false,
        app_id: None,
        modified: false,
        modified_paths: vec![],
        engine_requirement: None,
        engine_compatible: true,
        engine_incompatible_reason: None,
    }
}

#[test]
fn update_candidates_returns_newest_update_per_plugin() {
    let catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![
            catalog_plugin(
                "browser-learning",
                "0.1.1",
                MarketplacePluginStatus::UpdateAvailable,
                "core",
            ),
            catalog_plugin(
                "browser-learning",
                "0.2.0",
                MarketplacePluginStatus::UpdateAvailable,
                "community",
            ),
            catalog_plugin(
                "already-fresh",
                "0.1.0",
                MarketplacePluginStatus::Installed,
                "core",
            ),
            catalog_plugin(
                "new-plugin",
                "0.1.0",
                MarketplacePluginStatus::Available,
                "core",
            ),
        ],
        errors: vec![],
    };

    let candidates = update_candidates(&catalog, &Ok(semver::Version::new(0, 46, 1)));

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, "browser-learning");
    assert_eq!(candidates[0].version, "0.2.0");
    assert_eq!(candidates[0].marketplace_id, "community");
}

fn requiring(mut plugin: MarketplacePlugin, engine: &str) -> MarketplacePlugin {
    plugin.manifest["engine"] = serde_json::json!(engine);
    plugin
}

/// No "update available" notification for a version this release cannot
/// install. A lower version it can install still counts.
#[test]
fn update_candidates_skip_a_version_the_running_release_cannot_install() {
    let catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![
            catalog_plugin(
                "theme-studio",
                "0.1.1",
                MarketplacePluginStatus::UpdateAvailable,
                "core",
            ),
            requiring(
                catalog_plugin(
                    "theme-studio",
                    "0.2.0",
                    MarketplacePluginStatus::UpdateAvailable,
                    "community",
                ),
                ">=0.99.0",
            ),
            requiring(
                catalog_plugin(
                    "needs-newer",
                    "0.2.0",
                    MarketplacePluginStatus::UpdateAvailable,
                    "core",
                ),
                ">=0.99.0",
            ),
        ],
        errors: vec![],
    };

    let on_046 = update_candidates(&catalog, &Ok(semver::Version::new(0, 46, 1)));
    assert_eq!(on_046.len(), 1);
    assert_eq!(on_046[0].id, "theme-studio");
    assert_eq!(on_046[0].version, "0.1.1");

    let on_099 = update_candidates(&catalog, &Ok(semver::Version::new(0, 99, 0)));
    let versions: Vec<(&str, &str)> = on_099
        .iter()
        .map(|p| (p.id.as_str(), p.version.as_str()))
        .collect();
    assert_eq!(
        versions,
        vec![("needs-newer", "0.2.0"), ("theme-studio", "0.2.0")]
    );
}

#[test]
fn engine_compatibility_overlay_fills_the_catalog_row_fields() {
    let row = |status| {
        requiring(
            catalog_plugin("theme-studio", "0.1.0", status, "core"),
            ">=0.46.1",
        )
    };
    let mut catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![
            row(MarketplacePluginStatus::Available),
            catalog_plugin("plain", "0.1.0", MarketplacePluginStatus::Available, "core"),
        ],
        errors: vec![],
    };

    apply_engine_compatibility_to_catalog(&mut catalog, &Ok(semver::Version::new(0, 46, 0)));
    let blocked = &catalog.plugins[0];
    assert_eq!(blocked.engine_requirement.as_deref(), Some(">=0.46.1"));
    assert!(!blocked.engine_compatible);
    assert_eq!(
        blocked.engine_incompatible_reason.as_deref(),
        Some("Needs Lucidos 0.46.1 or later")
    );
    let plain = &catalog.plugins[1];
    assert_eq!(plain.engine_requirement, None);
    assert!(plain.engine_compatible);
    assert_eq!(plain.engine_incompatible_reason, None);

    // The same rows re-served by a newer engine flip back, so a verdict stored
    // in the cache can never outlive an upgrade.
    apply_engine_compatibility_to_catalog(&mut catalog, &Ok(semver::Version::new(0, 46, 1)));
    assert!(catalog.plugins[0].engine_compatible);
    assert_eq!(catalog.plugins[0].engine_incompatible_reason, None);
}

#[test]
fn engine_compatibility_fields_are_on_the_wire() {
    let mut plugin = requiring(
        catalog_plugin(
            "theme-studio",
            "0.1.0",
            MarketplacePluginStatus::Available,
            "core",
        ),
        ">=0.46.1",
    );
    apply_engine_compatibility(&mut plugin, &Ok(semver::Version::new(0, 46, 0)));
    let wire = serde_json::to_value(&plugin).unwrap();
    assert_eq!(wire["engine_requirement"], ">=0.46.1");
    assert_eq!(wire["engine_compatible"], false);
    assert_eq!(
        wire["engine_incompatible_reason"],
        "Needs Lucidos 0.46.1 or later"
    );
}

/// A row with no `engine` carries no `engine_requirement`, and still installs.
/// A malformed value counts as declared, so it is refused and never shown as
/// "no version requirement".
#[test]
fn engine_requirement_is_absent_only_when_the_manifest_declares_none() {
    let running = Ok(semver::Version::new(0, 46, 0));
    let mut undeclared =
        catalog_plugin("plain", "0.1.0", MarketplacePluginStatus::Available, "core");
    apply_engine_compatibility(&mut undeclared, &running);
    let wire = serde_json::to_value(&undeclared).unwrap();
    assert!(wire.get("engine_requirement").is_none(), "got {wire}");
    assert_eq!(wire["engine_compatible"], true);

    let mut malformed = catalog_plugin("typo", "0.1.0", MarketplacePluginStatus::Available, "core");
    malformed.manifest["engine"] = serde_json::json!(46);
    apply_engine_compatibility(&mut malformed, &running);
    assert_eq!(malformed.engine_requirement.as_deref(), Some("46"));
    assert!(!malformed.engine_compatible);
}

/// Install state is live, never cached. The *plugin catalog cache* holds rows
/// scanned minutes ago, so a plugin installed since then still reads
/// `Available` in the cache. Serving that unchanged puts an Install button on a
/// plugin already on disk, for as long as the TTL lasts.
mod installed_state_overlay {
    use super::*;

    fn cached_row(id: &str, version: &str) -> MarketplacePlugin {
        MarketplacePlugin {
            marketplace_id: "community".to_string(),
            marketplace_name: "Community".to_string(),
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            version: version.to_string(),
            source: "https://example.test/community".to_string(),
            manifest: serde_json::json!({}),
            content: vec![],
            categories: vec![],
            files_count: 1,
            status: MarketplacePluginStatus::Available,
            installed_version: None,
            setup_thread_id: None,
            setup_complete: false,
            app_id: None,
            modified: false,
            modified_paths: vec![],
            engine_requirement: None,
            engine_compatible: true,
            engine_incompatible_reason: None,
        }
    }

    fn installed(id: &str, version: &str) -> InstalledPluginSummary {
        InstalledPluginSummary {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            source: None,
            setup_thread_id: Some("thread-1".to_string()),
            app_id: Some("an-app".to_string()),
            content: vec![],
            files: vec![],
            modified: true,
            modified_paths: vec!["apps/an-app/index.html".to_string()],
            engine_requirement: None,
        }
    }

    fn catalog_of(plugins: Vec<MarketplacePlugin>) -> MarketplaceCatalog {
        MarketplaceCatalog {
            marketplaces: vec![],
            plugins,
            errors: vec![],
        }
    }

    #[test]
    fn a_plugin_installed_since_the_scan_stops_offering_install() {
        let mut catalog = catalog_of(vec![cached_row("browser-learning", "0.1.0")]);

        apply_installed_state_to_catalog(&mut catalog, &[installed("browser-learning", "0.1.0")]);

        let row = &catalog.plugins[0];
        assert_eq!(row.status, MarketplacePluginStatus::Installed);
        assert_eq!(row.installed_version.as_deref(), Some("0.1.0"));
        assert_eq!(row.app_id.as_deref(), Some("an-app"));
        assert_eq!(row.setup_thread_id.as_deref(), Some("thread-1"));
        assert!(row.modified);
        assert_eq!(row.modified_paths, vec!["apps/an-app/index.html"]);
    }

    /// An older install against a newer catalog row is an update, not a plain
    /// install: the card offers Update rather than Install.
    #[test]
    fn an_older_install_reads_as_an_update() {
        let mut catalog = catalog_of(vec![cached_row("browser-learning", "0.2.0")]);

        apply_installed_state_to_catalog(&mut catalog, &[installed("browser-learning", "0.1.0")]);

        assert_eq!(
            catalog.plugins[0].status,
            MarketplacePluginStatus::UpdateAvailable
        );
        assert_eq!(
            catalog.plugins[0].installed_version.as_deref(),
            Some("0.1.0")
        );
    }

    /// The overlay also has to CLEAR state, not only set it. A cache written
    /// while the plugin was installed must not keep saying so after uninstall.
    #[test]
    fn a_plugin_uninstalled_since_the_scan_goes_back_to_available() {
        let mut stale = cached_row("browser-learning", "0.1.0");
        stale.status = MarketplacePluginStatus::Installed;
        stale.installed_version = Some("0.1.0".to_string());
        stale.app_id = Some("an-app".to_string());
        stale.modified = true;
        stale.modified_paths = vec!["apps/an-app/index.html".to_string()];
        let mut catalog = catalog_of(vec![stale]);

        apply_installed_state_to_catalog(&mut catalog, &[]);

        let row = &catalog.plugins[0];
        assert_eq!(row.status, MarketplacePluginStatus::Available);
        assert_eq!(row.installed_version, None);
        assert_eq!(row.app_id, None);
        assert!(!row.modified);
        assert!(row.modified_paths.is_empty());
    }
}
