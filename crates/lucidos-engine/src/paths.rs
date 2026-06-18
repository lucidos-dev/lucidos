//! Runtime filesystem path helpers.
//!
//! Resolves the repo root from `current_exe()` instead of compile-time
//! `CARGO_MANIFEST_DIR`, so a project rename after build doesn't strand
//! script-spawns and asset reads at a path that no longer exists.

use std::path::PathBuf;
use std::sync::OnceLock;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const REPO_MARKER: &str = "scripts/web-dev.sh";

/// Resolve the Lucidos repo root from the running binary's path.
/// Cached for the process lifetime; called from polled handlers (`health`).
pub fn repo_root() -> Result<PathBuf, BoxError> {
    static CACHED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CACHED
        .get_or_init(compute_repo_root)
        .clone()
        .map_err(|e| e.into())
}

fn compute_repo_root() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot resolve current executable path: {e}"))?;

    for ancestor in exe.ancestors() {
        if ancestor.join(REPO_MARKER).exists() {
            return Ok(ancestor.to_path_buf());
        }
    }

    Err(format!(
        "No Lucidos repo root above current executable ({}); \
         expected an ancestor containing {REPO_MARKER}",
        exe.display()
    ))
}

/// Best-effort repo root: falls back to compile-time `CARGO_MANIFEST_DIR`
/// when the binary lives outside a Lucidos tree (engine startup, recovery).
pub fn repo_root_or_compile_time_fallback() -> PathBuf {
    repo_root().unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    })
}

/// Path to a script under `<repo_root>/scripts/<name>`. Errors with the
/// resolved path when the script is missing, so spawn failures surface a
/// useful message rather than a bare `No such file or directory`.
pub fn script(name: &str) -> Result<PathBuf, BoxError> {
    let path = repo_root()?.join("scripts").join(name);
    if !path.exists() {
        return Err(format!("Script not found: {}", path.display()).into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_finds_scripts_marker_from_test_binary() {
        let root = repo_root().expect("repo_root must resolve from a cargo-run test binary");
        assert!(
            root.join(REPO_MARKER).exists(),
            "resolved repo root {} does not contain {REPO_MARKER}",
            root.display()
        );
    }

    #[test]
    fn script_returns_existing_path() {
        let path = script("web-dev.sh").expect("web-dev.sh must exist in dev tree");
        assert!(path.ends_with("scripts/web-dev.sh"));
        assert!(path.exists());
    }

    #[test]
    fn script_errors_when_missing() {
        let err = script("definitely-not-a-real-script.sh").expect_err("missing script must err");
        let msg = err.to_string();
        assert!(
            msg.contains("definitely-not-a-real-script.sh"),
            "error must name the missing path; got: {msg}"
        );
    }

    /// Bind-mounting PGDATA on macOS pushes every WAL fsync through Docker
    /// Desktop's virtiofs, which throttles host writes and crashes the VM
    /// under sustained e2e load (34 GB dirtied in 4 hours, see thread
    /// 50724c03 — `com.apple.Virtualization.VirtualMachine` diagnostic at
    /// 22:44, DB pool timeouts at 05:50, force restart at 07:18). Switching
    /// to a Docker named volume keeps fsyncs inside the VM's ext4.
    ///
    /// PG 18's image relocated PGDATA to /var/lib/postgresql/18/docker and
    /// declares its VOLUME at the parent /var/lib/postgresql (PG 17 used
    /// /var/lib/postgresql/data for both), so the named volume must be mounted
    /// at the parent — mounting the old /data path would strand PGDATA on an
    /// anonymous volume that is lost on each container recreate.
    #[test]
    fn dev_postgres_uses_named_volume_not_bind_mount() {
        let repo = repo_root().expect("repo root");
        let yml = std::fs::read_to_string(repo.join("docker-compose.dev.yml"))
            .expect("docker-compose.dev.yml must exist");

        assert!(
            yml.contains("- lucidos-pg-data:/var/lib/postgresql\n"),
            "Postgres PGDATA must be a named volume mounted at the parent\n\
             /var/lib/postgresql (PG 18 keeps the cluster under it at 18/docker).\n\
             Expected line: '- lucidos-pg-data:/var/lib/postgresql'\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
        assert!(
            !yml.contains("- lucidos-pg-data:/var/lib/postgresql/data"),
            "PG 17 mount path must be gone — PG 18 relocated PGDATA, so mounting\n\
             the named volume at /var/lib/postgresql/data loses data on recreate.\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
        assert!(
            !yml.contains("/data/postgres:/var/lib/postgresql/data"),
            "Legacy host bind-mount entry must be removed.\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
    }
}
