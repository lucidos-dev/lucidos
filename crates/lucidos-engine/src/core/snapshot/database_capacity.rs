//! Read-only capacity probes bound to the PostgreSQL instance being restored.
use super::{store::check_floor, BoxError};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// SQL is expanded without retaining it in RAM or writing it to disk. Compressed
/// archive length is not an estimate of the rows and indexes replay will build.
pub fn expanded_dump_bytes(dump: &Path, url: &str) -> Result<u64, BoxError> {
    let mut child = crate::core::backup::pg_tool_command("pg_restore", url)?
        .args(["--no-owner", "--no-acl", "--file=-"])
        .arg(dump)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = std::io::copy(
        &mut child
            .stdout
            .take()
            .ok_or("snapshot_capacity_probe_failed")?,
        &mut std::io::sink(),
    );
    let status = child.wait()?;
    if !status.success() {
        return Err("snapshot_capacity_probe_failed".into());
    }
    Ok(result?)
}

pub struct DatabaseCapacity {
    paths: Vec<PathBuf>,
    identity: String,
    data: PathBuf,
    pub reservation: u64,
    pub recovery_bytes: u64,
}

impl DatabaseCapacity {
    pub async fn discover(pool: &sqlx::PgPool, expanded: u64) -> Result<Self, BoxError> {
        let (data, identity, current, wal): (String, String, i64, i64) = sqlx::query_as(
            "SELECT current_setting('data_directory'), system_identifier::text || ':' || split_part(pg_read_file('postmaster.pid'), chr(10), 3), pg_database_size(current_database()), pg_size_bytes(current_setting('max_wal_size')) FROM pg_control_system()"
        ).fetch_one(pool).await?;
        let mut paths = vec![PathBuf::from(&data), PathBuf::from(&data).join("pg_wal")];
        let tablespaces: Vec<String> = sqlx::query_scalar("SELECT pg_tablespace_location(oid) FROM pg_tablespace WHERE pg_tablespace_location(oid) <> ''").fetch_all(pool).await?;
        paths.extend(tablespaces.into_iter().map(PathBuf::from));
        Ok(Self {
            paths,
            identity,
            data: data.into(),
            reservation: replay_reservation(expanded, current.try_into()?, wal.try_into()?)?,
            recovery_bytes: current.try_into()?,
        })
    }

    /// A native probe is allowed only when pg_control and postmaster.pid bind
    /// the SQL-reported directory to the running instance. Otherwise discover
    /// the local Docker container by its published port and verify its identity.
    pub fn native_paths(&self) -> Result<Option<&[PathBuf]>, BoxError> {
        let identity = match read_native_identity(&self.data) {
            Ok(identity) => identity,
            // An inaccessible host path is not evidence about a container's
            // filesystem. Try the independently identity-bound Docker probe.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error.into()),
        };
        if identity != self.identity {
            return Ok(None);
        }
        Ok(Some(&self.paths))
    }

    pub fn check_docker(&self, url: &str, host_reservation: u64) -> Result<(), BoxError> {
        let target = lucidos_pg_url::PgUrl::parse(url)?;
        let host = match &target.host {
            lucidos_pg_url::Host::Tcp(host) => host,
            _ => return Err("snapshot_capacity_local_cluster_required".into()),
        };
        use std::net::ToSocketAddrs;
        let addresses = (host.as_str(), target.port)
            .to_socket_addrs()?
            .collect::<Vec<_>>();
        if addresses.is_empty() || addresses.iter().any(|address| !address.ip().is_loopback()) {
            return Err("snapshot_capacity_local_cluster_required".into());
        }
        let candidates = output(Command::new("docker").args(["ps", "--format", "{{.ID}}"]))?;
        let mut matched = Vec::new();
        for container in candidates.lines() {
            let inspect = output(Command::new("docker").args([
                "inspect",
                "--format",
                "{{json .NetworkSettings.Ports}}",
                container,
            ]))?;
            if !published_port_matches(&inspect, target.port)? {
                continue;
            }

            // Read cluster files directly rather than forwarding database
            // credentials to a container whose identity is not yet verified.
            let control = Command::new("docker")
                .args(["exec", container, "cat", "--"])
                .arg(self.data.join("global/pg_control"))
                .output()?;
            let pid = Command::new("docker")
                .args(["exec", container, "cat", "--"])
                .arg(self.data.join("postmaster.pid"))
                .output()?;
            if !control.status.success() || !pid.status.success() {
                return Err("snapshot_capacity_probe_failed".into());
            }
            let bytes: [u8; 8] = control
                .stdout
                .get(..8)
                .ok_or("snapshot_capacity_probe_failed")?
                .try_into()?;
            if control_identity(bytes, &String::from_utf8(pid.stdout)?)? == self.identity {
                matched.push(container);
            }
        }
        let [container] = matched.as_slice() else {
            return Err("snapshot_capacity_cluster_mismatch".into());
        };
        // Docker Desktop stores volumes in a VM. Host statfs on a container
        // path is invalid. Probe inside the verified container instead. Charge
        // all host reservations too: bind mounts may overlap host volumes and
        // we cannot safely equate a VM device number with a host device number.
        let required = self
            .reservation
            .checked_add(host_reservation)
            .ok_or("snapshot_disk_floor")?;
        for path in &self.paths {
            let stats = output(
                Command::new("docker")
                    .args(["exec", container, "stat", "-f", "-c", "%a %S %b", "--"])
                    .arg(path),
            )?;
            let (available, total) = parse_stat(&stats)?;
            check_floor(available, total, required)?;
        }
        Ok(())
    }
}

/// Forward and rollback can each retain an old relation set until commit.
/// Budget four times the larger expanded SQL/current DB estimate for relations,
/// indexes and replay, plus two WAL targets. max_wal_size is a soft limit.
/// The estimate includes the ordinary free-space floor.
/// Other cluster tenants can still exhaust space with unlimited concurrent writes.
fn replay_reservation(expanded: u64, current: u64, wal: u64) -> Result<u64, BoxError> {
    expanded
        .max(current)
        .checked_mul(4)
        .and_then(|bytes| wal.checked_mul(2).and_then(|wal| bytes.checked_add(wal)))
        .ok_or_else(|| "snapshot_disk_floor".into())
}

fn read_native_identity(data: &Path) -> Result<String, std::io::Error> {
    let mut bytes = [0u8; 8];
    std::fs::File::open(data.join("global/pg_control"))?.read_exact(&mut bytes)?;
    let pid = std::fs::read_to_string(data.join("postmaster.pid"))?;
    control_identity(bytes, &pid)
}

fn control_identity(bytes: [u8; 8], pid: &str) -> Result<String, std::io::Error> {
    let start = pid.lines().nth(2).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid postmaster identity",
        )
    })?;
    Ok(format!("{}:{start}", u64::from_ne_bytes(bytes)))
}

fn published_port_matches(json: &str, port: u16) -> Result<bool, BoxError> {
    let ports: serde_json::Value = serde_json::from_str(json)?;
    let ports = ports.as_object().ok_or("snapshot_capacity_probe_failed")?;
    for bindings in ports.values() {
        if bindings.is_null() {
            continue;
        }
        for binding in bindings
            .as_array()
            .ok_or("snapshot_capacity_probe_failed")?
        {
            let published = binding
                .get("HostPort")
                .and_then(|value| value.as_str())
                .ok_or("snapshot_capacity_probe_failed")?
                .parse::<u16>()?;
            if published == port {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn output(command: &mut Command) -> Result<String, BoxError> {
    let result = command.output()?;
    if !result.status.success() {
        return Err("snapshot_capacity_probe_failed".into());
    }
    Ok(String::from_utf8(result.stdout)?)
}

fn parse_stat(text: &str) -> Result<(u64, u64), BoxError> {
    let fields = text
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()?;
    let [available, block, total] = fields.as_slice() else {
        return Err("snapshot_capacity_probe_failed".into());
    };
    if *block == 0 || available > total {
        return Err("snapshot_capacity_probe_failed".into());
    }
    Ok((
        available.checked_mul(*block).ok_or("snapshot_disk_floor")?,
        total.checked_mul(*block).ok_or("snapshot_disk_floor")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn disposable_cluster_probe_and_expansion_are_bound_and_read_only() {
        use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
        let (pool, database) = setup_test_db().await;
        sqlx::raw_sql("CREATE TABLE capacity_payload(value text); INSERT INTO capacity_payload SELECT repeat('a', 1000000)").execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let dump = root.path().join("db.dump");
        let url = test_db_url(&database);
        let worker_url = url.clone();
        let worker_dump = dump.clone();
        let expanded = tokio::task::spawn_blocking(move || {
            let status = crate::core::backup::pg_tool_command("pg_dump", &worker_url)
                .unwrap()
                .args(["--format=custom", "--file"])
                .arg(&worker_dump)
                .status()
                .unwrap();
            assert!(status.success());
            expanded_dump_bytes(&worker_dump, &worker_url).unwrap()
        })
        .await
        .unwrap();
        assert!(expanded > std::fs::metadata(&dump).unwrap().len());
        assert!(expanded > 1_000_000);
        let mut probe = DatabaseCapacity::discover(&pool, expanded).await.unwrap();
        assert!(probe.recovery_bytes > 0);
        // Check authoritative paths on the disposable test cluster only. Keep
        // this plumbing test independent of the host's actual free capacity.
        probe.reservation = u64::MAX;
        tokio::task::spawn_blocking(move || {
            assert!(super::super::capacity::preflight_database(
                root.path(),
                root.path(),
                0,
                0,
                Some((&probe, &url))
            )
            .is_err());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
            probe.reservation = 0;
            if let Some(paths) = probe.native_paths().unwrap() {
                for path in paths {
                    assert!(fs2::total_space(path).unwrap() > 0);
                }
            } else {
                probe.check_docker(&url, 0).unwrap();
                probe.identity.push_str("-different-instance");
                assert!(probe.check_docker(&url, 0).is_err());
            }
        })
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM capacity_payload")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
        teardown_test_db(&database).await;
    }
    #[test]
    fn docker_discovery_uses_published_host_port() {
        let ports = r#"{"5432/tcp":[{"HostIp":"127.0.0.1","HostPort":"5510"}],"80/tcp":null}"#;
        assert!(published_port_matches(ports, 5510).unwrap());
        assert!(!published_port_matches(ports, 5432).unwrap());
        assert!(published_port_matches("{}", 5510).is_ok());
        assert!(published_port_matches("invalid", 5510).is_err());
    }
    #[test]
    fn expanded_and_current_relation_estimates_cover_both_directions() {
        assert_eq!(replay_reservation(100, 10, 20).unwrap(), 440);
        assert_eq!(replay_reservation(10, 100, 20).unwrap(), 440);
        assert!(replay_reservation(u64::MAX, 0, 0).is_err());
        assert!(replay_reservation(0, 0, u64::MAX).is_err());
    }
    #[test]
    fn container_space_is_block_scaled_and_strict() {
        assert_eq!(parse_stat("10 4096 100\n").unwrap(), (40960, 409600));
        for invalid in [
            "",
            "10 0 100",
            "101 1 100",
            "10 4096",
            "x 1 100",
            "10 4096 100 extra",
            "18446744073709551615 4096 18446744073709551615",
        ] {
            assert!(parse_stat(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn native_paths_require_running_cluster_identity() {
        let root = tempfile::tempdir().unwrap();
        let probe = DatabaseCapacity {
            paths: vec![root.path().into()],
            data: root.path().into(),
            identity: "42:123".into(),
            reservation: 0,
            recovery_bytes: 0,
        };
        assert!(probe.native_paths().unwrap().is_none());
        std::fs::create_dir(root.path().join("global")).unwrap();
        std::fs::write(root.path().join("global/pg_control"), 42u64.to_ne_bytes()).unwrap();
        std::fs::write(root.path().join("postmaster.pid"), "1\npath\n123\n").unwrap();
        assert_eq!(probe.native_paths().unwrap().unwrap(), probe.paths);
        std::fs::write(root.path().join("postmaster.pid"), "1\npath\n124\n").unwrap();
        assert!(probe.native_paths().unwrap().is_none());
    }
}
