use crate::core::sanitize_for_jsonb;
use std::fs;
use std::path::PathBuf;

/// Python bootstrap module dropped into the venv's `site-packages/` as
/// `_lucidos_agent_origin.py` by `PythonRuntime::install_agent_origin_shim`,
/// loaded via a sibling `_lucidos_agent_origin.pth` whose single line is
/// `import _lucidos_agent_origin`. The `.pth` `import` line runs during
/// `site` startup (before any user code in `run_python` /
/// `run_python_background`).
///
/// Why a `.pth` import and NOT `sitecustomize.py`: CPython imports only the
/// FIRST module named `sitecustomize` on `sys.path`, and Homebrew's
/// `python@3.x` ships its own `sitecustomize.py` in the stdlib dir (it
/// shuffles `sys.path`), which sits *earlier* than the venv's site-packages.
/// A venv `sitecustomize.py` is therefore silently shadowed on every Homebrew
/// Python — so the token never got forwarded and `urllib` calls landed as
/// `Api{Human}` ("You") instead of `Lucidos Agent`. `.pth` files are
/// different: `site` execs the `import` line of EVERY `.pth` in each site dir,
/// so a uniquely-named module can never be shadowed by a competing
/// `sitecustomize`. (Verified against Homebrew Python 3.14.)
///
/// What it does: monkey-patches `http.client.HTTPConnection.request` to
/// attach the engine's thread-bound origin token
/// (`x-lucidos-agent-origin-token`) on outbound HTTPS/HTTP requests that
/// target `localhost:LUCIDOS_API_PORT`. That one header carries both facts
/// the engine needs, because the token names the spawning thread. Since
/// `urllib`, `requests`, and `urllib3` all route through `http.client`
/// underneath, a single patch covers every common HTTP client library.
///
/// Why monkey-patch and not a Session/opener: the LLM-written agent script
/// in the original incident did `urllib.request.urlopen(req, ...)` directly,
/// not via a Lucidos-provided helper. We patch the layer they cannot avoid.
///
/// Gate is strict: token AND port both required, host must be one of
/// `localhost`/`127.0.0.1`/`::1`, port must match `LUCIDOS_API_PORT`. Pip
/// installs to PyPI (different host), any localhost service on a different
/// port, and Tauri installs (no `LUCIDOS_API_PORT`) are all untouched.
const AGENT_ORIGIN_SHIM_PY: &str = r#"# Lucidos venv bootstrap — auto-attribution for engine HTTP callbacks.
#
# Loaded at interpreter startup via a sibling _lucidos_agent_origin.pth
# (`import _lucidos_agent_origin`) — NOT as sitecustomize.py, which Homebrew
# Python shadows with its own. Patches http.client so that any
# urllib / requests / urllib3 / http.client call from a Lucidos-spawned
# subprocess (run_python, run_python_background, scheduled script, ...) to
# the engine's API port automatically carries the thread-bound agent-origin
# token. That one token is the whole contract: the engine reads the spawning
# thread off the token itself, so nothing here has to name it. Without the
# shim, raw urllib.request.urlopen("https://
# localhost:PORT/api/v1/changes/<id>/apply") lands as Api{Human} in the
# events table and the timeline renders the action as "You" instead of
# "Lucidos Agent".
#
# Inert (no patch installed) if LUCIDOS_AGENT_ORIGIN_TOKEN or
# LUCIDOS_API_PORT is missing. See crates/lucidos-engine/src/api/actor.rs
# for the engine-side token resolution and crates/lucidos-cli/src/http.rs
# for the parallel CLI implementation.
import os as _os

_TOKEN = _os.environ.get("LUCIDOS_AGENT_ORIGIN_TOKEN")
_PORT = _os.environ.get("LUCIDOS_API_PORT")

if _TOKEN and _PORT:
    import http.client as _http_client

    _HEADER_TOKEN = "x-lucidos-agent-origin-token"
    _LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1", "[::1]"}
    _orig_request = _http_client.HTTPConnection.request

    def _targets_engine(conn):
        try:
            host = (conn.host or "").lower()
            port = str(conn.port or "")
        except Exception:
            return False
        return host in _LOCAL_HOSTS and port == _PORT

    def _request_with_token(self, method, url, body=None, headers=None, *args, **kwargs):
        if headers is None:
            headers = {}
        # Caller may pass a dict OR an iterable of (k, v) pairs — http.client
        # accepts both. Normalise to a dict so we can do case-insensitive
        # "is this header already set" checks without mutating the caller's
        # object.
        if not isinstance(headers, dict):
            try:
                headers = dict(headers)
            except Exception:
                # Opaque headers (e.g. HTTPMessage) — leave them alone; the
                # caller knows what it's doing.
                return _orig_request(self, method, url, body, headers, *args, **kwargs)
        else:
            headers = dict(headers)
        if _targets_engine(self):
            present = {k.lower() for k in headers}
            if _HEADER_TOKEN not in present:
                headers[_HEADER_TOKEN] = _TOKEN
        return _orig_request(self, method, url, body, headers, *args, **kwargs)

    _http_client.HTTPConnection.request = _request_with_token
"#;

pub struct PythonRuntime {
    workspace_path: PathBuf,
    exhaust_path: PathBuf,
    venv_path: PathBuf,
    python_bin: PathBuf,
}

impl PythonRuntime {
    /// Build the Python runtime rooted at the canonicalized `workspace_path`.
    /// Returns `Err` if canonicalization fails — a missing or unreadable
    /// workspace at boot must surface as engine-startup failure.
    pub fn new(workspace_path: PathBuf) -> Result<Self, String> {
        let workspace_path = workspace_path.canonicalize().map_err(|e| {
            format!(
                "PythonRuntime: failed to canonicalize workspace path {}: {}",
                workspace_path.display(),
                e
            )
        })?;
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

        Ok(Self {
            workspace_path,
            exhaust_path,
            venv_path,
            python_bin,
        })
    }

    /// Path to the venv-rooted Python interpreter. The file only exists
    /// after `ensure_venv()` has run successfully; in-process callers that
    /// hand this off to a subprocess (e.g. `run_python_background` shelling
    /// out via `BackgroundBashRegistry::spawn`) MUST `ensure_venv().await`
    /// first so the binary is present before spawn time.
    pub fn python_bin(&self) -> &std::path::Path {
        &self.python_bin
    }

    /// Create the venv if it doesn't already exist. Called lazily on first
    /// execution by `run_script` and `ensure_packages`; also called
    /// directly by `run_python_background` before handing the venv-rooted
    /// python invocation to the background bash registry.
    pub async fn ensure_venv(&self) -> Result<(), String> {
        if self.python_bin.exists() {
            // Always re-install the agent-origin shim even on a pre-existing
            // venv — a Lucidos upgrade that changes the bootstrap (e.g. new
            // header, tightened gate, the sitecustomize→.pth migration) must
            // reach existing installs without forcing users to delete
            // `.lucidos/runtime/python`.
            self.install_agent_origin_shim()?;
            return Ok(());
        }

        log!(
            "[Python] Creating virtual environment at {}",
            self.venv_path.display()
        );

        let parent = self
            .venv_path
            .parent()
            .ok_or_else(|| format!("venv path has no parent: {}", self.venv_path.display()))?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;

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
        self.install_agent_origin_shim()?;
        Ok(())
    }

    /// Drop the agent-origin shim into the venv's `site-packages/` so every
    /// Python invocation rooted in this venv (run_python, run_python_background,
    /// any user script that imports tooling) auto-forwards the engine's
    /// agent-origin token + spawning thread id on HTTP requests aimed at the
    /// engine API.
    ///
    /// Two files, side by side in `site-packages/`:
    ///   - `_lucidos_agent_origin.py` — the shim (`AGENT_ORIGIN_SHIM_PY`).
    ///   - `_lucidos_agent_origin.pth` — one line, `import _lucidos_agent_origin`.
    ///
    /// The `.pth` is what makes this load. We deliberately do NOT use
    /// `sitecustomize.py`: CPython imports only the first `sitecustomize` on
    /// `sys.path`, and Homebrew's `python@3.x` ships its own in the stdlib dir
    /// (earlier on the path), silently shadowing a venv copy — which is exactly
    /// why `urllib` calls were landing as "You" on Homebrew machines. `site`
    /// execs the `import` line of every `.pth`, so a uniquely-named module can
    /// never be shadowed.
    ///
    /// Without this, the LLM-driven `urllib.request.urlopen(
    /// "https://localhost:PORT/api/v1/changes/<id>/apply")` call lands as
    /// `Api { mode: Human }` and the timeline renders the apply as "You" —
    /// the original incident from `dec8a433-…` that motivated this fix.
    /// The `lucidos` CLI already forwards the same headers via
    /// `crates/lucidos-cli/src/http.rs`; this brings raw `urllib` / `requests`
    /// / `http.client` callers up to the same standard with no agent code
    /// change.
    ///
    /// Re-written on every `ensure_venv()` so a Lucidos upgrade that changes
    /// the bootstrap takes effect on next run without `rm -rf .lucidos/runtime`.
    /// Inert when `LUCIDOS_AGENT_ORIGIN_TOKEN` or `LUCIDOS_API_PORT` are unset
    /// at script-startup time — pip installs to PyPI (different host) and any
    /// non-engine localhost call are never touched.
    fn install_agent_origin_shim(&self) -> Result<(), String> {
        let lib_dir = self.venv_path.join("lib");
        let read_dir = fs::read_dir(&lib_dir)
            .map_err(|e| format!("read venv lib dir {}: {}", lib_dir.display(), e))?;
        for entry in read_dir.flatten() {
            let path = entry.path();
            let is_python_subdir = path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("python"));
            if !is_python_subdir {
                continue;
            }
            let site_packages = path.join("site-packages");
            if site_packages.is_dir() {
                let module = site_packages.join("_lucidos_agent_origin.py");
                fs::write(&module, AGENT_ORIGIN_SHIM_PY)
                    .map_err(|e| format!("write {}: {}", module.display(), e))?;
                // `.pth` lines that start with `import` are exec'd by `site`
                // for every `.pth` in the dir — not subject to the single
                // `sitecustomize` name shadow.
                let pth = site_packages.join("_lucidos_agent_origin.pth");
                fs::write(&pth, "import _lucidos_agent_origin\n")
                    .map_err(|e| format!("write {}: {}", pth.display(), e))?;
                // Remove any stale sitecustomize.py from a pre-.pth install so
                // we don't leave a shadowed, misleading copy behind.
                let _ = fs::remove_file(site_packages.join("sitecustomize.py"));
                return Ok(());
            }
        }
        Err(format!(
            "no `pythonX.Y/site-packages` under {}",
            lib_dir.display()
        ))
    }

    /// Fresh per-run directory under `.lucidos/exhaust/` — the audit sink for
    /// this run's `stdout.txt` / `stderr.txt` (and, for string-code runs, the
    /// `script.py` copy itself).
    fn new_run_dir(&self) -> Result<PathBuf, String> {
        let task_id = uuid::Uuid::new_v4().to_string();
        let task_dir = self.exhaust_path.join(&task_id);
        fs::create_dir_all(&task_dir).map_err(|e| e.to_string())?;
        Ok(task_dir)
    }

    /// Shared execution core: writes preamble + user code to a script, runs it, returns output.
    /// The preamble is empty for `execute_with_env` (no monkey-patching) and the staging-redirect
    /// `builtins.open` shim for `execute_staged`. Scripts run with full host write access — the
    /// previous outside-workspace write-guard was removed for consistency with `run_bash`,
    /// `run_bash_background`, and `run_python_background`, none of which sandbox writes.
    ///
    /// This is the *string-code* path (`run_python`, via `execute_staged`),
    /// where the LLM supplies code that has no home on disk — so the copy under
    /// `<exhaust>/<uuid>/script.py` IS the script and `__file__` legitimately
    /// points there. (`run_python_background` doesn't come through here; it
    /// writes its own copy under the same exhaust layout and hands the
    /// invocation to `BackgroundBashRegistry::spawn`.) A script that already
    /// exists on disk must go through `execute_file_with_env` instead.
    async fn run_script(
        &self,
        preamble: &str,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.ensure_venv().await?;
        let run_dir = self.new_run_dir()?;

        let full_code = format!("{}\n{}", preamble, code);
        let script_path = run_dir.join("script.py");
        fs::write(&script_path, &full_code).map_err(|e| e.to_string())?;

        self.spawn_python(&script_path, &run_dir, env_vars).await
    }

    /// Execute an EXISTING Python file **in place**, from its real on-disk path.
    ///
    /// This is the path for a *script* — code that already lives on disk beside
    /// its consumer, invoked by a script trigger's
    /// `run = { type = "script", path = "triggers/<slug>/scripts/run.py" }`.
    /// The difference from `run_script` is the whole point: the interpreter
    /// reads the code from where the human wrote it, so `__file__` resolves to
    /// the real file and a script can reach its siblings the ordinary way
    /// (`os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state")`).
    /// Running a copy out of the exhaust dir silently redirects every such path
    /// into a phantom `.lucidos/exhaust/...` directory — reads return defaults,
    /// writes land where nobody looks, and nothing errors. That is exactly how
    /// the 2026-07-29 `notary-verdict-watch` trigger missed the user's DMG
    /// approval while the approval sat in the real `state/` dir.
    ///
    /// Identical to `run_script` in every other respect (venv, cwd, kill_on_drop,
    /// env, output sanitizing, exhaust audit logs, error shaping).
    pub async fn execute_file_with_env(
        &self,
        script_path: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.ensure_venv().await?;
        let run_dir = self.new_run_dir()?;
        self.spawn_python(script_path, &run_dir, env_vars).await
    }

    /// Spawn `python <script_path>` in the workspace, capture output, drop the
    /// audit logs in `run_dir`, and shape a non-zero exit into an `Err`.
    /// `script_path` is either the exhaust copy written by `run_script` or an
    /// on-disk script run in place (`execute_file_with_env`) — the spawn is the
    /// same either way, only the provenance of the file differs.
    async fn spawn_python(
        &self,
        script_path: &std::path::Path,
        run_dir: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        let mut cmd = tokio::process::Command::new(&self.python_bin);
        cmd.arg(script_path)
            .current_dir(&self.workspace_path)
            .stdin(std::process::Stdio::null())
            // Without kill_on_drop, dropping this command future on cancel
            // (the agent loop's run_tool_with_cancel wrapper) leaves the
            // python child orphaned — a hung urlopen() / time.sleep() keeps
            // running in the background while the engine logs nothing about
            // it. With it, the OS sends SIGKILL when the future drops, so a
            // user clicking Stop reliably reaps the subprocess.
            .kill_on_drop(true);
        for (key, value) in &env_vars {
            cmd.env(key, value);
        }
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to execute Python: {}", e))?;

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        if let Err(e) = fs::write(run_dir.join("stdout.txt"), &stdout) {
            log!("[Python] Failed to write stdout debug log: {}", e);
        }
        if let Err(e) = fs::write(run_dir.join("stderr.txt"), &stderr) {
            log!("[Python] Failed to write stderr debug log: {}", e);
        }

        if output.status.success() {
            Ok(stdout)
        } else {
            // Stripping middle frames + tail-clipping pre-traceback noise
            // keeps the LLM context lean. The FULL stderr is still on
            // disk at exhaust_path/<task_id>/stderr.txt for debugging.
            let trimmed = truncate_python_error(&stderr);
            Err(format!("Python error:\n{}", trimmed))
        }
    }

    fn workspace_str_escaped(&self) -> String {
        self.workspace_path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
    }

    /// Build preamble that redirects writes under `data/` to the staging directory and
    /// reads from staging first (so a script can read back what it just wrote). Writes
    /// outside `data/` go straight to the host. The staging redirect is what gives
    /// `run_python` its atomic-commit semantics — apps see old `data/` files until the
    /// script finishes and the engine commits the staged tree.
    ///
    /// The `_data_dir` prefix carries a trailing separator on purpose. Without it a
    /// bare `startswith` also matched every SIBLING whose name merely starts with
    /// `data` (`database.json`, `datasets/`, `data_backup/`): those writes were
    /// diverted into staging, and since the committer only copies `<staging>/data`
    /// before deleting the whole staging tree, they were silently discarded while the
    /// tool still reported success.
    fn staging_preamble(&self, staging_dir: &std::path::Path) -> String {
        let workspace = self.workspace_str_escaped();
        let staging = staging_dir
            .canonicalize()
            .unwrap_or(staging_dir.to_path_buf())
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        format!(
            "import builtins as _builtins, os as _os\n\
             _workspace = _os.path.realpath('{workspace}')\n\
             _staging = _os.path.realpath('{staging}')\n\
             _data_dir = _os.path.join(_workspace, 'data') + _os.sep\n\
             def _staged_open(file, mode='r', *args, _orig=_builtins.open, **kwargs):\n\
             \x20   if not isinstance(file, (str, bytes, _os.PathLike)):\n\
             \x20       return _orig(file, mode, *args, **kwargs)\n\
             \x20   real = _os.path.realpath(str(file))\n\
             \x20   is_write = any(c in str(mode) for c in 'wxa+')\n\
             \x20   if is_write:\n\
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
             _builtins.open = _staged_open\n\
             del _staged_open\n",
            workspace = workspace,
            staging = staging,
        )
    }

    pub async fn execute_with_env(
        &self,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.run_script("", code, env_vars).await
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
        self.run_script(&preamble, code, env_vars).await
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
            // pip install can hang on a slow network — kill_on_drop ensures
            // the cancel path reaps it instead of leaking the child.
            .kill_on_drop(true)
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

/// Hard cap on lines/bytes we'll send back as a Python error. Above this,
/// the truncator kicks in. 30 lines + 4 KB is enough room for a full
/// 5-frame traceback, the exception line, and a couple of pre-crash
/// stderr prints — below that, the agent rarely benefits from more
/// context (the full stderr is on disk at exhaust_path/<id>/stderr.txt
/// either way).
const PY_ERROR_LINE_BUDGET: usize = 30;
const PY_ERROR_BYTE_BUDGET: usize = 4096;

/// Strip middle frames out of a Python traceback and tail-clip
/// pre-traceback stderr noise so the LLM-facing tool result stays
/// useful without bloating context. Preserves: any text before the
/// `Traceback (...)` line (up to a small cap), the traceback header,
/// the first and last `File "..."` frames (with the indented code line
/// below each), and the final `ExceptionClass: message` line.
///
/// Why this exists: the personal-workspace chat agent observed in
/// `dev` thread `9d44e81c…` returned 19 errors in 500 events, most of
/// them ~30-line `ModuleNotFoundError` tracebacks dominated by
/// interpreter / importer frames the LLM doesn't act on. Trimming the
/// middle here cuts each error's context cost without dropping the
/// signal (exception line + the user frame where it fired).
pub(crate) fn truncate_python_error(stderr: &str) -> String {
    let line_count = stderr.lines().count();
    if line_count <= PY_ERROR_LINE_BUDGET && stderr.len() <= PY_ERROR_BYTE_BUDGET {
        return stderr.to_string();
    }
    let lines: Vec<&str> = stderr.lines().collect();
    let traceback_start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("Traceback "));

    if let Some(tb) = traceback_start {
        // Collect File-frame indices inside the traceback.
        let file_idxs: Vec<usize> = lines
            .iter()
            .enumerate()
            .skip(tb)
            .filter(|(_, l)| l.trim_start().starts_with("File \""))
            .map(|(i, _)| i)
            .collect();

        // Find the exception line: last non-empty, non-whitespace line that
        // doesn't start with a space (frames are indented; the exception
        // line is flush-left and looks like `ClassName: message`).
        let exception_idx = lines
            .iter()
            .enumerate()
            .rev()
            .find(|(i, l)| {
                *i > tb
                    && !l.is_empty()
                    && !l.starts_with(' ')
                    && !l.starts_with('\t')
                    && !l.trim_start().starts_with("File \"")
            })
            .map(|(i, _)| i);

        if file_idxs.len() > 4 {
            let mut out = String::new();
            // Pre-traceback stderr noise (warnings, print(file=sys.stderr)
            // calls) — keep the last few lines so the LLM still sees what
            // the script logged before crashing.
            if tb > 0 {
                let pre_keep = tb.saturating_sub(3);
                if pre_keep > 0 {
                    out.push_str(&format!(
                        "[... {} lines of pre-traceback stderr omitted ...]\n",
                        pre_keep
                    ));
                }
                for line in &lines[pre_keep..tb] {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            // Traceback header.
            out.push_str(lines[tb]);
            out.push('\n');
            // First user frame + its indented code line if present.
            let first_file = file_idxs[0];
            out.push_str(lines[first_file]);
            out.push('\n');
            if let Some(code_line) = lines.get(first_file + 1) {
                if (code_line.starts_with("    ") || code_line.starts_with("\t"))
                    && !code_line.trim_start().starts_with("File \"")
                {
                    out.push_str(code_line);
                    out.push('\n');
                }
            }
            // Omitted middle.
            let omitted = file_idxs.len() - 2;
            out.push_str(&format!("  [... {} frames omitted ...]\n", omitted));
            // Last user frame + its indented code line if present.
            let last_file = *file_idxs.last().unwrap();
            out.push_str(lines[last_file]);
            out.push('\n');
            if let Some(code_line) = lines.get(last_file + 1) {
                if (code_line.starts_with("    ") || code_line.starts_with("\t"))
                    && !code_line.trim_start().starts_with("File \"")
                {
                    out.push_str(code_line);
                    out.push('\n');
                }
            }
            // Exception line. `> last_file` (not `> last_file + 1`) so
            // we still emit when Python omitted the indented source line
            // below the last frame — frozen importlib._bootstrap frames,
            // C-extension frames, and some re-raise paths leave the
            // exception immediately at last_file + 1, and the code-line
            // if-let above won't have pushed it (exception is flush-left,
            // not indented). The two paths are mutually exclusive: at the
            // same index, a line is either indented (code) or flush-left
            // (exception) — never both.
            if let Some(ex) = exception_idx {
                if ex > last_file {
                    out.push_str(lines[ex]);
                    out.push('\n');
                }
            }
            return out.trim_end_matches('\n').to_string();
        }
    }

    // No traceback (or traceback was already short) — pure stderr noise
    // overrun, e.g. a script that printed 200 progress lines before
    // crashing. Keep the tail.
    let drop = line_count.saturating_sub(PY_ERROR_LINE_BUDGET);
    let mut out = String::new();
    if drop > 0 {
        out.push_str(&format!("[... {} lines of stderr omitted ...]\n", drop));
    }
    out.push_str(&lines[drop..].join("\n"));
    out
}

#[cfg(test)]
#[path = "python_tests.rs"]
mod tests;
