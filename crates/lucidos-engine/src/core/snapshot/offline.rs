//! Gateway-bound offline command. No normal engine construction occurs here.
use super::{service::Session, store::Store, ArchiveJobKind, ArchiveJobSlot, BoxError};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

pub struct Arguments {
    workspace: PathBuf,
    root: PathBuf,
    restore_id: Uuid,
}

impl Arguments {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, BoxError> {
        let args: Vec<String> = args.into_iter().collect();
        let mut workspace = None;
        let mut root = None;
        let mut restore_id = None;
        for pair in args.chunks(2) {
            let [flag, value] = pair else {
                return Err("snapshot_restore_arguments_invalid".into());
            };
            let accepted = match flag.as_str() {
                "--workspace-dir" if workspace.is_none() => {
                    workspace = Some(PathBuf::from(value));
                    true
                }
                "--snapshot-root" if root.is_none() => {
                    root = Some(PathBuf::from(value));
                    true
                }
                "--restore-id" if restore_id.is_none() => {
                    restore_id = Some(
                        value
                            .parse::<Uuid>()
                            .map_err(|_| "snapshot_restore_arguments_invalid")?,
                    );
                    true
                }
                _ => false,
            };
            if !accepted {
                return Err("snapshot_restore_arguments_invalid".into());
            }
        }
        Ok(Self {
            workspace: workspace.ok_or("snapshot_restore_arguments_invalid")?,
            root: root.ok_or("snapshot_restore_arguments_invalid")?,
            restore_id: restore_id.ok_or("snapshot_restore_arguments_invalid")?,
        })
    }
}

/// The environment supplies the same database and slug as the stopped engine.
/// The registry, not that environment, authorizes their directory binding.
pub async fn run(args: Arguments, database_url: String) -> Result<(), BoxError> {
    run_inner(args, database_url, false).await
}

/// Gateway restart recovery never continues a partly installed forward restore.
pub async fn recover(args: Arguments, database_url: String) -> Result<(), BoxError> {
    run_inner(args, database_url, true).await
}

async fn run_inner(
    args: Arguments,
    database_url: String,
    recovering: bool,
) -> Result<(), BoxError> {
    async {
        let slug = crate::api::base_path::workspace_id().ok_or("gateway required")?;
        let app_data = lucidos_home::gateway_data_dir().ok_or("gateway registry unavailable")?;
        let request = super::registry_binding::resolve(
            &app_data,
            &args.workspace,
            &database_url,
            &slug,
            args.restore_id,
        )?;
        let database = crate::core::secret_store::boot::database_name(&database_url)?;
        let expected = Store::database_root(&database)?;
        // A caller may name the store, never redirect the engine to another one.
        if args.root.canonicalize()? != expected.canonicalize()? {
            return Err::<_, BoxError>("snapshot root mismatch".into());
        }
        let url = database_url.clone();
        let session = tokio::task::spawn_blocking(move || {
            Session::open_for_database(
                &url,
                Some(&slug),
                &ArchiveJobSlot::default(),
                ArchiveJobKind::SnapshotRestore,
            )
        })
        .await??;
        let session = Arc::new(session.with_restore_reporting());
        if recovering {
            return session
                .reconcile_interrupted_restore(args.restore_id, &args.workspace, &database_url)
                .await;
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await?;
        let result = session
            .run_restore(&pool, &database_url, &args.workspace, request)
            .await;
        pool.close().await;
        result
    }
    .await
    .map_err(|_| "snapshot_restore_failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_each_flag_once_and_never_echoes_invalid_arguments() {
        let id = Uuid::new_v4().to_string();
        let valid = [
            "--workspace-dir",
            "/tmp/workspace",
            "--snapshot-root",
            "/tmp/snapshots",
            "--restore-id",
            &id,
        ];
        assert!(Arguments::parse(valid.map(str::to_owned)).is_ok());
        for invalid in [
            vec![],
            vec!["--workspace-dir"],
            vec!["secret-sentinel", "value"],
            vec!["--restore-id", "secret-sentinel"],
            vec!["--workspace-dir", "/tmp/a", "--workspace-dir", "/tmp/b"],
        ] {
            assert_eq!(
                Arguments::parse(invalid.into_iter().map(str::to_owned))
                    .err()
                    .unwrap()
                    .to_string(),
                "snapshot_restore_arguments_invalid"
            );
        }
    }
}
