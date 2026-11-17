//! Real local authentication tests, enabled with the bundled Postgres toolchain.

use super::*;
use sqlx::Connection as _;

struct Cluster {
    _root: tempfile::TempDir,
    bin: PathBuf,
    lib: PathBuf,
    data: PathBuf,
    socket: PathBuf,
    url: String,
}

impl Cluster {
    fn start() -> Option<Self> {
        let (Some(bin), Some(lib)) = (
            std::env::var_os("LUCIDOS_PG_BIN_DIR"),
            std::env::var_os("LUCIDOS_PG_LIB_DIR"),
        ) else {
            crate::log!("[Sandbox test] Native authentication tests need LUCIDOS_PG_BIN_DIR and LUCIDOS_PG_LIB_DIR");
            return None;
        };
        let root = tempfile::Builder::new()
            .prefix("lucidos-auth-")
            .tempdir_in("/tmp")
            .unwrap();
        let top = std::fs::canonicalize(root.path()).unwrap();
        let data = top.join("data");
        let socket = top.join("socket");
        std::fs::create_dir(&socket).unwrap();
        let url = lucidos_pg_url::PgUrl {
            user: Some("sandbox_admin".into()),
            password: Some("fixture-admin-password".into()),
            host: lucidos_pg_url::Host::Socket(socket.clone()),
            port: 5432,
            database: "postgres".into(),
            params: vec![],
        }
        .read_only_url();
        let c = Self {
            _root: root,
            bin: bin.into(),
            lib: lib.into(),
            data,
            socket,
            url,
        };
        let out = c
            .command("initdb")
            .arg("-D")
            .arg(&c.data)
            .args([
                "-U",
                "sandbox_admin",
                "--auth-local=trust",
                "--auth-host=reject",
                "--encoding=UTF8",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let options = format!(
            "-c listen_addresses='' -c unix_socket_directories='{}'",
            c.socket.display()
        );
        let out = c
            .command("pg_ctl")
            .arg("-D")
            .arg(&c.data)
            .arg("-l")
            .arg(c.data.join("server.log"))
            .args(["-o", &options, "-w", "start"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(c)
    }

    fn command(&self, tool: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new(self.bin.join(tool));
        cmd.env("DYLD_LIBRARY_PATH", &self.lib)
            .env("LD_LIBRARY_PATH", &self.lib);
        cmd
    }

    async fn pool(&self) -> PgPool {
        let options = sqlx::postgres::PgConnectOptions::new()
            .host(self.socket.to_str().unwrap())
            .port(5432)
            .username("sandbox_admin")
            .password("fixture-admin-password")
            .database("postgres");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("ALTER ROLE sandbox_admin PASSWORD 'fixture-admin-password'")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn password_file(&self) {
        std::fs::write(
            self.data.join("pg_hba.conf"),
            "local all all scram-sha-256\n",
        )
        .unwrap();
    }

    fn reload(&self) {
        let out = self
            .command("pg_ctl")
            .arg("-D")
            .arg(&self.data)
            .arg("reload")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if self.data.join("postmaster.pid").exists() {
            // This fixture owns the cluster and its data directory.
            let _ = self
                .command("pg_ctl")
                .arg("-D")
                .arg(&self.data)
                .args(["-m", "immediate", "-w", "stop"])
                .output();
        }
    }
}

#[tokio::test]
async fn external_mixed_loaded_trust_is_refused_even_when_the_admin_probe_rejects() {
    let Some(cluster) = Cluster::start() else {
        return;
    };
    let pool = cluster.pool().await;
    sqlx::query("CREATE ROLE alternate_admin SUPERUSER LOGIN")
        .execute(&pool)
        .await
        .unwrap();
    std::fs::write(
        cluster.data.join("pg_hba.conf"),
        "local all sandbox_admin scram-sha-256\nlocal all alternate_admin trust\n",
    )
    .unwrap();
    cluster.reload();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !password_auth_is_effective(&cluster.url).await.unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cluster.password_file();
    assert!(local_auth_requires_password(&pool).await.unwrap());
    let mut alternate = lucidos_pg_url::PgUrl::parse(&cluster.url).unwrap();
    alternate.user = Some("alternate_admin".into());
    alternate.password = Some("wrong".into());
    sqlx::PgConnection::connect(&alternate.read_only_url())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let error = verify_database_socket(
        &pool,
        database_socket(&cluster.url).unwrap().unwrap(),
        &cluster.url,
        false,
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("gateway-managed"), "{error}");
}

#[tokio::test]
async fn disk_password_rules_do_not_authorize_a_loaded_trust_cluster() {
    let Some(cluster) = Cluster::start() else {
        return;
    };
    let pool = cluster.pool().await;
    let socket = database_socket(&cluster.url).unwrap().unwrap();
    assert!(!password_auth_is_effective(&cluster.url).await.unwrap());
    assert!(
        verify_database_socket(&pool, socket.clone(), &cluster.url, true)
            .await
            .is_err()
    );
    cluster.password_file();
    let mut wrong = lucidos_pg_url::PgUrl::parse(&cluster.url).unwrap();
    wrong.password = Some("deliberately-wrong-password".into());
    let conn = sqlx::PgConnection::connect(&wrong.read_only_url())
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(local_auth_requires_password(&pool).await.unwrap());
    assert!(!password_auth_is_effective(&cluster.url).await.unwrap());
    assert!(
        verify_database_socket(&pool, socket.clone(), &cluster.url, true)
            .await
            .is_err()
    );
    cluster.reload();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !password_auth_is_effective(&cluster.url).await.unwrap() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let verified = verify_database_socket(&pool, socket.clone(), &cluster.url, true)
        .await
        .unwrap();
    assert_eq!(verified.0, socket);

    // Malformed disk rules fail closed even while loaded rules remain safe.
    std::fs::write(
        cluster.data.join("pg_hba.conf"),
        "not a valid authentication rule\n",
    )
    .unwrap();
    assert!(
        verify_database_socket(&pool, socket.clone(), &cluster.url, true)
            .await
            .is_err()
    );
    pool.close().await;
    assert!(verify_database_socket(&pool, socket, &cluster.url, true)
        .await
        .is_err());
}
