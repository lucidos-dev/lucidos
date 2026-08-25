//! Shared fixtures + helpers for the plugin test suite. Split out of the
//! monolithic `plugins_tests.rs`; the concern modules pull these in via
//! `use super::helpers::*`.

use super::*;
use std::io::Write;

/// Install a plugin by source string (git URL, GitHub tree URL, or
/// `.lucidos-plugin` archive path). Detects the shape, fetches into a temp
/// dir, then delegates to `install_from_unpacked_with_bus`. Test-only entry
/// point — production install runs through `prepare_install_request` +
/// `confirm_pending_install`, which surfaces the user-confirm panel before
/// any bytes touch `data/`.
pub(super) async fn install_from_source_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    source_str: &str,
    overwrite: bool,
) -> Result<String, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, source_type) = fetch_source(workspace_path, &source)?;
    let write = install_from_unpacked_with_bus(
        workspace_path,
        bus,
        &plugin_root,
        InstallContext::plain(source_type, overwrite),
    )
    .await?;
    Ok(write.summary)
}

pub(super) const FIXTURE_MANIFEST: &str = r#"
id = "fixture-plugin"
version = "0.1.0"
name = "Fixture Plugin"
description = "test"
source = "https://github.com/x/y"
"#;

pub(super) fn build_archive(
    tmp: &Path,
    archive_name: &str,
    manifest: &str,
    files: &[(&str, &[u8])],
) -> PathBuf {
    let archive_path = tmp.join(archive_name);
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.toml", opts).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    for (path, body) in files {
        zip.start_file(*path, opts).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    archive_path
}

pub(super) fn build_fixture_archive(tmp: &Path, knowhow_body: &str) -> PathBuf {
    build_archive(
        tmp,
        "fixture.lucidos-plugin",
        FIXTURE_MANIFEST,
        &[
            ("knowhow/fixture.md", knowhow_body.as_bytes()),
            (
                "triggers/fixture/fixture.md",
                b"---\nname: Fixture\n---\nrun me",
            ),
        ],
    )
}

pub(super) fn extract_to(tmp: &Path, archive: &Path) -> PathBuf {
    let dest = tmp.join("unpacked");
    std::fs::create_dir_all(&dest).unwrap();
    super::source::extract_zip(archive, &dest).unwrap();
    dest
}

pub(super) fn fresh_workspace() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("lucidos_plugins_int_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(p.join("data")).unwrap();
    // Production workspaces are git repos rooted at the workspace dir (the
    // ArtifactManager opens/inits one), and plugin install now commits the
    // files it writes into `data/`. Init a repo here so the writer's commit
    // step has somewhere to land — matching the real workspace layout.
    git2::Repository::init(&p).unwrap();
    p
}

/// Fetch the persisted payload(s) for an aggregate id, oldest first, so
/// tests can index by emit order.
pub(super) async fn read_events(
    pool: &sqlx::PgPool,
    event_type: &str,
    id: &str,
) -> Vec<serde_json::Value> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        r#"SELECT payload FROM events
           WHERE event_type = $1 AND aggregate_id = $2
           ORDER BY sequence ASC"#,
    )
    .bind(event_type)
    .bind(id)
    .fetch_all(pool)
    .await
    .expect("query events");
    rows.into_iter().map(|(p,)| p).collect()
}
