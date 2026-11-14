//! The `op` command line tool as a [`SecretResolver`].
//!
//! 1Password service accounts have no REST API, so the engine runs
//! `op read --no-newline <reference>`. The child gets a cleared environment
//! holding only `OP_SERVICE_ACCOUNT_TOKEN`, `PATH` and `HOME`, and a timeout.
//! The token reaches no other process.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{ResolveFailure, SecretReference, SecretResolver};
use crate::core::CredentialStore;

/// How long one `op` run may take before it counts as unreachable.
pub const OP_TIMEOUT: Duration = Duration::from_secs(20);

/// The most of `op`'s stderr a failure reason quotes.
const REASON_MAX_CHARS: usize = 300;

/// What a `onepassword_service_account` credential holds, sealed: the token,
/// and optionally where `op` lives. The path sits here rather than in a
/// preference because the agent role may write preferences, and whoever
/// picks the binary receives the token.
#[derive(Deserialize, Serialize)]
pub struct ServiceAccount {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_path: Option<String>,
}

impl ServiceAccount {
    /// Read the stored payload: a JSON object, or the bare token.
    pub fn parse(stored: &str) -> Self {
        serde_json::from_str::<Self>(stored).unwrap_or_else(|_| Self {
            token: stored.trim().to_string(),
            cli_path: None,
        })
    }

    /// The `op` binary to run: the stored path, else the first `op` on the
    /// engine's `PATH`, which already holds the usual install folders
    /// (`core::user_path`).
    fn cli(&self) -> Result<PathBuf, ResolveFailure> {
        if let Some(path) = self.cli_path.as_deref().map(str::trim) {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        std::env::var_os("PATH")
            .and_then(|path| find_on_path("op", &path))
            .ok_or_else(|| {
                ResolveFailure::Transport(
                    "the 1Password command line tool (op) is not installed, \
                     or not where Lucidos looks. Set its path in Settings, 1Password"
                        .to_string(),
                )
            })
    }
}

/// The first executable `name` in `path`.
fn find_on_path(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Resolves through `op` with the workspace's service account.
pub struct OpResolver {
    pool: PgPool,
    timeout: Duration,
}

impl OpResolver {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            timeout: OP_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn with_timeout(pool: PgPool, timeout: Duration) -> Self {
        Self { pool, timeout }
    }
}

#[async_trait]
impl SecretResolver for OpResolver {
    async fn resolve(&self, reference: &SecretReference) -> Result<String, ResolveFailure> {
        let account = CredentialStore::onepassword_service_account(&self.pool)
            .await
            .map_err(|e| ResolveFailure::Transport(format!("the database could not be read: {e}")))?
            .ok_or_else(|| {
                ResolveFailure::Definitive(
                    "no 1Password service account is set up. Add its token in Settings, 1Password"
                        .to_string(),
                )
            })?;
        run_op_read(&account, reference, self.timeout).await
    }

    async fn record_outcome(&self, service_name: &str, failure: Option<&str>) {
        super::notify_outage(service_name, failure).await;
    }
}

/// Run `op read` once. Only the token, `PATH` and `HOME` cross into the child.
pub async fn run_op_read(
    account: &ServiceAccount,
    reference: &SecretReference,
    timeout: Duration,
) -> Result<String, ResolveFailure> {
    let stdout = run_op(
        account,
        &["read", "--no-newline", reference.as_str()],
        timeout,
    )
    .await?;
    String::from_utf8(stdout)
        .map_err(|_| ResolveFailure::Definitive(format!("{} is not text", reference.as_str())))
}

/// What a working service account reaches: the `op` that ran, and the names
/// of the vaults it may read.
#[derive(Serialize)]
pub struct VaultCheck {
    pub cli: String,
    pub vaults: Vec<String>,
}

/// Ask 1Password which vaults `account` may read, which proves the token and
/// the `op` path work together.
pub async fn list_vaults(
    account: &ServiceAccount,
    timeout: Duration,
) -> Result<VaultCheck, ResolveFailure> {
    #[derive(Deserialize)]
    struct Vault {
        name: String,
    }
    let cli = account.cli()?.display().to_string();
    let stdout = run_op(account, &["vault", "list", "--format", "json"], timeout).await?;
    let vaults: Vec<Vault> = serde_json::from_slice(&stdout).map_err(|e| {
        ResolveFailure::Transport(format!(
            "op listed vaults in a shape Lucidos cannot read: {e}"
        ))
    })?;
    Ok(VaultCheck {
        cli,
        vaults: vaults.into_iter().map(|vault| vault.name).collect(),
    })
}

/// Run `op` once with `args`, returning its stdout. The child gets a cleared
/// environment holding only the token, `PATH` and `HOME`.
async fn run_op(
    account: &ServiceAccount,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, ResolveFailure> {
    let cli = account.cli()?;
    let mut command = tokio::process::Command::new(&cli);
    command
        .args(args)
        .env_clear()
        .env("OP_SERVICE_ACCOUNT_TOKEN", &account.token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in ["PATH", "HOME"] {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Err(_) => {
            return Err(ResolveFailure::Transport(format!(
                "1Password did not answer within {} seconds",
                timeout.as_secs()
            )))
        }
        Ok(Err(e)) => {
            return Err(ResolveFailure::Transport(format!(
                "{} could not run: {e}",
                cli.display()
            )))
        }
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let masked = crate::core::subprocess_secrets::redact_subprocess_output(
        stderr.trim(),
        std::slice::from_ref(&account.token),
    );
    let reason: String = masked.chars().take(REASON_MAX_CHARS).collect();
    Err(classify_op_failure(&format!("op said: {reason}")))
}

/// Sort an `op` error into a definitive answer or a transport failure.
///
/// Only an answer about the item, the vault or access is definitive. Anything
/// else, such as a dropped connection, may pass, so the last good value
/// keeps serving.
fn classify_op_failure(reason: &str) -> ResolveFailure {
    const DEFINITIVE: &[&str] = &[
        "isn't an item",
        "isn't a vault",
        "isn't a field",
        "could not find",
        "cannot be found",
        "can't be found",
        "not found",
        "does not exist",
        "doesn't exist",
        "no item",
        "forbidden",
        "unauthorized",
        "access denied",
        "permission",
        "invalid",
        "authentication",
    ];
    let lowered = reason.to_lowercase();
    if DEFINITIVE.iter().any(|marker| lowered.contains(marker)) {
        ResolveFailure::Definitive(reason.to_string())
    } else {
        ResolveFailure::Transport(reason.to_string())
    }
}

#[cfg(test)]
#[path = "op_tests.rs"]
pub(crate) mod tests;
