use crate::core::sanitize_for_jsonb;
use std::fs;
use std::path::PathBuf;

pub struct PythonRuntime {
    workspace_path: PathBuf,
    exhaust_path: PathBuf,
    venv_path: PathBuf,
    python_bin: PathBuf,
}

impl PythonRuntime {
    pub fn new(workspace_path: PathBuf) -> Self {
        // Canonicalize to absolute path to avoid issues when changing directories
        let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
        let exhaust_path = workspace_path.join(".lucidos").join("exhaust");
        if let Err(e) = fs::create_dir_all(&exhaust_path) {
            log!(
                "[Python] Failed to create exhaust dir at {}: {}",
                exhaust_path.display(),
                e
            );
        }

        // Clean up orphaned staging dirs from previous crashed runs
        let staging_root = workspace_path.join(".lucidos/staging");
        if staging_root.exists() {
            if let Err(e) = fs::remove_dir_all(&staging_root) {
                log!(
                    "[Python] Failed to clean up orphaned staging dir {}: {}",
                    staging_root.display(),
                    e
                );
            }
        }

        let venv_path = workspace_path.join(".lucidos/runtime/python/venv");
        let python_bin = venv_path.join("bin/python");

        Self {
            workspace_path,
            exhaust_path,
            venv_path,
            python_bin,
        }
    }

    /// Create the venv if it doesn't already exist. Called lazily on first execution.
    async fn ensure_venv(&self) -> Result<(), String> {
        if self.python_bin.exists() {
            return Ok(());
        }

        log!(
            "[Python] Creating virtual environment at {}",
            self.venv_path.display()
        );

        fs::create_dir_all(self.venv_path.parent().unwrap()).map_err(|e| e.to_string())?;

        let output = tokio::process::Command::new("python3")
            .args(["-m", "venv", self.venv_path.to_string_lossy().as_ref()])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("Failed to create venv: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to create venv:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        log!("[Python] Virtual environment created");
        Ok(())
    }

    /// Shared execution core: writes sandboxed code + user code to a script, runs it, returns output.
    async fn run_sandboxed(
        &self,
        preamble: &str,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.ensure_venv().await?;
        let task_id = uuid::Uuid::new_v4().to_string();
        let task_dir = self.exhaust_path.join(&task_id);
        fs::create_dir_all(&task_dir).map_err(|e| e.to_string())?;

        let full_code = format!("{}\n{}", preamble, code);
        let script_path = task_dir.join("script.py");
        fs::write(&script_path, &full_code).map_err(|e| e.to_string())?;

        let mut cmd = tokio::process::Command::new(&self.python_bin);
        cmd.arg(&script_path)
            .current_dir(&self.workspace_path)
            .stdin(std::process::Stdio::null());
        for (key, value) in &env_vars {
            cmd.env(key, value);
        }
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to execute Python: {}", e))?;

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        if let Err(e) = fs::write(task_dir.join("stdout.txt"), &stdout) {
            log!("[Python] Failed to write stdout debug log: {}", e);
        }
        if let Err(e) = fs::write(task_dir.join("stderr.txt"), &stderr) {
            log!("[Python] Failed to write stderr debug log: {}", e);
        }

        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!("Python error:\n{}", stderr))
        }
    }

    fn workspace_str_escaped(&self) -> String {
        self.workspace_path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
    }

    /// Build sandbox preamble that blocks writes outside the workspace.
    fn write_guard_preamble(&self) -> String {
        let workspace = self.workspace_str_escaped();
        format!(
            "import builtins as _builtins, os as _os, tempfile as _tempfile\n\
             _original_open = _builtins.open\n\
             def _sandboxed_open(file, mode='r', *args, _allowed=(\n\
             \x20   _os.path.realpath('{workspace}'),\n\
             \x20   _os.path.realpath(_tempfile.gettempdir()),\n\
             ), _orig=_builtins.open, **kwargs):\n\
             \x20   if isinstance(file, (str, bytes, _os.PathLike)) and any(c in str(mode) for c in 'wxa+'):\n\
             \x20       real = _os.path.realpath(str(file))\n\
             \x20       if not any(real.startswith(p) for p in _allowed):\n\
             \x20           raise PermissionError('Sandbox: cannot write outside workspace: ' + str(file) + '. Use the run_claude tool to edit source code.')\n\
             \x20   return _orig(file, mode, *args, **kwargs)\n\
             _builtins.open = _sandboxed_open\n\
             del _sandboxed_open, _original_open\n",
            workspace = workspace,
        )
    }

    /// Build sandbox preamble that redirects data/ writes to staging and provides read-through.
    fn staging_preamble(&self, staging_dir: &std::path::Path) -> String {
        let workspace = self.workspace_str_escaped();
        let staging = staging_dir
            .canonicalize()
            .unwrap_or(staging_dir.to_path_buf())
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        format!(
            "import builtins as _builtins, os as _os, tempfile as _tempfile\n\
             _workspace = _os.path.realpath('{workspace}')\n\
             _staging = _os.path.realpath('{staging}')\n\
             _data_dir = _os.path.join(_workspace, 'data')\n\
             _original_open = _builtins.open\n\
             def _sandboxed_open(file, mode='r', *args, _orig=_builtins.open, **kwargs):\n\
             \x20   if not isinstance(file, (str, bytes, _os.PathLike)):\n\
             \x20       return _orig(file, mode, *args, **kwargs)\n\
             \x20   real = _os.path.realpath(str(file))\n\
             \x20   is_write = any(c in str(mode) for c in 'wxa+')\n\
             \x20   if is_write:\n\
             \x20       allowed = (_workspace, _os.path.realpath(_tempfile.gettempdir()))\n\
             \x20       if not any(real.startswith(p) for p in allowed):\n\
             \x20           raise PermissionError('Sandbox: cannot write outside workspace: ' + str(file))\n\
             \x20       if real.startswith(_data_dir):\n\
             \x20           rel = _os.path.relpath(real, _workspace)\n\
             \x20           staged = _os.path.join(_staging, rel)\n\
             \x20           _os.makedirs(_os.path.dirname(staged), exist_ok=True)\n\
             \x20           return _orig(staged, mode, *args, **kwargs)\n\
             \x20   elif real.startswith(_data_dir):\n\
             \x20       rel = _os.path.relpath(real, _workspace)\n\
             \x20       staged = _os.path.join(_staging, rel)\n\
             \x20       if _os.path.exists(staged):\n\
             \x20           return _orig(staged, mode, *args, **kwargs)\n\
             \x20   return _orig(file, mode, *args, **kwargs)\n\
             _builtins.open = _sandboxed_open\n\
             del _sandboxed_open, _original_open\n",
            workspace = workspace,
            staging = staging,
        )
    }

    pub async fn execute_with_env(
        &self,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        let preamble = self.write_guard_preamble();
        self.run_sandboxed(&preamble, code, env_vars).await
    }

    /// Execute Python with staging: writes under data/ are redirected to the staging directory.
    /// Reads check staging first (for files written this run), then fall through to workspace.
    pub async fn execute_staged(
        &self,
        code: &str,
        env_vars: Vec<(String, String)>,
        staging_dir: &std::path::Path,
    ) -> Result<String, String> {
        fs::create_dir_all(staging_dir).map_err(|e| e.to_string())?;
        let preamble = self.staging_preamble(staging_dir);
        self.run_sandboxed(&preamble, code, env_vars).await
    }

    pub async fn execute(&self, code: &str) -> Result<String, String> {
        self.execute_with_env(code, vec![]).await
    }

    /// Install multiple packages in a single pip invocation.
    pub async fn ensure_packages(&self, packages: &[&str]) -> Result<(), String> {
        if packages.is_empty() {
            return Ok(());
        }
        self.ensure_venv().await?;

        let mut args = vec!["-m", "pip", "install", "--quiet"];
        args.extend(packages);

        let output = tokio::process::Command::new(&self.python_bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("Failed to install packages: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Failed to install packages:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_execute_simple_python() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf());

        let result = runtime.execute("print('hello')").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_python_error() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf());

        let result = runtime.execute("raise ValueError('test error')").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ValueError"));
    }

    #[tokio::test]
    async fn test_sandbox_blocks_write_outside_workspace() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf());

        // /tmp is allowed (tempdir prefix), so try a path that's clearly outside
        let result = runtime
            .execute("open('/etc/lucidos_sandbox_test', 'w').write('bad')")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Sandbox"));
    }

    #[tokio::test]
    async fn test_sandbox_allows_write_inside_workspace() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf());

        let result = runtime
            .execute("open('test_file.txt', 'w').write('ok'); print('done')")
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "done");
        assert!(dir.path().join("test_file.txt").exists());
    }

    #[tokio::test]
    async fn test_sandbox_allows_read_anywhere() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf());

        // Reading outside workspace should be fine
        let result = runtime
            .execute("import os; print(os.path.exists('/etc'))")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_staging_redirects_data_writes() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf());

        let staging = ws.join(".lucidos/staging/test-run");

        let result = runtime
            .execute_staged(
                "open('data/artifacts/report.csv', 'w').write('a,b\\n1,2'); print('done')",
                vec![],
                &staging,
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "done");

        // File should be in staging, NOT in real workspace
        assert!(staging.join("data/artifacts/report.csv").exists());
        assert!(!ws.join("data/artifacts/report.csv").exists());
    }

    #[tokio::test]
    async fn test_staging_reads_fall_through_to_workspace() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/existing.txt"), "original").unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf());

        let staging = ws.join(".lucidos/staging/test-run-2");

        let result = runtime
            .execute_staged(
                "content = open('data/artifacts/existing.txt').read(); print(content)",
                vec![],
                &staging,
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "original");
    }

    #[tokio::test]
    async fn test_staging_reads_own_writes() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf());

        let staging = ws.join(".lucidos/staging/test-run-3");

        let result = runtime.execute_staged(
            "open('data/output.txt', 'w').write('hello')\ncontent = open('data/output.txt').read()\nprint(content)",
            vec![],
            &staging,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "hello");
    }

    #[tokio::test]
    async fn test_staging_non_data_writes_go_to_workspace() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let runtime = PythonRuntime::new(ws.to_path_buf());

        let staging = ws.join(".lucidos/staging/test-run-4");

        let result = runtime
            .execute_staged(
                "open('scratch.txt', 'w').write('temp'); print('ok')",
                vec![],
                &staging,
            )
            .await;

        assert!(result.is_ok());
        // Non-data/ writes go directly to workspace
        assert!(ws.join("scratch.txt").exists());
    }
}
