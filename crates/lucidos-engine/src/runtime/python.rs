use crate::core::sanitize_for_jsonb;
use crate::core::script_output::{
    script_error_output, truncate_python_error, PY_ERROR_BYTE_BUDGET,
};
use crate::core::shell::{finalize_stream, KEPT_BYTES_CAP};
use crate::runtime::agent_sandbox::Sandbox;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// INVARIANT: Hard wall-clock ceiling for ONE synchronous Python child, shared by the
/// `run_python` tool (via `execute_staged`) and by an in-place script run
/// (`execute_file_with_env`).
///
/// Deliberately read from `llm::tools::MAX_TIMEOUT_SECS` rather than restated,
/// because that constant is already the documented number: three sibling tool
/// descriptions in `llm/tools/exec.rs` and `system-knowhow/running-python.md`
/// all quote "run_python's 300s sync ceiling". Until this was wired the
/// promise was fiction. The synchronous python path had no timeout at all, so
/// a script with an infinite loop ran at 100% CPU until a human noticed and
/// killed the OS process by hand. `run_bash` (`engine::tools::bash`) and the
/// scheduled `.sh` path (`engine_impl::scripts`) already spend exactly this
/// budget, enforced exactly this way.
///
/// The ceiling covers the CHILD only. `ensure_venv` and `ensure_packages` are
/// awaited before the spawn, so a cold first-run venv creation or a large pip
/// install never eats a script's budget. Neither of those is bounded here on
/// purpose: a legitimate `torch` install routinely outruns 300s, and killing
/// it would break setup rather than a runaway.
const EXECUTION_TIMEOUT_SECS: u64 = crate::llm::tools::MAX_TIMEOUT_SECS;

/// INVARIANT: How long a run's exhaust dir survives before the next `PythonRuntime::new`
/// reclaims it.
///
/// A sweep is needed at all because nothing else prunes the sink: a one-minute
/// script trigger mints 1,440 dirs a day and the volume fills with no warning.
/// A blanket wipe is the wrong sweep, because `system-knowhow/running-python.md`
/// points debugging at `.lucidos/exhaust/<run_id>/stderr.txt`.
///
/// Seven days keeps that affordance for the Friday failure a user reads on
/// Monday. It caps the same trigger near 10,000 dirs instead of 525,000 a year.
const EXHAUST_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// INVARIANT: Python bootstrap module dropped into the venv's `site-packages/` as
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
# LUCIDOS_API_PORT is missing. See crates/lucidos-agents-api/src/api/actor.rs
# for the engine-side token resolution and crates/lucidos-cli/src/http.rs
# for the parallel CLI implementation.
import os as _os

_TOKEN = _os.environ.get("{ENV_AGENT_ORIGIN_TOKEN}")
_PORT = _os.environ.get("LUCIDOS_API_PORT")

if _TOKEN and _PORT:
    import http.client as _http_client

    _HEADER_TOKEN = "{HEADER_AGENT_ORIGIN_TOKEN}"
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

fn agent_origin_shim_py() -> String {
    use crate::core::subprocess_origin::{ENV_AGENT_ORIGIN_TOKEN, HEADER_AGENT_ORIGIN_TOKEN};
    AGENT_ORIGIN_SHIM_PY
        .replace("{ENV_AGENT_ORIGIN_TOKEN}", ENV_AGENT_ORIGIN_TOKEN)
        .replace("{HEADER_AGENT_ORIGIN_TOKEN}", HEADER_AGENT_ORIGIN_TOKEN)
}

pub struct PythonRuntime {
    workspace_path: PathBuf,
    exhaust_path: PathBuf,
    venv_path: PathBuf,
    python_bin: PathBuf,
    venv_setup: tokio::sync::Mutex<()>,
    execution_timeout: Duration,
}

impl PythonRuntime {
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
            log_legacy!(
                "[Python] Failed to create exhaust dir at {}: {}",
                exhaust_path.display(),
                e
            );
        }
        Self::sweep_stale_run_dirs(&exhaust_path);

        let staging_root = workspace_path.join(".lucidos/staging");
        if staging_root.exists() {
            if let Err(e) = fs::remove_dir_all(&staging_root) {
                log_legacy!(
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
            venv_setup: tokio::sync::Mutex::new(()),
            execution_timeout: Duration::from_secs(EXECUTION_TIMEOUT_SECS),
        })
    }

    /// INVARIANT: Reclaim every run dir past `EXHAUST_RETENTION`. Runs once per engine
    /// start, so the whole cost is one `read_dir` of the exhaust root.
    ///
    /// Destructive, so it deletes only what it can prove is a spent run dir: a
    /// real directory, named by the UUID `new_run_dir` mints, with a
    /// readable mtime past the window. `file_type` reports a symlink without
    /// following it, so a link planted here is never a delete of its target.
    /// Anything the probe cannot answer is kept, because an unreadable entry is
    /// not evidence of an expired one.
    fn sweep_stale_run_dirs(exhaust_path: &std::path::Path) {
        let Ok(entries) = fs::read_dir(exhaust_path) else {
            return;
        };
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            let named_by_run_id = entry
                .file_name()
                .to_str()
                .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok());
            if !named_by_run_id || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let Ok(age) = now.duration_since(modified) else {
                continue;
            };
            if age <= EXHAUST_RETENTION {
                continue;
            }
            let path = entry.path();
            if let Err(e) = fs::remove_dir_all(&path) {
                log_legacy!(
                    "[Python] Failed to reclaim stale exhaust dir {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_execution_timeout(mut self, timeout: Duration) -> Self {
        self.execution_timeout = timeout;
        self
    }

    /// INVARIANT: Path to the venv-rooted Python interpreter. The file only exists
    /// after `ensure_venv()` has run successfully; in-process callers that
    /// hand this off to a subprocess (e.g. `run_python_background` shelling
    /// out via `BackgroundBashRegistry::spawn`) MUST `ensure_venv().await`
    /// first so the binary is present before spawn time.
    pub fn python_bin(&self) -> &std::path::Path {
        &self.python_bin
    }

    pub async fn ensure_venv(&self) -> Result<(), String> {
        self.ensure_venv_with(tokio::process::Command::new("python3"))
            .await
    }

    async fn ensure_venv_with(&self, mut command: tokio::process::Command) -> Result<(), String> {
        let _setup = self.venv_setup.lock().await;
        if self.python_bin.exists() && self.venv_path.join("bin/pip").is_file() {
            // INVARIANT: Always re-install the agent-origin shim even on a pre-existing
            // venv — a Lucidos upgrade that changes the bootstrap (e.g. new
            // header, tightened gate, the sitecustomize→.pth migration) must
            // reach existing installs without forcing users to delete
            // `.lucidos/runtime/python`.
            self.install_agent_origin_shim()?;
            return Ok(());
        }

        log_legacy!(
            "[Python] Creating virtual environment at {}",
            self.venv_path.display()
        );

        let parent = self
            .venv_path
            .parent()
            .ok_or_else(|| format!("venv path has no parent: {}", self.venv_path.display()))?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;

        let venv_existed = self.venv_path.exists();
        let output = command
            .args(["-m", "venv"])
            .arg(&self.venv_path)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;

        let error = match output {
            Ok(output) if output.status.success() => None,
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut error = format!(
                    "Failed to create venv ({}):\n{stdout}{stderr}",
                    output.status
                );
                if stdout.contains("ensurepip is not available")
                    || stderr.contains("ensurepip is not available")
                {
                    error.push_str("\nOn Debian or Ubuntu, install python3-venv (or python3.X-venv for your Python version).");
                }
                Some(error)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(format!(
                "Failed to launch {:?}: {error}. Install Python 3 with venv and ensurepip.",
                command.as_std().get_program()
            )),
            Err(error) => Some(format!("Failed to create venv: {error}")),
        };
        if let Some(mut error) = error {
            if venv_existed {
                log_legacy!(
                    "[Python] Kept the existing venv at {} after a failed repair",
                    self.venv_path.display()
                );
            } else if let Err(cleanup) = self.remove_partial_venv() {
                error.push_str(&format!("\n{cleanup}"));
            }
            return Err(error);
        }

        log_legacy!("[Python] Virtual environment created");
        self.install_agent_origin_shim()?;
        Ok(())
    }

    fn remove_partial_venv(&self) -> Result<(), String> {
        match fs::remove_dir_all(&self.venv_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Failed to remove partial venv at {}: {error}",
                self.venv_path.display()
            )),
        }
    }

    /// INVARIANT: Drop the agent-origin shim into the venv's `site-packages/` so every
    /// Python invocation rooted in this venv (run_python, run_python_background,
    /// any user script that imports tooling) auto-forwards the engine's
    /// agent-origin token + spawning thread id on HTTP requests aimed at the
    /// engine API.
    ///
    /// Two files, side by side in `site-packages/`:
    ///   - `_lucidos_agent_origin.py`, the shim (`agent_origin_shim_py`).
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
                fs::write(&module, agent_origin_shim_py())
                    .map_err(|e| format!("write {}: {}", module.display(), e))?;
                let pth = site_packages.join("_lucidos_agent_origin.pth");
                fs::write(&pth, "import _lucidos_agent_origin\n")
                    .map_err(|e| format!("write {}: {}", pth.display(), e))?;
                let _ = fs::remove_file(site_packages.join("sitecustomize.py"));
                return Ok(());
            }
        }
        Err(format!(
            "no `pythonX.Y/site-packages` under {}",
            lib_dir.display()
        ))
    }

    fn new_run_dir(&self) -> Result<PathBuf, String> {
        let task_id = uuid::Uuid::new_v4().to_string();
        let task_dir = self.exhaust_path.join(&task_id);
        fs::create_dir_all(&task_dir).map_err(|e| e.to_string())?;
        Ok(task_dir)
    }

    async fn run_script(
        &self,
        sandbox: &Sandbox,
        preamble: &str,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.ensure_venv().await?;
        let run_dir = self.new_run_dir()?;

        let full_code = format!("{}\n{}", preamble, code);
        let script_path = run_dir.join("script.py");
        fs::write(&script_path, &full_code).map_err(|e| e.to_string())?;

        self.spawn_python(sandbox, &script_path, &run_dir, env_vars)
            .await
    }

    /// INVARIANT: Execute an EXISTING Python file **in place**, from its real on-disk path.
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
        sandbox: &Sandbox,
        script_path: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.ensure_venv().await?;
        let run_dir = self.new_run_dir()?;
        self.spawn_python(sandbox, script_path, &run_dir, env_vars)
            .await
    }

    /// INVARIANT: Spawn `python <script_path>` in the workspace, capture output, drop the
    /// audit logs in `run_dir`, and shape a non-zero exit into an `Err`.
    /// `script_path` is either the exhaust copy written by `run_script` or an
    /// on-disk script run in place (`execute_file_with_env`) — the spawn is the
    /// same either way, only the provenance of the file differs.
    ///
    /// Bounded by `self.execution_timeout` ([`EXECUTION_TIMEOUT_SECS`]). Both
    /// callers get the ceiling: an unbounded trigger script is the same hazard
    /// as an unbounded `run_python`, and the `.sh` half of the very same
    /// scheduled-script dispatch has spent a 300s budget all along.
    ///
    /// An expiry is shaped into the ordinary `Err` a crash produces, so the
    /// agent reads a normal failed tool result instead of watching a turn hang.
    /// Every caller therefore takes its existing failure path unchanged, which
    /// is what keeps `run_python`'s staging invariant intact: a timed-out run
    /// removes its staging tree and commits nothing, exactly like a crash.
    async fn spawn_python(
        &self,
        sandbox: &Sandbox,
        script_path: &std::path::Path,
        run_dir: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        let mut cmd = sandbox
            .command(&self.python_bin)
            .map_err(|e| format!("Failed to execute Python: {}", e))?;
        cmd.arg(script_path)
            .current_dir(&self.workspace_path)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        crate::core::apply_to_subprocess_env(
            &mut cmd,
            &env_vars,
            crate::core::InheritedSecrets::Strip(crate::core::inherited_secret_env()),
        );
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let started = chrono::Utc::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to execute Python: {}", e))?;
        let pid = child.id();
        let (stdout_pipe, stderr_pipe) = (child.stdout.take(), child.stderr.take());
        let run = async {
            tokio::join!(
                read_capped(stdout_pipe, Keep::Head),
                read_capped(stderr_pipe, Keep::Tail),
                child.wait()
            )
        };
        let (out, err, status) = match tokio::time::timeout(self.execution_timeout, run).await {
            Ok((out, err, Ok(status))) => (out, err, status),
            Ok((_, _, Err(e))) => return Err(format!("Failed to execute Python: {}", e)),
            Err(_) => return Err(self.on_execution_timeout(run_dir)),
        };

        let stdout = finalize_stream(&out.kept, out.total);
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&err.kept));

        if let Err(e) = fs::write(run_dir.join("stdout.txt"), &stdout) {
            log_legacy!("[Python] Failed to write stdout debug log: {}", e);
        }
        if let Err(e) = fs::write(run_dir.join("stderr.txt"), &stderr) {
            log_legacy!("[Python] Failed to write stderr debug log: {}", e);
        }

        let outcome = crate::core::shell::TaskOutcome::from_status(status);
        if outcome.is_success() {
            Ok(stdout)
        } else {
            let stdout_end = sanitize_for_jsonb(&String::from_utf8_lossy(&out.end));
            let error =
                script_error_output(&truncate_python_error(&stderr), &stdout_end, out.total);
            let denial = sandbox
                .seatbelt_write_refusal(pid, started, &outcome)
                .await
                .map(|d| format!("\n{d}"))
                .unwrap_or_default();
            Err(format!("Python error:\n{error}{denial}"))
        }
    }

    fn on_execution_timeout(&self, run_dir: &std::path::Path) -> String {
        let message = format!(
            "Python script timed out after {}s, the hard ceiling for synchronous execution. \
             The script was killed and wrote no output. Move work that needs longer to \
             run_python_background, which runs up to {}s.",
            self.execution_timeout.as_secs(),
            crate::llm::tools::BG_MAX_TIMEOUT_SECS,
        );
        log_legacy!("[Python] {}", message);
        if let Err(e) = fs::write(
            run_dir.join("stderr.txt"),
            format!("[lucidos] {}\n", message),
        ) {
            log_legacy!(
                "[Python] Failed to write timeout note to stderr debug log: {}",
                e
            );
        }
        message
    }

    fn workspace_str_escaped(&self) -> String {
        self.workspace_path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
    }

    /// INVARIANT: Build preamble that redirects writes under `data/` to the staging directory and
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
    ///
    /// `_data_root` is realpath'd, exactly like the paths matched against it.
    /// A `data` symlink onto another volume is a supported layout, and against
    /// an unresolved root no write ever matched: every `data/` write landed on
    /// the host, and the committer then found no staged tree to publish. The
    /// staged layout stays `<staging>/data/<rel>` either way, which is the one
    /// shape the committer reads back.
    ///
    /// The write branch seeds the staged file from the real one first. A mode
    /// holding `w` truncates, so it is never seeded. Every other write mode
    /// (`a`, `r+`, `x`) keeps what the file already held, and an empty staged
    /// file loses it. The seed runs once per run: a later open must not
    /// overwrite bytes this run already staged.
    fn staging_preamble(&self, staging_dir: &std::path::Path) -> String {
        let workspace = self.workspace_str_escaped();
        let staging = staging_dir
            .canonicalize()
            .unwrap_or(staging_dir.to_path_buf())
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        format!(
            "import builtins as _builtins, os as _os, shutil as _shutil\n\
             _workspace = _os.path.realpath('{workspace}')\n\
             _staging = _os.path.realpath('{staging}')\n\
             _data_root = _os.path.realpath(_os.path.join(_workspace, 'data'))\n\
             _data_dir = _data_root + _os.sep\n\
             def _staged_path(real):\n\
             \x20   rel = _os.path.relpath(real, _data_root)\n\
             \x20   return _os.path.join(_staging, 'data', rel)\n\
             def _staged_open(file, mode='r', *args, _orig=_builtins.open, **kwargs):\n\
             \x20   if not isinstance(file, (str, bytes, _os.PathLike)):\n\
             \x20       return _orig(file, mode, *args, **kwargs)\n\
             \x20   real = _os.path.realpath(str(file))\n\
             \x20   is_write = any(c in str(mode) for c in 'wxa+')\n\
             \x20   if is_write:\n\
             \x20       if real.startswith(_data_dir):\n\
             \x20           staged = _staged_path(real)\n\
             \x20           _os.makedirs(_os.path.dirname(staged), exist_ok=True)\n\
             \x20           truncating = 'w' in str(mode)\n\
             \x20           fresh = not _os.path.exists(staged)\n\
             \x20           if not truncating and fresh and _os.path.exists(real):\n\
             \x20               _shutil.copy2(real, staged)\n\
             \x20           return _orig(staged, mode, *args, **kwargs)\n\
             \x20   elif real.startswith(_data_dir):\n\
             \x20       staged = _staged_path(real)\n\
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
        sandbox: &Sandbox,
        code: &str,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, String> {
        self.run_script(sandbox, "", code, env_vars).await
    }

    pub async fn execute_staged(
        &self,
        sandbox: &Sandbox,
        code: &str,
        env_vars: Vec<(String, String)>,
        staging_dir: &std::path::Path,
    ) -> Result<String, String> {
        fs::create_dir_all(staging_dir).map_err(|e| e.to_string())?;
        let preamble = self.staging_preamble(staging_dir);
        self.run_script(sandbox, &preamble, code, env_vars).await
    }

    pub async fn execute(&self, sandbox: &Sandbox, code: &str) -> Result<String, String> {
        self.execute_with_env(sandbox, code, vec![]).await
    }

    pub async fn ensure_packages(
        &self,
        sandbox: &Sandbox,
        packages: &[&str],
    ) -> Result<(), String> {
        if packages.is_empty() {
            return Ok(());
        }
        for spec in packages {
            check_requirement_spec(spec)?;
        }
        self.ensure_venv().await?;

        let mut args = vec!["-m", "pip", "install", "--quiet", "--"];
        args.extend(packages);

        let mut cmd = sandbox
            .command(&self.python_bin)
            .map_err(|e| format!("Failed to install packages: {}", e))?;
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let inherited = crate::core::inherited_secret_env();
        crate::core::apply_to_subprocess_env(
            &mut cmd,
            &[],
            crate::core::InheritedSecrets::Strip(inherited),
        );
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to install packages: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let secrets = crate::core::subprocess_secret_values(&[], inherited);
            Err(format!(
                "Failed to install packages:\n{}",
                crate::core::redact_subprocess_output(
                    &String::from_utf8_lossy(&output.stderr),
                    &secrets
                )
            ))
        }
    }
}

/// INVARIANT: Refuse a package spec that is not a plain index requirement such as
/// `pandas`, `numpy>=1.26` or `requests[socks]`. A leading `-` is a pip flag
/// (`--index-url`, `-r file`). A URL, a path, an archive file name or a
/// `name @ url` reference installs code from somewhere the user never chose.
fn check_requirement_spec(spec: &str) -> Result<(), String> {
    const ARCHIVE_SUFFIXES: &[&str] = &[
        ".whl",
        ".zip",
        ".tar.gz",
        ".tgz",
        ".tar.bz2",
        ".tbz",
        ".tar.xz",
        ".txz",
        ".tar.lz",
        ".tlz",
        ".tar.lzma",
        ".tar",
        ".egg",
    ];
    let lowered = spec.to_ascii_lowercase();
    let plain = spec
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && !spec.contains(['@', ':', '/', '\\'])
        && !ARCHIVE_SUFFIXES.iter().any(|ext| lowered.ends_with(ext));
    if plain {
        Ok(())
    } else {
        Err(format!(
            "'{spec}' is not a package name: give a PyPI name with an optional version, \
             such as 'pandas' or 'numpy>=1.26'"
        ))
    }
}

#[derive(Clone, Copy)]
enum Keep {
    Head,
    Tail,
}

#[derive(Default)]
struct CapturedStream {
    kept: Vec<u8>,
    end: Vec<u8>,
    total: u64,
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    keep: Keep,
) -> CapturedStream {
    use tokio::io::AsyncReadExt;
    let mut captured = CapturedStream::default();
    let Some(mut pipe) = pipe else {
        return captured;
    };
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                captured.total += n as u64;
                match keep {
                    Keep::Head => {
                        let room = KEPT_BYTES_CAP.saturating_sub(captured.kept.len());
                        captured.kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Keep::Tail => {
                        append_keeping_tail(&mut captured.kept, &chunk[..n], KEPT_BYTES_CAP)
                    }
                }
                append_keeping_tail(&mut captured.end, &chunk[..n], PY_ERROR_BYTE_BUDGET);
            }
            Err(e) => {
                log_legacy!("[Python] Output pipe read failed: {}", e);
                break;
            }
        }
    }
    for (buf, cap) in [
        (&mut captured.end, PY_ERROR_BYTE_BUDGET),
        (&mut captured.kept, KEPT_BYTES_CAP),
    ] {
        let excess = buf.len().saturating_sub(cap);
        buf.drain(..excess);
    }
    captured
}

fn append_keeping_tail(buf: &mut Vec<u8>, bytes: &[u8], cap: usize) {
    buf.extend_from_slice(bytes);
    if buf.len() > 2 * cap {
        buf.drain(..buf.len() - cap);
    }
}

#[cfg(test)]
#[path = "python_tests.rs"]
mod tests;

/// INVARIANT: A non-truncating write must not lose the artifact it writes into. The
/// committer copies the staged file over the real one, so a staged file that
/// starts empty erases whatever the artifact held.
#[cfg(test)]
mod staging_seed_tests {
    use super::PythonRuntime;
    use tempfile::tempdir;

    #[tokio::test]
    async fn an_append_to_an_existing_data_file_keeps_the_prior_contents() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/log.jsonl"), "one\ntwo\n").unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

        let staging = ws.join(".lucidos/staging/append-run");
        let out = runtime
            .execute_staged(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "open('data/artifacts/log.jsonl', 'a').write('three\\n')\nprint('done')",
                vec![],
                &staging,
            )
            .await
            .expect("the append must succeed");
        assert_eq!(out.trim(), "done");

        assert_eq!(
            std::fs::read_to_string(staging.join("data/artifacts/log.jsonl")).unwrap(),
            "one\ntwo\nthree\n",
            "the staged file is what the committer copies over the artifact"
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("data/artifacts/log.jsonl")).unwrap(),
            "one\ntwo\n",
            "the real artifact stays untouched until the engine commits"
        );
    }

    #[tokio::test]
    async fn a_truncating_write_to_an_existing_data_file_still_truncates() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/report.csv"), "stale,rows\n1,2\n").unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

        let staging = ws.join(".lucidos/staging/truncate-run");
        runtime
            .execute_staged(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "open('data/artifacts/report.csv', 'w').write('fresh\\n')",
                vec![],
                &staging,
            )
            .await
            .expect("the truncating write must succeed");

        assert_eq!(
            std::fs::read_to_string(staging.join("data/artifacts/report.csv")).unwrap(),
            "fresh\n",
            "a 'w' mode asked for truncation, so the seed must be skipped"
        );
    }

    #[tokio::test]
    async fn an_update_mode_open_reads_an_existing_data_file() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/notes.txt"), "hello\n").unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

        let staging = ws.join(".lucidos/staging/update-run");
        let out = runtime
            .execute_staged(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "with open('data/artifacts/notes.txt', 'r+') as f:\n    print(f.read().strip())",
                vec![],
                &staging,
            )
            .await
            .expect("r+ on an existing artifact must not raise");

        assert_eq!(out.trim(), "hello");
    }

    #[tokio::test]
    async fn a_huge_stdout_comes_back_capped_and_marked() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

        let staging = ws.join(".lucidos/staging/huge-run");
        let out = runtime
            .execute_staged(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "import sys\nsys.stdout.write('x' * 3_000_000)",
                vec![],
                &staging,
            )
            .await
            .expect("a chatty script still succeeds");

        assert!(out.len() < 200 * 1024, "returned {} bytes", out.len());
        assert!(
            out.ends_with("[truncated: 3000000 bytes total]"),
            "{}",
            &out[out.len() - 60..]
        );
    }

    #[tokio::test]
    async fn a_traceback_after_a_stderr_flood_survives() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

        let staging = ws.join(".lucidos/staging/flood-run");
        let err = runtime
            .execute_staged(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                "import sys\nsys.stderr.write('w' * 2_000_000)\nraise ValueError('the real cause')",
                vec![],
                &staging,
            )
            .await
            .expect_err("the script raises");
        assert!(
            err.contains("the real cause"),
            "{}",
            &err[..err.len().min(300)]
        );
    }
}

#[cfg(test)]
mod sandbox_regression_tests {
    use super::*;
    use crate::runtime::agent_sandbox::Sandbox;

    #[tokio::test]
    #[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
    async fn reopened_workspace_runs_python_and_reports_hidden_read_errors() {
        if let lucidos_agent_sandbox::Availability::Unavailable(why) =
            lucidos_agent_sandbox::probe()
        {
            panic!("the real sandbox is unavailable: {why}");
        }
        let scratch = tempfile::tempdir().unwrap();
        let gateway = std::fs::canonicalize(scratch.path())
            .unwrap()
            .join("gateway");
        let ws = gateway.join("workspaces/ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(gateway.join("secret"), "secret").unwrap();
        let runtime = PythonRuntime::new(ws).unwrap();
        runtime.ensure_venv().await.unwrap();
        let sandbox = Sandbox::enforced_for_test(lucidos_agent_sandbox::Policy {
            hidden: vec![gateway.clone()],
            open_within_hidden: vec![gateway.join("workspaces")],
            ..Default::default()
        });
        assert_eq!(
            runtime
                .execute(&sandbox, "print('ready')")
                .await
                .unwrap()
                .trim(),
            "ready"
        );
        let code = format!("open({:?}).read()", gateway.join("secret"));
        let error = runtime.execute(&sandbox, &code).await.unwrap_err();
        assert!(error.contains("PermissionError"), "{error}");
        assert!(
            error.contains("Operation not permitted") || error.contains("Permission denied"),
            "{error}"
        );
    }
}
