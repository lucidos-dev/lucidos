use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

#[path = "vertex/adc.rs"]
pub mod adc;

pub type TokenCache = Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>;

/// INVARIANT: Get a cached Vertex access token, refreshing only when expired (50 min TTL).
/// Shared by VertexProvider and VertexImagenProvider, so every Vertex caller
/// benefits from the same auth.
///
/// On a cache miss the token is acquired **ADC-file-first, gcloud-fallback**:
/// 1. Refresh an `authorized_user` ADC file directly via OAuth. This path
///    works in a packaged build without a `gcloud` binary.
/// 2. If no usable file exists or refresh fails, fall back to
///    `gcloud auth application-default print-access-token`.
pub async fn get_cached_access_token(
    cache: &TokenCache,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    const TOKEN_TTL: Duration = Duration::from_secs(3000);

    {
        let guard = cache
            .lock()
            .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
        if let Some((ref token, ref fetched_at)) = *guard {
            if fetched_at.elapsed() < TOKEN_TTL {
                return Ok(token.clone());
            }
        }
    }

    let token = fetch_access_token().await?;
    let mut guard = cache
        .lock()
        .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
    *guard = Some((token.clone(), std::time::Instant::now()));
    Ok(token)
}

async fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(format!("{label} timed out after {}s", timeout.as_secs()).into()),
    }
}

async fn fetch_access_token() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(creds) = adc::load() {
        match adc::refresh_access_token(&creds).await {
            Ok(token) => return Ok(token),
            // INVARIANT: Don't fail outright — a stale/revoked ADC refresh should still try
            // the gcloud path (which may re-prompt / use a different config).
            Err(e) => crate::log_legacy!(
                "[Vertex] ADC token refresh failed, falling back to gcloud: {}",
                e
            ),
        }
    }

    let mut cmd = Command::new("gcloud");
    cmd.args(["auth", "application-default", "print-access-token"]);
    let output = run_with_timeout(cmd, adc::TOKEN_REQUEST_TIMEOUT, "gcloud").await?;

    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(format!(
            "Failed to get Vertex access token (no usable ADC file, and gcloud failed): {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_wedged_command_is_killed_at_the_timeout() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let err = run_with_timeout(cmd, Duration::from_millis(50), "test-command")
            .await
            .expect_err("a command that outlives its timeout must error");
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }

    #[tokio::test]
    async fn a_fast_command_completes_normally() {
        let cmd = Command::new("true");
        let output = run_with_timeout(cmd, Duration::from_secs(5), "test-command")
            .await
            .expect("a fast command must not be treated as wedged");
        assert!(output.status.success());
    }
}
