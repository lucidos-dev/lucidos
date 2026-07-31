//! Spawns `script_handshake` auth scripts and parses their JSON output.
//!
//! Contract:
//! - Script reads creds from env vars (`CRED_<NAME>_USERNAME` /
//!   `CRED_<NAME>_PASSWORD` for password creds; `CRED_<NAME>` for others).
//! - Script prints exactly one JSON object to stdout:
//!   `{"headers": {"<name>": "<value>", ...}, "expires_in": <seconds>}`.
//! - Non-zero exit → stderr is captured into the engine's 502 response.
//! - 30-second timeout. Exceeded → 502.

use axum::http::{HeaderName, HeaderValue};
use std::path::Path as FsPath;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct HandshakeOutput {
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub expires_in: u64,
}

#[derive(Debug)]
pub enum RunError {
    NotFound(String),
    Timeout,
    NonZeroExit { code: Option<i32>, stderr: String },
    InvalidOutput(String),
    Spawn(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotFound(p) => write!(f, "auth handshake script not found: {}", p),
            RunError::Timeout => write!(f, "auth handshake script timed out after 30s"),
            RunError::NonZeroExit { code, stderr } => write!(
                f,
                "auth handshake script exited {:?}: {}",
                code,
                stderr.trim()
            ),
            RunError::InvalidOutput(msg) => {
                write!(f, "auth handshake script output invalid: {}", msg)
            }
            RunError::Spawn(e) => write!(f, "failed to spawn auth handshake script: {}", e),
        }
    }
}

pub async fn run_handshake_script(
    workspace_path: &FsPath,
    script_rel_path: &str,
    env_vars: Vec<(String, String)>,
) -> Result<HandshakeOutput, RunError> {
    // The script path comes from workspace config (`apis.json`). Reject any
    // value that would escape the workspace tree (`..`, absolute path) before
    // joining — otherwise a crafted config could run an arbitrary host script.
    if crate::api::is_path_traversal(script_rel_path) {
        return Err(RunError::NotFound(script_rel_path.to_string()));
    }
    // Resolve the script relative to `data/` first — that's where all
    // user-authored, git-tracked workspace content lives, and what the
    // auth-handshake docs document (a file at `data/scripts/auth/<foo>.py`
    // referenced in apis.json as `"script": "scripts/auth/<foo>.py"`). Fall
    // back to the workspace root for back-compat with scripts placed there
    // before this fix (temporary measure — see docs/temporary-measures.md,
    // "script_handshake workspace-root script fallback"). The traversal
    // guard above already ran on the raw relative value, so both joins stay
    // inside the workspace tree.
    let data_abs = workspace_path.join("data").join(script_rel_path);
    let script_abs = if data_abs.exists() {
        data_abs
    } else {
        let root_abs = workspace_path.join(script_rel_path);
        if root_abs.exists() {
            root_abs
        } else {
            // Neither location has it. Report the documented `data/`-relative
            // path so the operator knows where to put the script — a clean
            // error before we pay to spawn python3 only to have it print a
            // confusing "can't open file" (common cause: a typo in apis.json).
            return Err(RunError::NotFound(data_abs.display().to_string()));
        }
    };
    // Collect the secret env values (CRED_*/OAUTH_*) so any that the script
    // echoes to stderr (a traceback dumping os.environ, a debug print) is
    // scrubbed out of the error we surface to the caller / logs — a script's
    // stderr is returned verbatim in the 502, and these env vars carry live
    // credentials and OAuth tokens.
    let secret_values: Vec<String> = env_vars
        .iter()
        .filter(|(k, _)| k.starts_with("CRED_") || k.starts_with("OAUTH_"))
        .map(|(_, v)| v.clone())
        .collect();

    let mut cmd = Command::new("python3");
    cmd.arg(&script_abs);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match timeout(SCRIPT_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(RunError::Spawn(e.to_string())),
        Err(_) => return Err(RunError::Timeout),
    };
    if !output.status.success() {
        let stderr = crate::core::redact_secret_values(
            &String::from_utf8_lossy(&output.stderr),
            &secret_values,
        );
        return Err(RunError::NonZeroExit {
            code: output.status.code(),
            stderr,
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| RunError::InvalidOutput(format!("not JSON: {}", e)))?;
    let headers_obj = parsed
        .get("headers")
        .and_then(|v| v.as_object())
        .ok_or_else(|| RunError::InvalidOutput("missing 'headers' object".to_string()))?;
    let mut headers = Vec::with_capacity(headers_obj.len());
    for (k, v) in headers_obj {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| RunError::InvalidOutput(format!("bad header name '{}': {}", k, e)))?;
        let val_str = v
            .as_str()
            .ok_or_else(|| RunError::InvalidOutput(format!("header '{}' value not a string", k)))?;
        let val = HeaderValue::from_str(val_str)
            .map_err(|e| RunError::InvalidOutput(format!("bad header value for '{}': {}", k, e)))?;
        headers.push((name, val));
    }
    let expires_in = parsed
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            RunError::InvalidOutput("missing or non-integer 'expires_in'".to_string())
        })?;
    Ok(HandshakeOutput {
        headers,
        expires_in,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_script(tmp: &FsPath, name: &str, body: &str) -> String {
        let dir = tmp.join("scripts/auth");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
        format!("scripts/auth/{}", name)
    }

    /// Writes a script at the git-tracked `data/scripts/auth/` location and
    /// returns the same `scripts/auth/<name>` relative value the config uses
    /// (the engine prepends `data/` when resolving).
    fn write_data_script(tmp: &FsPath, name: &str, body: &str) -> String {
        let dir = tmp.join("data/scripts/auth");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
        format!("scripts/auth/{}", name)
    }

    #[tokio::test]
    async fn happy_path_returns_headers_and_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "happy.py",
            r#"
import json
print(json.dumps({"headers": {"Authorization": "Bearer abc", "X-Client-Id": "xyz"}, "expires_in": 3600}))
"#,
        );
        let out = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .unwrap();
        assert_eq!(out.expires_in, 3600);
        assert_eq!(out.headers.len(), 2);
        let names: Vec<&str> = out.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"authorization"));
        assert!(names.contains(&"x-client-id"));
    }

    #[tokio::test]
    async fn data_relative_script_is_found() {
        // The documented + git-tracked location: file at
        // `data/scripts/auth/<foo>.py`, config value `scripts/auth/<foo>.py`.
        // Before the fix this resolved to the workspace root and 404'd.
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_data_script(
            tmp.path(),
            "data-loc.py",
            r#"import json; print(json.dumps({"headers": {"X-Where": "data"}, "expires_in": 60}))"#,
        );
        let out = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .unwrap();
        let names: Vec<&str> = out.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"x-where"),
            "data/-relative script should run"
        );
    }

    #[tokio::test]
    async fn data_relative_preferred_over_workspace_root() {
        // When the same relative path exists in BOTH `data/` and the
        // workspace root, `data/` (git-tracked) wins.
        let tmp = tempfile::tempdir().unwrap();
        let rel = "scripts/auth/both.py";
        write_data_script(
            tmp.path(),
            "both.py",
            r#"import json; print(json.dumps({"headers": {"X-Src": "data"}, "expires_in": 60}))"#,
        );
        write_script(
            tmp.path(),
            "both.py",
            r#"import json; print(json.dumps({"headers": {"X-Src": "root"}, "expires_in": 60}))"#,
        );
        let out = run_handshake_script(tmp.path(), rel, vec![]).await.unwrap();
        let src = out
            .headers
            .iter()
            .find(|(n, _)| n.as_str() == "x-src")
            .map(|(_, v)| v.to_str().unwrap());
        assert_eq!(src, Some("data"), "data/ location must take precedence");
    }

    #[tokio::test]
    async fn missing_script_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_handshake_script(tmp.path(), "scripts/auth/nope.py", vec![])
            .await
            .expect_err("expected NotFound");
        match err {
            RunError::NotFound(_) => {}
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn traversal_script_path_rejected_before_spawn() {
        // The script path comes from workspace config (`apis.json`), so a
        // crafted `..` escape must be rejected by the `is_path_traversal` guard
        // before we ever join it onto the workspace and spawn python3. The
        // traversal branch returns the *original* relative path, whereas the
        // on-disk-not-found branch returns the joined absolute path — asserting
        // on the relative value proves the guard fired (and is still wired in),
        // not merely that the file happens to be absent.
        let tmp = tempfile::tempdir().unwrap();
        let err = run_handshake_script(tmp.path(), "../../etc/evil.py", vec![])
            .await
            .expect_err("traversal path must be rejected");
        match err {
            RunError::NotFound(p) => assert_eq!(p, "../../etc/evil.py"),
            other => panic!("expected NotFound for traversal, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_zero_exit_returns_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "fail.py",
            r#"
import sys
print("login failed: bad creds", file=sys.stderr)
sys.exit(2)
"#,
        );
        let err = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .expect_err("expected non-zero exit");
        match err {
            RunError::NonZeroExit { code, stderr } => {
                assert_eq!(code, Some(2));
                assert!(
                    stderr.contains("login failed: bad creds"),
                    "got: {:?}",
                    stderr
                );
            }
            other => panic!("expected NonZeroExit, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_zero_exit_stderr_redacts_credential_env_values() {
        // A failing script that leaks a CRED_/OAUTH_ env value to stderr (a
        // traceback dumping os.environ, an over-eager debug print) must not have
        // that secret echoed back in the surfaced error — stderr is returned
        // verbatim into the 502 / logs.
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "leaky.py",
            r#"
import os, sys
print("login failed; token was " + os.environ.get("CRED_SVC", ""), file=sys.stderr)
print("oauth=" + os.environ.get("OAUTH_GOOGLE_ACCESS_TOKEN", ""), file=sys.stderr)
sys.exit(3)
"#,
        );
        let err = run_handshake_script(
            tmp.path(),
            &rel,
            vec![
                (
                    "CRED_SVC".to_string(),
                    "sk-live-supersecret-value".to_string(),
                ),
                (
                    "OAUTH_GOOGLE_ACCESS_TOKEN".to_string(),
                    "ya29.oauth-access-token".to_string(),
                ),
            ],
        )
        .await
        .expect_err("expected non-zero exit");
        match err {
            RunError::NonZeroExit { code, stderr } => {
                assert_eq!(code, Some(3));
                assert!(
                    !stderr.contains("sk-live-supersecret-value"),
                    "CRED_ value must be redacted from stderr; got: {stderr:?}"
                );
                assert!(
                    !stderr.contains("ya29.oauth-access-token"),
                    "OAUTH_ value must be redacted from stderr; got: {stderr:?}"
                );
                assert!(
                    stderr.contains("[REDACTED]"),
                    "redaction marker should be present; got: {stderr:?}"
                );
                // Non-secret context is preserved so the operator still sees the failure.
                assert!(stderr.contains("login failed"), "got: {stderr:?}");
            }
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_json_output_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(tmp.path(), "badjson.py", r#"print("not json")"#);
        let err = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .expect_err("expected invalid output");
        match err {
            RunError::InvalidOutput(msg) => assert!(msg.contains("not JSON")),
            other => panic!("expected InvalidOutput, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn missing_headers_field_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "noheaders.py",
            r#"import json; print(json.dumps({"expires_in": 60}))"#,
        );
        let err = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .expect_err("expected missing headers");
        match err {
            RunError::InvalidOutput(msg) => assert!(msg.contains("headers")),
            other => panic!("expected InvalidOutput, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn missing_expires_in_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "noexpiry.py",
            r#"import json; print(json.dumps({"headers": {"X": "y"}}))"#,
        );
        let err = run_handshake_script(tmp.path(), &rel, vec![])
            .await
            .expect_err("expected missing expires_in");
        match err {
            RunError::InvalidOutput(msg) => assert!(msg.contains("expires_in")),
            other => panic!("expected InvalidOutput, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn oauth_env_vars_reach_the_script() {
        // Pins the `OAUTH_<P>_ACCESS_TOKEN` / `_EMAIL` name format that the
        // `script_handshake` layer's `oauth_providers` config produces, so
        // a prefix-rename breaks here instead of silently leaving scripts
        // reading a missing env var.
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "oauth.py",
            r#"
import os, json
tok = os.environ.get("OAUTH_GOOGLE_ACCESS_TOKEN", "")
email = os.environ.get("OAUTH_GOOGLE_EMAIL", "")
print(json.dumps({"headers": {"X-Token": tok, "X-Email": email}, "expires_in": 60}))
"#,
        );
        let out = run_handshake_script(
            tmp.path(),
            &rel,
            vec![
                (
                    "OAUTH_GOOGLE_ACCESS_TOKEN".to_string(),
                    "ya29.test-token".to_string(),
                ),
                (
                    "OAUTH_GOOGLE_EMAIL".to_string(),
                    "user@gmail.com".to_string(),
                ),
            ],
        )
        .await
        .unwrap();
        let map: std::collections::HashMap<&str, &str> = out
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.to_str().unwrap()))
            .collect();
        assert_eq!(map.get("x-token").copied(), Some("ya29.test-token"));
        assert_eq!(map.get("x-email").copied(), Some("user@gmail.com"));
    }

    #[tokio::test]
    async fn env_vars_reach_the_script() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = write_script(
            tmp.path(),
            "env.py",
            r#"
import os, json
u = os.environ.get("CRED_TEST_USERNAME", "")
p = os.environ.get("CRED_TEST_PASSWORD", "")
print(json.dumps({"headers": {"X-User": u, "X-Pass": p}, "expires_in": 60}))
"#,
        );
        let out = run_handshake_script(
            tmp.path(),
            &rel,
            vec![
                ("CRED_TEST_USERNAME".to_string(), "alice".to_string()),
                ("CRED_TEST_PASSWORD".to_string(), "s3cret".to_string()),
            ],
        )
        .await
        .unwrap();
        let map: std::collections::HashMap<&str, &str> = out
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.to_str().unwrap()))
            .collect();
        assert_eq!(map.get("x-user").copied(), Some("alice"));
        assert_eq!(map.get("x-pass").copied(), Some("s3cret"));
    }
}
