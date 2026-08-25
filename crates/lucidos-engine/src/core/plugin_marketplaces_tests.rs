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

    let candidates = update_candidates(&catalog);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, "browser-learning");
    assert_eq!(candidates[0].version, "0.2.0");
    assert_eq!(candidates[0].marketplace_id, "community");
}
