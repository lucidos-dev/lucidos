use super::super::LucidosEngine;
use crate::core::sanitize_for_jsonb;

const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;

impl LucidosEngine {
    pub(crate) async fn execute_bash_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => return Ok("Error: command is required".to_string()),
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        let env_vars = self.build_script_env_vars().await;

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", command])
            .current_dir(self.workspace_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in &env_vars {
            cmd.env(key, value);
        }

        log!(
            "[Bash] Running: {}",
            &command[..command.floor_char_boundary(200)]
        );

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Ok(format!("Error executing command: {}", e)),
            Err(_) => {
                // Timeout — kill the process. child.kill() requires &mut self,
                // but wait_with_output() consumed child. Since the timeout
                // means the future was dropped (child process handle dropped),
                // the OS will clean up the child. We just report the timeout.
                return Ok(format!("Error: command timed out after {}s", timeout_secs));
            }
        };

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));
        let exit_code = output.status.code().unwrap_or(-1);

        let mut response = String::new();

        if !stdout.is_empty() {
            response.push_str(&truncate_output(&stdout, MAX_OUTPUT_BYTES));
        }

        if !stderr.is_empty() {
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&format!(
                "[stderr]\n{}",
                truncate_output(&stderr, MAX_OUTPUT_BYTES)
            ));
        }

        if !output.status.success() {
            if !response.is_empty() {
                response.push_str("\n\n");
            }
            response.push_str(&format!("[exit code: {}]", exit_code));
        }

        if response.is_empty() {
            response = format!("[exit code: {}]", exit_code);
        }

        Ok(response)
    }
}

fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max);
        format!("{}...\n[truncated — {} bytes total]", &s[..end], s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_output_short_string() {
        let s = "hello world";
        assert_eq!(truncate_output(s, 100), "hello world");
    }

    #[test]
    fn truncate_output_long_string() {
        let s = "a".repeat(200);
        let result = truncate_output(&s, 50);
        assert!(result.starts_with(&"a".repeat(50)));
        assert!(result.contains("[truncated"));
        assert!(result.contains("200 bytes total"));
    }

    #[test]
    fn truncate_output_multibyte_boundary() {
        let s = "ééééé"; // 10 bytes in UTF-8
        let result = truncate_output(s, 5);
        assert!(result.contains("[truncated"));
    }
}
