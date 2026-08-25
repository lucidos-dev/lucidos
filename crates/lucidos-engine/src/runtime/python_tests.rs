use super::*;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_execute_simple_python() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime.execute("print('hello')").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().trim(), "hello");
}

#[tokio::test]
async fn test_execute_python_error() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime.execute("raise ValueError('test error')").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("ValueError"));
}

#[tokio::test]
async fn test_execute_writes_to_workspace_cwd() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime
        .execute("open('test_file.txt', 'w').write('ok'); print('done')")
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().trim(), "done");
    assert!(dir.path().join("test_file.txt").exists());
}

#[tokio::test]
async fn test_execute_reads_anywhere() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime
        .execute("import os; print(os.path.exists('/etc'))")
        .await;
    assert!(result.is_ok());
}

/// Consistency contract: sync `run_python` writes outside the workspace must
/// succeed, the same way `run_bash`, `run_bash_background`, and
/// `run_python_background` already do. A regression that re-introduces the
/// outside-workspace write-guard would break legitimate uses like
/// `~/.cache/huggingface/...` and break the symmetry the four exec tools
/// promise the LLM.
#[tokio::test]
async fn test_execute_writes_outside_workspace_succeed() {
    let outside = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let runtime = PythonRuntime::new(workspace.path().to_path_buf()).unwrap();

    let target = outside.path().join("outside.txt");
    let target_str = target.to_string_lossy().replace('\\', "\\\\");
    let code = format!("open('{}', 'w').write('hello'); print('done')", target_str);

    let result = runtime.execute(&code).await;
    assert!(
        result.is_ok(),
        "outside-workspace write failed: {:?}",
        result
    );
    assert_eq!(result.unwrap().trim(), "done");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
}

#[tokio::test]
async fn test_staging_redirects_data_writes() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

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
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

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
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

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
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

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

/// Regression: a `data` prefix match without a trailing separator treats a
/// SIBLING like `database.json` as a `data/` write, and diverts it into the
/// staging tree. The committer copies only `<staging>/data` back before
/// deleting that tree, so the write is silently discarded while `run_python`
/// reports success.
///
/// The sibling names below are the three real shapes: a file (`database.json`),
/// a directory (`datasets/`), and an underscore variant (`data_backup/`).
#[tokio::test]
async fn test_staging_data_prefixed_siblings_are_not_diverted() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::create_dir_all(ws.join("datasets")).unwrap();
    std::fs::create_dir_all(ws.join("data_backup")).unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

    let staging = ws.join(".lucidos/staging/test-run-data-prefix");

    let result = runtime
        .execute_staged(
            "open('database.json', 'w').write('{}')\n\
             open('datasets/rows.csv', 'w').write('a,b')\n\
             open('data_backup/old.txt', 'w').write('x')\n\
             open('data/real.txt', 'w').write('staged')\n\
             print('ok')",
            vec![],
            &staging,
        )
        .await;

    assert!(result.is_ok(), "script failed: {:?}", result);

    // The three siblings land on the host, where the script asked for them.
    for sibling in ["database.json", "datasets/rows.csv", "data_backup/old.txt"] {
        assert!(
            ws.join(sibling).exists(),
            "{sibling} was diverted into staging and discarded"
        );
    }

    // A genuine `data/` write still stages rather than hitting the host
    // directly, which is what gives run_python its atomic-commit semantics.
    assert!(
        !ws.join("data/real.txt").exists(),
        "a real data/ write must stage, not land on the host before commit"
    );
    assert!(staging.join("data/real.txt").exists());
}

// ── scripts execute in place ───────────────────────────────────────────
//
// A `.py` script must run from where it lives. Running a COPY out of
// `.lucidos/exhaust/<uuid>/` points `__file__` into the exhaust dir, so every
// `__file__`-relative sibling path resolves to a phantom neighbour: reads
// return defaults, writes create the phantom dir, and nothing errors.

/// Create a realistic script-trigger layout inside `ws`:
/// `data/triggers/<slug>/scripts/run.py` holding `body`. Returns the script's
/// real absolute path.
fn write_trigger_script(ws: &std::path::Path, slug: &str, body: &str) -> std::path::PathBuf {
    let scripts_dir = ws.join("data/triggers").join(slug).join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    let script = scripts_dir.join("run.py");
    std::fs::write(&script, body).unwrap();
    script
}

/// `__file__` must be the script's REAL path, not a temp copy. This is the
/// single assertion that would have caught the original bug.
#[tokio::test]
async fn execute_file_runs_the_real_path_not_a_copy() {
    let dir = tempdir().unwrap();
    // Canonicalize: PythonRuntime canonicalizes its workspace, and on macOS
    // /var/folders/... is a symlink to /private/var/folders/..., so the path
    // the interpreter is handed must be built from the same root we compare to.
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "notary-verdict-watch",
        "import os\nprint(os.path.abspath(__file__))\n",
    );

    let out = runtime
        .execute_file_with_env(&script, vec![])
        .await
        .expect("execute_file_with_env");
    let reported = out.trim();

    assert_eq!(
        reported,
        script.to_string_lossy(),
        "__file__ must resolve to the script's real on-disk path"
    );
    assert!(
        !reported.contains("exhaust"),
        "script ran from a copy under the exhaust dir: {reported}"
    );
}

/// The consequence that actually bit: a script resolving a sibling directory
/// via `__file__` must reach the REAL sibling, not a phantom one.
#[tokio::test]
async fn execute_file_resolves_sibling_dir_via_file() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let state_dir = ws.join("data/triggers/notary-verdict-watch/state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("marker.json"),
        // A sentinel, deliberately NOT a real release version: a literal equal to
        // RELEASE would trip version_sources_test.sh's unmanaged-literal scan.
        r#"{"approved_version": "0.0.0-fixture"}"#,
    )
    .unwrap();

    // The exact shape from the incident: sibling dir via dirname(__file__)/...
    let script = write_trigger_script(
        &ws,
        "notary-verdict-watch",
        r#"
import json, os
_STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state")
try:
    with open(os.path.join(_STATE, "marker.json")) as f:
        data = json.load(f)
except FileNotFoundError:
    data = {"approved_version": None}
print(data["approved_version"])
"#,
    );

    let out = runtime
        .execute_file_with_env(&script, vec![])
        .await
        .expect("execute_file_with_env");

    assert_eq!(
        out.trim(),
        "0.0.0-fixture",
        "script must read the REAL sibling state dir, not fall back to the default"
    );
    assert!(
        !ws.join(".lucidos/exhaust/state").exists(),
        "a phantom state dir was created under .lucidos/exhaust — the script ran from a copy"
    );
}

/// stdout/stderr still land in a per-run exhaust dir for audit, and running in
/// place must NOT drop a `script.py` copy beside them.
#[tokio::test]
async fn execute_file_writes_audit_logs_without_copying_the_script() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "audit-probe",
        "import sys\nprint('to-stdout')\nprint('to-stderr', file=sys.stderr)\n",
    );

    runtime
        .execute_file_with_env(&script, vec![])
        .await
        .expect("execute_file_with_env");

    let run_dirs: Vec<_> = std::fs::read_dir(ws.join(".lucidos/exhaust"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        run_dirs.len(),
        1,
        "expected one exhaust run dir: {run_dirs:?}"
    );
    let run_dir = &run_dirs[0];

    assert_eq!(
        std::fs::read_to_string(run_dir.join("stdout.txt"))
            .unwrap()
            .trim(),
        "to-stdout"
    );
    assert!(std::fs::read_to_string(run_dir.join("stderr.txt"))
        .unwrap()
        .contains("to-stderr"));
    assert!(
        !run_dir.join("script.py").exists(),
        "in-place execution must not write a script.py copy into the exhaust dir"
    );
}

/// A failing on-disk script surfaces the same shaped error as the string
/// path — `Python error:` + the truncated traceback.
#[tokio::test]
async fn execute_file_shapes_errors_like_run_script() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(&ws, "boom", "raise ValueError('kaboom')\n");

    let err = runtime
        .execute_file_with_env(&script, vec![])
        .await
        .expect_err("script raises");
    assert!(err.starts_with("Python error:"), "got: {err}");
    assert!(err.contains("ValueError: kaboom"), "got: {err}");
}

/// Env vars reach an in-place script the same way they reach a string-code one.
/// `execute_script` injects `LUCIDOS_*`, `CRED_*`, `OAUTH_*`, and the trigger
/// event vars through exactly this argument.
#[tokio::test]
async fn execute_file_applies_env_vars() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "env-probe",
        "import os\nprint(os.environ['TRIGGER_EVENT_TYPE'])\n",
    );

    let out = runtime
        .execute_file_with_env(
            &script,
            vec![(
                "TRIGGER_EVENT_TYPE".to_string(),
                "NotaryVerdictReceived".to_string(),
            )],
        )
        .await
        .expect("execute_file_with_env");
    assert_eq!(out.trim(), "NotaryVerdictReceived");
}

/// The two paths must stay distinct. `run_python`-style string execution has no
/// file on disk, so its `__file__` legitimately IS the exhaust copy. That copy
/// is the only record of what ran, and unifying the two paths would drop it.
#[tokio::test]
async fn execute_with_env_still_runs_from_the_exhaust_copy() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let out = runtime
        .execute_with_env("import os\nprint(os.path.abspath(__file__))\n", vec![])
        .await
        .expect("execute_with_env");
    let reported = out.trim();

    assert!(
        reported.starts_with(ws.join(".lucidos/exhaust").to_string_lossy().as_ref()),
        "string-code execution must still run from the exhaust dir: {reported}"
    );
    assert!(
        reported.ends_with("script.py"),
        "string-code execution must still run a written script.py: {reported}"
    );
}

/// `python_bin()` and `ensure_venv()` are the public API the
/// `run_python_background` chat tool needs, to hand a venv-rooted python
/// invocation to `BackgroundBashRegistry::spawn`. Without them the background
/// tool cannot see the per-workspace venv. Pin both as public and working
/// together, so hiding either trips this test rather than the LLM surface.
#[tokio::test]
async fn ensure_venv_makes_python_bin_executable() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    assert!(
        !runtime.python_bin().exists(),
        "python_bin must not exist on a fresh PythonRuntime before ensure_venv"
    );

    runtime.ensure_venv().await.expect("ensure_venv");

    assert!(
        runtime.python_bin().exists(),
        "ensure_venv must materialize the python binary at python_bin()"
    );

    // The binary must actually run — a stale venv that points at a
    // missing interpreter would silently shell-fail under bash_background.
    let output = tokio::process::Command::new(runtime.python_bin())
        .args(["-c", "print('hi from venv')"])
        .output()
        .await
        .expect("invoke python_bin");
    assert!(
        output.status.success(),
        "python_bin failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("hi from venv"),
        "stdout was: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Regression: a hung subprocess parks the agent loop forever unless two
/// things hold. The loop must race `execute_tool` against cancel, and the
/// Command must set `kill_on_drop(true)`, because tokio's `Child` does not
/// kill its OS child on drop.
///
/// This test pins the second. Drop the `execute()` future against a cancel,
/// then check the python child actually exited. The marker file is the
/// witness: a surviving subprocess overwrites it after `time.sleep(2)`, a
/// SIGKILLed one leaves the initial content. The read happens after 3s, so a
/// survivor has had time to expose itself.
///
/// The cancel fires on the marker APPEARING, not on a fixed timer. A fixed
/// head start measures host speed instead. On a loaded host a cold interpreter
/// has not reached step 1, so the marker never exists and the read fails with
/// `NotFound`.
///
/// Each write lands through `os.replace`, the same witness the hard-ceiling
/// test below uses. A plain `open(...).write(...)` creates the file before it
/// flushes. The watcher could cancel inside that window, and the read would
/// then call an empty marker a survivor.
#[tokio::test]
async fn execute_kills_subprocess_when_future_dropped() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let marker = dir.path().join("marker.txt");
    let marker_str = marker.to_string_lossy().replace('\\', "\\\\");

    // Step 1: write "alive". Step 2: sleep 2s. Step 3: write "survived".
    // If kill_on_drop works, the subprocess dies between step 1 and
    // step 3, so marker stays "alive".
    let code = format!(
        "import os, time\n\
         def mark(word):\n\
        \x20   f = open('{m}.tmp', 'w')\n\
        \x20   f.write(word)\n\
        \x20   f.flush()\n\
        \x20   os.fsync(f.fileno())\n\
        \x20   f.close()\n\
        \x20   os.replace('{m}.tmp', '{m}')\n\
         mark('alive')\n\
         time.sleep(2)\n\
         mark('survived')",
        m = marker_str,
    );

    // Pre-warm the venv so the cancel below races the actual subprocess, not
    // the one-time venv creation, which takes seconds on a fresh tempdir.
    runtime.execute("print('warmup')").await.expect("warmup");

    let token = tokio_util::sync::CancellationToken::new();
    let token_clone = token.clone();
    let marker_watch = marker.clone();
    tokio::spawn(async move {
        // Cancel as soon as step 1 is on disk. The deadline is a hang
        // guard, not the trigger: reaching it means the interpreter never
        // started, and the read below then reports that plainly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !marker_watch.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        token_clone.cancel();
    });

    // Race the execute against the cancel token. On cancel, the
    // execute future is dropped — taking the inner Command future and
    // its Child handle with it. kill_on_drop(true) on the Command
    // makes the OS send SIGKILL to the python process.
    tokio::select! {
        _ = token.cancelled() => {}
        r = runtime.execute(&code) => {
            panic!("execute completed before cancel: {:?}", r);
        }
    };

    // Wait longer than the python sleep so a surviving child has time
    // to overwrite the marker.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let content =
        std::fs::read_to_string(&marker).expect("marker should exist: step 1 ran before cancel");
    assert_eq!(
        content, "alive",
        "subprocess survived the dropped future and overwrote the marker — \
         kill_on_drop(true) is missing on the Python Command, or tokio is \
         not delivering SIGKILL on Child drop"
    );
}

// ── the hard execution ceiling ─────────────────────────────────────────
//
// The synchronous python path must be bounded. Unbounded, a runaway loop
// spins at 100% CPU until someone kills the OS process by hand. Three sibling
// tool descriptions and the `running-python` knowhow all promise a 300s
// ceiling.
//
// Every test here injects a short ceiling via `with_execution_timeout`. The
// real one is 300s and a suite that waited it out is a suite nobody runs.

/// Longer than any injected ceiling in this file by a wide margin. A run that
/// returns quickly can only have been killed, never have finished.
const RUNAWAY_SLEEP_SECS: u64 = 60;

/// Create a runtime whose venv is already built, then shorten its ceiling.
/// Venv creation is deliberately NOT covered by the timeout, but on a cold
/// tempdir it takes seconds. A test measuring the ceiling must not measure
/// that instead.
async fn warmed_runtime(workspace: &std::path::Path, ceiling: Duration) -> PythonRuntime {
    let runtime = PythonRuntime::new(workspace.to_path_buf()).unwrap();
    runtime.execute("print('warmup')").await.expect("warmup");
    runtime.with_execution_timeout(ceiling)
}

/// The ceiling the rest of the system quotes. `llm/tools/exec.rs` states it in
/// three tool descriptions, and `system-knowhow/running-python.md` puts it in
/// the pick-your-tool table. A change here that leaves those alone puts the
/// docs back to describing a mechanism that does not exist.
#[test]
fn the_default_ceiling_is_the_300s_the_docs_promise() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        runtime.execution_timeout,
        Duration::from_secs(300),
        "run_python's sync ceiling is documented as 300s in llm/tools/exec.rs \
         and system-knowhow/running-python.md"
    );
    assert_eq!(
        EXECUTION_TIMEOUT_SECS,
        crate::llm::tools::MAX_TIMEOUT_SECS,
        "the python ceiling reads the one bash constant, it does not restate it"
    );
}

/// The headline property: a script that outruns the ceiling comes back as an
/// ordinary `Err`, promptly, instead of hanging the turn. The message names the
/// ceiling and points at the escape hatch. This error is the exact moment an
/// agent needs to learn `run_python_background` exists.
#[tokio::test]
async fn sync_execution_is_bounded_by_the_hard_ceiling() {
    let dir = tempdir().unwrap();
    let runtime = warmed_runtime(dir.path(), Duration::from_secs(1)).await;

    let started = std::time::Instant::now();
    let err = runtime
        .execute(&format!(
            "import time\ntime.sleep({RUNAWAY_SLEEP_SECS})\nprint('ran to completion')"
        ))
        .await
        .expect_err("a script that outruns the ceiling must fail");
    let elapsed = started.elapsed();

    assert!(
        err.contains("timed out after 1s"),
        "the error must name the ceiling it hit: {err}"
    );
    assert!(
        err.contains("run_python_background"),
        "the error must point at the escape hatch for longer work: {err}"
    );
    assert!(
        elapsed < Duration::from_secs(RUNAWAY_SLEEP_SECS / 2),
        "the ceiling did not cut the run short, it took {elapsed:?}"
    );
}

/// The property that matters most and is the likeliest to regress silently:
/// the OS child is really dead, not orphaned to keep burning CPU after the
/// engine has already told the agent the call failed. `kill_on_drop(true)` on
/// the Command is what delivers it, and only because `Command::output()` owns
/// the spawned Child inside the future the timeout drops.
///
/// The witness is a marker file. The script writes "alive" immediately, sleeps
/// past the ceiling, then writes "survived". A child that outlived the expiry
/// overwrites it; a killed one cannot. We read the marker after the script's
/// own sleep would have elapsed, so a survivor has had its chance.
///
/// Each write lands through `os.replace`, so the marker is only ever absent or
/// one of the two whole words. The plain `open(...).write(...)` this replaced
/// left the kill a window between creating the file and flushing it: a loaded
/// host read the empty file back and this test called the child a survivor.
#[tokio::test]
async fn the_execution_timeout_kills_the_python_child() {
    let dir = tempdir().unwrap();
    let runtime = warmed_runtime(dir.path(), Duration::from_secs(3)).await;

    let marker = dir.path().join("marker.txt");
    let marker_str = marker.to_string_lossy().replace('\\', "\\\\");
    let code = format!(
        "import os, time\n\
         def mark(word):\n\
        \x20   f = open('{m}.tmp', 'w')\n\
        \x20   f.write(word)\n\
        \x20   f.flush()\n\
        \x20   os.fsync(f.fileno())\n\
        \x20   f.close()\n\
        \x20   os.replace('{m}.tmp', '{m}')\n\
         mark('alive')\n\
         time.sleep(6)\n\
         mark('survived')",
        m = marker_str,
    );

    let err = runtime
        .execute(&code)
        .await
        .expect_err("the script sleeps past the ceiling");
    assert!(err.contains("timed out"), "got: {err}");

    // Past the script's own sleep, measured from its start: by now a surviving
    // child would have reached step 3.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let content = std::fs::read_to_string(&marker).expect(
        "marker should exist: the interpreter had a 3s ceiling to reach step 1. \
         Its absence means the child never started, so this run proves nothing",
    );
    assert_eq!(
        content, "alive",
        "the python child survived the expiry and kept running: kill_on_drop(true) \
         is missing from the Command, or the timeout is not dropping the future \
         that owns the Child"
    );
}

/// `run_python`'s staging invariant survives a timeout. A killed script has
/// written whatever it wrote into the staging tree, never into `data/`, so the
/// real artifact is byte-identical afterwards. This is why the incident left
/// the workspace untouched, and it must not become an accident.
#[tokio::test]
async fn a_timed_out_staged_run_leaves_data_untouched() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/report.csv"), "original").unwrap();
    let runtime = warmed_runtime(ws, Duration::from_secs(1)).await;

    let staging = ws.join(".lucidos/staging/timed-out-run");
    let err = runtime
        .execute_staged(
            &format!(
                "open('data/artifacts/report.csv', 'w').write('half written')\n\
                 import time\n\
                 time.sleep({RUNAWAY_SLEEP_SECS})"
            ),
            vec![],
            &staging,
        )
        .await
        .expect_err("the script sleeps past the ceiling");

    assert!(err.contains("timed out"), "got: {err}");
    assert_eq!(
        std::fs::read_to_string(ws.join("data/artifacts/report.csv")).unwrap(),
        "original",
        "a killed script must not have touched the real data/ file"
    );
}

/// An in-place script gets the same ceiling. An unbounded trigger script is the
/// same hazard as an unbounded `run_python`. The `.sh` branch of the same
/// `execute_script` dispatch has spent a 300s budget all along.
#[tokio::test]
async fn an_in_place_script_gets_the_ceiling_too() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = warmed_runtime(&ws, Duration::from_secs(1)).await;

    let script = write_trigger_script(
        &ws,
        "runaway-watch",
        &format!("import time\ntime.sleep({RUNAWAY_SLEEP_SECS})\n"),
    );

    let err = runtime
        .execute_file_with_env(&script, vec![])
        .await
        .expect_err("an in-place script must be bounded too");
    assert!(err.contains("timed out after 1s"), "got: {err}");
}

/// The wrapper must be invisible on the happy path: a script well inside the
/// ceiling still succeeds, still returns its stdout, and still stages its
/// `data/` write for the committer.
#[tokio::test]
async fn a_fast_script_still_succeeds_and_still_stages() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    let runtime = warmed_runtime(ws, Duration::from_secs(30)).await;

    let staging = ws.join(".lucidos/staging/fast-run");
    let out = runtime
        .execute_staged(
            "open('data/artifacts/report.csv', 'w').write('a,b\\n1,2'); print('done')",
            vec![],
            &staging,
        )
        .await
        .expect("a fast script must still succeed");

    assert_eq!(out.trim(), "done");
    assert!(
        staging.join("data/artifacts/report.csv").exists(),
        "the write must still be staged for the committer"
    );
    assert!(!ws.join("data/artifacts/report.csv").exists());
}

/// The run's exhaust dir explains itself after a timeout. Without the note it
/// holds neither `stdout.txt` nor `stderr.txt`, which looks exactly like a
/// spawn that never happened.
#[tokio::test]
async fn a_timeout_leaves_a_note_in_the_exhaust_dir() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = warmed_runtime(&ws, Duration::from_secs(1)).await;

    runtime
        .execute(&format!("import time\ntime.sleep({RUNAWAY_SLEEP_SECS})"))
        .await
        .expect_err("the script sleeps past the ceiling");

    let notes: Vec<String> = std::fs::read_dir(ws.join(".lucidos/exhaust"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| std::fs::read_to_string(e.path().join("stderr.txt")).ok())
        .filter(|s| s.contains("timed out"))
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "exactly one run dir should carry the timeout note, got: {notes:?}"
    );
    assert!(
        notes[0].starts_with("[lucidos]"),
        "the note must be marked engine-written, since every other line in \
         that file is the child's own stderr: {:?}",
        notes[0]
    );
}

// -----------------------------------------------------------------
// truncate_python_error, context-trim tests.
//
// A retried import failure returns a 30-line ModuleNotFoundError
// traceback every time. The signal is the exception line and the
// user frame, so the truncator must always preserve those two.
// -----------------------------------------------------------------

use super::truncate_python_error;

#[test]
fn short_error_returns_verbatim() {
    let err = "Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    a = 1/0\nZeroDivisionError: division by zero";
    assert_eq!(truncate_python_error(err), err);
}

#[test]
fn long_traceback_keeps_first_and_last_frame_plus_exception() {
    // 20-frame chain — comfortably above the 30-line budget so the
    // truncator engages. Models the kind of recursive descent into
    // an importer / framework that hides the one user frame the
    // LLM cares about.
    let mut tb = String::from("Traceback (most recent call last):\n");
    for i in 0..20 {
        tb.push_str(&format!(
            "  File \"/Users/x/.../mod_{i}.py\", line {line}, in func_{i}\n    helper_{i}()\n",
            i = i,
            line = 100 + i
        ));
    }
    tb.push_str("ModuleNotFoundError: No module named 'strategy_params'");

    let trimmed = truncate_python_error(&tb);
    // Header preserved.
    assert!(
        trimmed.starts_with("Traceback "),
        "header must be preserved: {trimmed:?}"
    );
    // First frame survives.
    assert!(
        trimmed.contains("mod_0.py"),
        "first frame must be preserved: {trimmed:?}"
    );
    assert!(
        trimmed.contains("helper_0()"),
        "first frame's code line must be preserved: {trimmed:?}"
    );
    // Last frame survives.
    assert!(
        trimmed.contains("mod_19.py"),
        "last frame must be preserved: {trimmed:?}"
    );
    assert!(
        trimmed.contains("helper_19()"),
        "last frame's code line must be preserved: {trimmed:?}"
    );
    // Middle frames dropped.
    assert!(
        !trimmed.contains("mod_10.py"),
        "middle frame should be dropped: {trimmed:?}"
    );
    // Omitted-marker present so the LLM knows trimming happened.
    assert!(
        trimmed.contains("frames omitted"),
        "trim marker required: {trimmed:?}"
    );
    // Exception line preserved — this is the ONE line the LLM
    // most needs to act on.
    assert!(
        trimmed.contains("ModuleNotFoundError: No module named 'strategy_params'"),
        "exception line must be preserved: {trimmed:?}"
    );
    // Result is much shorter than the original.
    assert!(
        trimmed.len() < tb.len() / 2,
        "trimmed should be < half the original ({} vs {})",
        trimmed.len(),
        tb.len()
    );
}

#[test]
fn short_traceback_under_budget_returns_verbatim() {
    // Three frames, under the 4-frame threshold, pass through untouched even
    // though the byte count crosses the line-count budget.
    let tb = "Traceback (most recent call last):\n\
              \x20 File \"a.py\", line 1, in <module>\n    f()\n\
              \x20 File \"b.py\", line 2, in f\n    g()\n\
              \x20 File \"c.py\", line 3, in g\n    raise RuntimeError('boom')\n\
              RuntimeError: boom";
    assert_eq!(truncate_python_error(tb), tb);
}

#[test]
fn pre_traceback_noise_is_tail_trimmed() {
    // A script that prints 50 progress lines then crashes. We
    // want the LAST few pre-crash lines (most informative) plus
    // the full traceback structure.
    let mut s = String::new();
    for i in 0..50 {
        s.push_str(&format!("progress: step {i}\n"));
    }
    s.push_str("Traceback (most recent call last):\n");
    for i in 0..6 {
        s.push_str(&format!(
            "  File \"/x/mod_{i}.py\", line {i}, in func\n    work()\n"
        ));
    }
    s.push_str("ValueError: bad value");

    let trimmed = truncate_python_error(&s);
    // Most pre-traceback noise dropped.
    assert!(
        trimmed.contains("lines of pre-traceback stderr omitted"),
        "pre-traceback trim marker required: {trimmed:?}"
    );
    // Some recent pre-traceback context kept.
    assert!(
        trimmed.contains("progress: step 49") || trimmed.contains("progress: step 48"),
        "must keep some recent progress lines: {trimmed:?}"
    );
    // First progress line gone.
    assert!(
        !trimmed.contains("progress: step 0\n"),
        "early progress should be dropped: {trimmed:?}"
    );
    // Traceback structure intact.
    assert!(trimmed.contains("Traceback "));
    assert!(trimmed.contains("ValueError: bad value"));
}

#[test]
fn pure_stderr_noise_without_traceback_keeps_tail() {
    // No "Traceback" line — just a flood of stderr prints (e.g.
    // a deprecation-warning loop). Keep the tail so the most
    // recent state is visible.
    let mut s = String::new();
    for i in 0..50 {
        s.push_str(&format!("warning {i}\n"));
    }
    let trimmed = truncate_python_error(&s);
    assert!(
        trimmed.contains("lines of stderr omitted"),
        "trim marker required: {trimmed:?}"
    );
    assert!(
        trimmed.contains("warning 49"),
        "tail must be preserved: {trimmed:?}"
    );
    assert!(
        !trimmed.contains("warning 0\n"),
        "head should be dropped: {trimmed:?}"
    );
}

#[test]
fn empty_stderr_returns_empty() {
    assert_eq!(truncate_python_error(""), "");
}

#[test]
fn exception_preserved_when_last_frame_has_no_source_line() {
    // Regression: frozen importlib._bootstrap, C-extension boundaries and
    // some re-raise paths land the exception line at exactly `last_file + 1`,
    // with no indented source line before it. The exception MUST still be
    // preserved: it is the single most actionable line.
    //
    // Use 40 frames so we exceed the 30-line budget and the truncator
    // engages. Each File line, no indented code line between frames.
    let mut tb = String::from("Traceback (most recent call last):\n");
    for i in 0..40 {
        tb.push_str(&format!(
            "  File \"<frozen importlib._bootstrap>\", line {line}, in func_{i}\n",
            i = i,
            line = 1000 + i
        ));
    }
    tb.push_str("ModuleNotFoundError: No module named 'strategy_params'");

    let trimmed = truncate_python_error(&tb);
    assert!(
        trimmed.contains("ModuleNotFoundError: No module named 'strategy_params'"),
        "exception MUST be preserved even when no source line follows the last frame: {trimmed:?}"
    );
    assert!(
        trimmed.contains("frozen importlib"),
        "frame line preserved: {trimmed:?}"
    );
    assert!(
        trimmed.contains("frames omitted"),
        "trim marker present: {trimmed:?}"
    );
}

#[test]
fn module_not_found_realistic_shape() {
    // Reproduces the shape observed in a live `dev`
    // thread: short-ish stderr but the import chain through
    // `_bootstrap`, `_find_and_load`, etc. fills the frame list.
    // Under the 4-frame threshold + under byte budget → return
    // verbatim. We want the truncator to NOT trim this case.
    let tb = "Traceback (most recent call last):\n\
              \x20 File \"/x/data/apps/habit-tracker/scripts/_smoke_abs.py\", line 5, in <module>\n\
              \x20   import big_candle_backtest as bcb\n\
              ModuleNotFoundError: No module named 'big_candle_backtest'";
    assert_eq!(truncate_python_error(tb), tb);
}

// ── agent-origin shim: auto-token forwarding ────────────────────────────
//
// A Lucidos agent that POSTs the engine over raw `urllib.request.urlopen`
// carries no agent-origin token. The resulting event then stamps
// `Api { mode: Human }` and the timeline renders it as "You".
//
// `install_agent_origin_shim` patches `http.client.HTTPConnection.request`, so
// any urllib, requests, urllib3 or http.client call to
// `localhost:LUCIDOS_API_PORT` auto-attaches the token and source thread
// headers. It is a `.pth`-loaded `_lucidos_agent_origin` module, NOT a
// `sitecustomize.py`, which Homebrew Python shadows. These tests pin the
// patched behaviour end-to-end against a real local TCP listener.

/// Read the raw HTTP request bytes off a localhost listener and return
/// the lowercased header set. Closes on the first request — enough to
/// inspect headers; we don't care about the response.
async fn capture_one_request_headers(
    listener: tokio::net::TcpListener,
) -> std::collections::HashSet<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut socket, _) = listener.accept().await.expect("accept");
    let mut buf = Vec::with_capacity(2048);
    // Read until headers terminator (CRLFCRLF). Cap at 8 KB to avoid
    // hanging if the client sends an unexpected stream.
    loop {
        let mut chunk = [0u8; 1024];
        let n = socket.read(&mut chunk).await.expect("read request bytes");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= 8192 {
            break;
        }
    }
    // Respond with a minimal 204 so the client doesn't hang in urlopen.
    let _ = socket
        .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
        .await;
    let _ = socket.shutdown().await;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .filter_map(|line| {
            let (k, _) = line.split_once(':')?;
            Some(k.trim().to_ascii_lowercase())
        })
        .collect()
}

/// With both LUCIDOS_AGENT_ORIGIN_TOKEN and LUCIDOS_API_PORT set, a raw
/// `urllib.request.urlopen(http://localhost:PORT/...)` from a Lucidos
/// subprocess automatically carries the agent-origin token and the spawning
/// thread id.
#[tokio::test]
async fn agent_origin_shim_forwards_token_on_urllib_request_to_engine_port() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    let thread_id = "11111111-2222-3333-4444-555555555555";
    let env = vec![
        (
            "LUCIDOS_AGENT_ORIGIN_TOKEN".to_string(),
            "test-token-xyz".to_string(),
        ),
        ("LUCIDOS_THREAD_ID".to_string(), thread_id.to_string()),
        ("LUCIDOS_API_PORT".to_string(), port.to_string()),
    ];

    let code = format!(
        r#"
import urllib.request
req = urllib.request.Request('http://127.0.0.1:{port}/api/v1/changes/abc/apply', method='POST', data=b'')
try:
    urllib.request.urlopen(req, timeout=5)
except Exception:
    pass
print('done')
"#,
        port = port,
    );

    let out = runtime.execute_with_env(&code, env).await.expect("execute");
    assert!(out.contains("done"), "python output was: {out}");

    let headers = capture.await.expect("capture task");
    assert!(
        headers.contains("x-lucidos-agent-origin-token"),
        "expected x-lucidos-agent-origin-token in captured headers, got: {headers:?}"
    );
    assert!(
        !headers.contains("x-lucidos-source-thread-id"),
        "the shim must send ONE origin header: the thread id rides inside the \
         token, and a separate claim of it is what the binding removed. Got: {headers:?}"
    );
}

/// Inert when the agent-origin token env var is missing. Pip installs to PyPI,
/// ad-hoc localhost calls, and any non-Lucidos use of this venv must not have
/// headers injected.
///
/// Test integrity: when `cargo test` runs from a Lucidos subprocess, the engine
/// has already stamped `LUCIDOS_AGENT_ORIGIN_TOKEN` and `LUCIDOS_API_PORT` into
/// the env. `tokio::process::Command::env` adds vars without clearing the
/// inherited set, so the python child would inherit them and the shim WOULD
/// install. The assertion would then pass only because the listener's random
/// port mismatches the inherited engine port. So pass explicit empty-string
/// overrides: the shim's gate (`if _TOKEN and _PORT:`) treats an empty string
/// as falsy and skips the patch.
#[tokio::test]
async fn agent_origin_shim_does_not_forward_token_when_env_missing() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    // Empty-string overrides clear any inherited engine env so the
    // shim's `if _TOKEN and _PORT:` gate is exercised honestly.
    let env = vec![
        ("LUCIDOS_AGENT_ORIGIN_TOKEN".to_string(), String::new()),
        ("LUCIDOS_API_PORT".to_string(), String::new()),
        ("LUCIDOS_THREAD_ID".to_string(), String::new()),
    ];

    let code = format!(
        r#"
import urllib.request
try:
    urllib.request.urlopen('http://127.0.0.1:{port}/x', timeout=5)
except Exception:
    pass
print('done')
"#,
        port = port,
    );

    runtime.execute_with_env(&code, env).await.expect("execute");

    let headers = capture.await.expect("capture task");
    assert!(
        !headers.contains("x-lucidos-agent-origin-token"),
        "token must not leak when env vars are empty, got: {headers:?}"
    );
}

/// Strict-port gate: a Lucidos subprocess calling a non-engine localhost
/// service must NOT receive the engine's token. Leaking a per-engine-startup
/// secret to arbitrary localhost listeners is bad hygiene, even though they
/// would ignore the header. Pin the gate so a future "any localhost"
/// relaxation lands here first.
#[tokio::test]
async fn agent_origin_shim_does_not_forward_token_on_non_engine_port() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let real_port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    // Tell the shim that the "engine" lives on a DIFFERENT port
    // (real_port + 1) than the one the request goes to. The patch must
    // see the host match but the port mismatch and skip header injection.
    let fake_engine_port = real_port.wrapping_add(1);
    let env = vec![
        (
            "LUCIDOS_AGENT_ORIGIN_TOKEN".to_string(),
            "test-token-xyz".to_string(),
        ),
        ("LUCIDOS_API_PORT".to_string(), fake_engine_port.to_string()),
    ];

    let code = format!(
        r#"
import urllib.request
try:
    urllib.request.urlopen('http://127.0.0.1:{port}/x', timeout=5)
except Exception:
    pass
print('done')
"#,
        port = real_port,
    );

    runtime.execute_with_env(&code, env).await.expect("execute");

    let headers = capture.await.expect("capture task");
    assert!(
        !headers.contains("x-lucidos-agent-origin-token"),
        "token must not leak to a non-engine localhost port, got: {headers:?}"
    );
}

/// Host-independent regression for the Homebrew-Python shadow bug. A
/// `sitecustomize.py` earlier on `sys.path` shadows the venv's own by
/// single-module-name resolution, and Homebrew ships one in its stdlib dir.
/// A `sitecustomize.py`-based shim therefore never runs. We load via a `.pth`
/// import instead, which `site` execs for EVERY `.pth` regardless of any
/// competing `sitecustomize`.
///
/// The test plants a no-op `sitecustomize.py` on `PYTHONPATH`, earlier than
/// the venv site-packages. That proves the shadow is active, and the token is
/// STILL forwarded. It is the durable reproduction, where the original test
/// only caught this on a host that shipped a shadowing `sitecustomize`.
#[tokio::test]
async fn agent_origin_shim_survives_shadowing_sitecustomize() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    // A competing sitecustomize, earlier on sys.path via PYTHONPATH. It writes
    // a marker so the test can prove the shadow actually loaded (otherwise a
    // green result would be meaningless).
    let shadow_dir = tempdir().unwrap();
    let marker = shadow_dir.path().join("shadow_ran.txt");
    let shadow_src = format!(
        "open({marker:?}, 'w').write('1')\n",
        marker = marker.to_string_lossy()
    );
    std::fs::write(shadow_dir.path().join("sitecustomize.py"), shadow_src).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    let env = vec![
        (
            "LUCIDOS_AGENT_ORIGIN_TOKEN".to_string(),
            "test-token-xyz".to_string(),
        ),
        (
            "LUCIDOS_THREAD_ID".to_string(),
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("LUCIDOS_API_PORT".to_string(), port.to_string()),
        (
            "PYTHONPATH".to_string(),
            shadow_dir.path().to_string_lossy().into_owned(),
        ),
    ];

    let code = format!(
        r#"
import urllib.request
req = urllib.request.Request('http://127.0.0.1:{port}/api/v1/changes/abc/apply', method='POST', data=b'')
try:
    urllib.request.urlopen(req, timeout=5)
except Exception:
    pass
print('done')
"#,
        port = port,
    );

    runtime.execute_with_env(&code, env).await.expect("execute");

    assert!(
        marker.exists(),
        "shadow sitecustomize did not load — PYTHONPATH precedence assumption is wrong, test proves nothing"
    );
    let headers = capture.await.expect("capture task");
    assert!(
        headers.contains("x-lucidos-agent-origin-token"),
        "token forwarding must survive a shadowing sitecustomize (the Homebrew bug), got: {headers:?}"
    );
}
