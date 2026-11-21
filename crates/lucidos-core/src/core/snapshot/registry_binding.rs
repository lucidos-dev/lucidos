//! Read-only binding of offline restore to the gateway's authoritative registry.
use super::{service::OfflineRestoreRequest, BoxError};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Deserialize)]
struct Registry {
    #[serde(default)]
    version: u32,
    workspaces: Vec<Workspace>,
}

#[derive(Deserialize)]
struct Workspace {
    id: String,
    name: String,
    dir: PathBuf,
}

/// Read only the local gateway registry. Legacy database URLs are migration
/// sources, never the identity of a shared-cluster database.
pub fn resolve(
    app_data: &Path,
    workspace: &Path,
    database_url: &str,
    gateway_slug: &str,
    restore_id: Uuid,
) -> Result<OfflineRestoreRequest, BoxError> {
    resolve_inner(app_data, workspace, database_url, gateway_slug, restore_id)
        .map_err(|_| "snapshot_restore_binding_mismatch".into())
}

fn resolve_inner(
    app_data: &Path,
    workspace: &Path,
    database_url: &str,
    gateway_slug: &str,
    restore_id: Uuid,
) -> Result<OfflineRestoreRequest, BoxError> {
    let expected = lucidos_gateway_constants::workspace_database_name(gateway_slug)?;
    // PostgreSQL truncates identifiers beyond this boundary. Never restore a
    // legacy overlong slug into a database that might belong to another slug.
    if expected.len() > lucidos_pg_url::PG_MAX_IDENTIFIER_BYTES
        || crate::core::secret_store::boot::database_name(database_url)? != expected
    {
        return Err("database mismatch".into());
    }
    let registry: Registry =
        serde_json::from_slice(&std::fs::read(app_data.join("config/workspaces.json"))?)?;
    if registry.version > 1 {
        return Err("unsupported registry version".into());
    }
    let mut matches = registry
        .workspaces
        .iter()
        .filter(|entry| entry.id == gateway_slug);
    let entry = matches.next().ok_or("workspace not registered")?;
    if matches.next().is_some() {
        return Err("ambiguous registry identity".into());
    }
    let registered = app_data.join(&entry.dir).canonicalize()?;
    if !registered.is_dir() || registered != workspace.canonicalize()? {
        return Err("workspace directory mismatch".into());
    }
    Ok(OfflineRestoreRequest {
        restore_id,
        gateway_slug: entry.id.clone(),
        workspace_name: entry.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_authority_for_directory_database_and_display_name() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path();
        let workspace = app.join("workspaces/demo");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir(app.join("config")).unwrap();
        let registry = app.join("config/workspaces.json");
        let entry = serde_json::json!({
            "id": "demo", "name": "Renamed workspace", "dir": "workspaces/demo",
            "database_url": "postgres://ignored/legacy"
        });
        let write = |entries: serde_json::Value| {
            std::fs::write(
                &registry,
                serde_json::to_vec(&serde_json::json!({
                    "version": 1, "workspaces": entries
                }))
                .unwrap(),
            )
            .unwrap();
        };
        write(serde_json::json!([entry]));
        let id = Uuid::new_v4();
        let resolve = |path: &Path, url: &str, slug: &str| super::resolve(app, path, url, slug, id);
        let request = resolve(&workspace, "postgres://localhost/lucidos_demo", "demo").unwrap();
        assert_eq!(request.workspace_name, "Renamed workspace");
        assert_eq!(request.gateway_slug, "demo");
        assert_eq!(request.restore_id, id);
        for (path, url, slug) in [
            (app, "postgres://localhost/lucidos_demo", "demo"),
            (workspace.as_path(), "postgres://localhost/legacy", "demo"),
            (
                workspace.as_path(),
                "postgres://localhost/lucidos_other",
                "other",
            ),
            (
                workspace.as_path(),
                "postgres://localhost/lucidos_demo",
                "database:demo",
            ),
            (workspace.as_path(), "invalid private connection", "demo"),
        ] {
            assert_eq!(
                resolve(path, url, slug).err().unwrap().to_string(),
                "snapshot_restore_binding_mismatch"
            );
        }
        write(serde_json::json!([entry, entry]));
        assert!(resolve(&workspace, "postgres://localhost/lucidos_demo", "demo").is_err());
        write(serde_json::json!([{
            "id": "demo", "name": "Absolute", "dir": workspace
        }]));
        assert!(resolve(&workspace, "postgres://localhost/lucidos_demo", "demo").is_ok());
        let alias = app.join("alias");
        std::os::unix::fs::symlink(&workspace, &alias).unwrap();
        assert!(resolve(&alias, "postgres://localhost/lucidos_demo", "demo").is_ok());
        // A formerly valid registry id may exceed PostgreSQL's identifier
        // length. Even its exact URL cannot authorize a truncated database.
        let long_slug = "a".repeat(64);
        write(serde_json::json!([{
            "id": long_slug, "name": "Legacy", "dir": workspace
        }]));
        assert!(resolve(
            &workspace,
            &format!("postgres://localhost/lucidos_{long_slug}"),
            &long_slug
        )
        .is_err());
        std::fs::write(&registry, br#"{"version":2,"workspaces":[]}"#).unwrap();
        assert!(resolve(&workspace, "postgres://localhost/lucidos_demo", "demo").is_err());
        std::fs::remove_file(registry).unwrap();
        assert!(resolve(&workspace, "postgres://localhost/lucidos_demo", "demo").is_err());
    }
}
