use super::*;
use crate::core::subprocess_origin::{ENV_AGENT_ORIGIN_TOKEN, HEADER_AGENT_ORIGIN_TOKEN};
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn failed_venv_creation_removes_only_the_environment_it_created() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let launcher = dir.path().join("python-fixture");
    fs::write(
        &launcher,
        r#"#!/bin/sh
mkdir -p "$3/bin"
touch "$3/bin/python"
echo 'The virtual environment was not created successfully because ensurepip is not available.'
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    for _ in 0..2 {
        let error = runtime
            .ensure_venv_with(tokio::process::Command::new(&launcher))
            .await
            .unwrap_err();
        assert!(error.contains("ensurepip is not available"), "{error}");
        assert!(error.contains("python3-venv"), "{error}");
        assert!(!runtime.venv_path.exists());
    }
}

#[tokio::test]
async fn failed_repair_keeps_the_existing_venv() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let launcher = dir.path().join("python-fixture");
    fs::write(
        &launcher,
        "#!/bin/sh\necho 'ensurepip is not available'\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
    let package = runtime.venv_path.join("lib/site-packages/user_pkg.py");
    fs::create_dir_all(package.parent().unwrap()).unwrap();
    fs::write(&package, "").unwrap();
    fs::create_dir_all(runtime.venv_path.join("bin")).unwrap();
    fs::write(runtime.python_bin(), "").unwrap();

    let error = runtime
        .ensure_venv_with(tokio::process::Command::new(&launcher))
        .await
        .unwrap_err();
    assert!(error.contains("ensurepip is not available"), "{error}");
    assert!(package.is_file(), "the failed repair kept user packages");
    assert!(runtime.python_bin().exists());
}

#[tokio::test]
async fn ensure_packages_refuses_flags_urls_and_paths() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
    for spec in [
        "--index-url=http://x",
        "-r",
        "git+https://evil.example/x.git",
        "x @ https://evil.example/x.whl",
        "./local-pkg",
        "/abs/pkg",
        "file:pkg",
        "evil-1.0.tar.gz",
        "evil-1.0-py3-none-any.whl",
        "",
    ] {
        let err = runtime
            .ensure_packages(
                &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
                &["pandas", spec],
            )
            .await
            .expect_err(spec);
        assert!(err.contains("is not a package name"), "{spec}: {err}");
    }
    assert!(!runtime.venv_path.exists(), "no venv, so no pip ran");
    for spec in [
        "pandas",
        "numpy>=1.26,<2",
        "requests[socks]",
        "pywin32; sys_platform == \"win32\"",
    ] {
        assert!(check_requirement_spec(spec).is_ok(), "{spec}");
    }
}

#[tokio::test]
async fn test_execute_simple_python() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "print('hello')",
        )
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().trim(), "hello");
}

#[tokio::test]
async fn test_execute_python_error() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "raise ValueError('test error')",
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("ValueError"));
}

#[tokio::test]
async fn test_execute_writes_to_workspace_cwd() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let result = runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('test_file.txt', 'w').write('ok'); print('done')",
        )
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
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "import os; print(os.path.exists('/etc'))",
        )
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_execute_writes_outside_workspace_succeed() {
    let outside = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let runtime = PythonRuntime::new(workspace.path().to_path_buf()).unwrap();

    let target = outside.path().join("outside.txt");
    let target_str = target.to_string_lossy().replace('\\', "\\\\");
    let code = format!("open('{}', 'w').write('hello'); print('done')", target_str);

    let result = runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
        )
        .await;
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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('data/artifacts/report.csv', 'w').write('a,b\\n1,2'); print('done')",
            vec![],
            &staging,
        )
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().trim(), "done");

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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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
        &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('scratch.txt', 'w').write('temp'); print('ok')",
            vec![],
            &staging,
        )
        .await;

    assert!(result.is_ok());
    assert!(ws.join("scratch.txt").exists());
}

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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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

    for sibling in ["database.json", "datasets/rows.csv", "data_backup/old.txt"] {
        assert!(
            ws.join(sibling).exists(),
            "{sibling} was diverted into staging and discarded"
        );
    }

    assert!(
        !ws.join("data/real.txt").exists(),
        "a real data/ write must stage, not land on the host before commit"
    );
    assert!(staging.join("data/real.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_data_root_still_stages_its_writes() {
    let dir = tempdir().unwrap();
    let elsewhere = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(elsewhere.path().join("artifacts")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), ws.join("data")).unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

    let staging = ws.join(".lucidos/staging/relocated-data-run");
    let out = runtime
        .execute_staged(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "open('data/artifacts/report.csv', 'w').write('a,b\\n')\n\
             print(open('data/artifacts/report.csv').read().strip())",
            vec![],
            &staging,
        )
        .await
        .expect("the write must succeed");
    assert_eq!(
        out.trim(),
        "a,b",
        "the read must see this run's staged bytes"
    );

    assert_eq!(
        std::fs::read_to_string(staging.join("data/artifacts/report.csv")).unwrap(),
        "a,b\n",
        "the staged tree is the only thing the committer publishes"
    );
    assert!(
        !elsewhere.path().join("artifacts/report.csv").exists(),
        "a relocated data/ write must stage, not land on the host before commit"
    );
}

fn write_trigger_script(ws: &std::path::Path, slug: &str, body: &str) -> std::path::PathBuf {
    let scripts_dir = ws.join("data/triggers").join(slug).join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    let script = scripts_dir.join("run.py");
    std::fs::write(&script, body).unwrap();
    script
}

#[tokio::test]
async fn execute_file_runs_the_real_path_not_a_copy() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "notary-verdict-watch",
        "import os\nprint(os.path.abspath(__file__))\n",
    );

    let out = runtime
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
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

#[tokio::test]
async fn execute_file_resolves_sibling_dir_via_file() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let state_dir = ws.join("data/triggers/notary-verdict-watch/state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("marker.json"),
        r#"{"approved_version": "0.0.0-fixture"}"#,
    )
    .unwrap();

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
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
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
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
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

#[tokio::test]
async fn execute_file_shapes_errors_like_run_script() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(&ws, "boom", "raise ValueError('kaboom')\n");

    let err = runtime
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
        .await
        .expect_err("script raises");
    assert!(err.starts_with("Python error:"), "got: {err}");
    assert!(err.contains("ValueError: kaboom"), "got: {err}");
}

#[tokio::test]
async fn a_failure_with_empty_stderr_reports_the_end_of_stdout() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "logs-to-stdout",
        "import sys\nprint('fetching')\nprint('HTTP 500 from the provider')\nsys.exit(1)\n",
    );

    let err = runtime
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
        .await
        .expect_err("script exits 1");
    assert!(err.contains("end of stdout"), "got: {err}");
    assert!(
        err.trim_end().ends_with("HTTP 500 from the provider"),
        "got: {err}"
    );
}

#[tokio::test]
async fn a_long_stdout_log_reports_its_end_under_the_label() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let script = write_trigger_script(
        &ws,
        "long-stdout-log",
        "import sys\nfor i in range(10000):\n    print('progress line', i, 'x' * 60)\n\
         print('HTTP 500 from the provider')\nsys.exit(1)\n",
    );

    let err = runtime
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
        .await
        .expect_err("script exits 1");
    assert!(err.contains("this is the end of stdout"), "got: {err}");
    assert!(err.contains("bytes of stdout omitted"), "got: {err}");
    assert!(!err.contains("stderr omitted"), "got: {err}");
    assert!(
        err.trim_end().ends_with("HTTP 500 from the provider"),
        "got: {err}"
    );
}

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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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

#[tokio::test]
async fn execute_with_env_still_runs_from_the_exhaust_copy() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = PythonRuntime::new(ws.clone()).unwrap();

    let out = runtime
        .execute_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "import os\nprint(os.path.abspath(__file__))\n",
            vec![],
        )
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

#[tokio::test]
async fn execute_kills_subprocess_when_future_dropped() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

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
         time.sleep(2)\n\
         mark('survived')",
        m = marker_str,
    );

    runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "print('warmup')",
        )
        .await
        .expect("warmup");

    let token = tokio_util::sync::CancellationToken::new();
    let token_clone = token.clone();
    let marker_watch = marker.clone();
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !marker_watch.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        token_clone.cancel();
    });

    let sandbox = crate::runtime::agent_sandbox::Sandbox::unsandboxed();
    tokio::select! {
        _ = token.cancelled() => {}
        r = runtime.execute(&sandbox, &code) => {
            panic!("execute completed before cancel: {:?}", r);
        }
    };

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

const RUNAWAY_SLEEP_SECS: u64 = 60;

async fn warmed_runtime(workspace: &std::path::Path, ceiling: Duration) -> PythonRuntime {
    let runtime = PythonRuntime::new(workspace.to_path_buf()).unwrap();
    runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "print('warmup')",
        )
        .await
        .expect("warmup");
    runtime.with_execution_timeout(ceiling)
}

#[tokio::test]
async fn sync_execution_is_bounded_by_the_hard_ceiling() {
    let dir = tempdir().unwrap();
    let runtime = warmed_runtime(dir.path(), Duration::from_secs(1)).await;

    let started = std::time::Instant::now();
    let err = runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &format!("import time\ntime.sleep({RUNAWAY_SLEEP_SECS})\nprint('ran to completion')"),
        )
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
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
        )
        .await
        .expect_err("the script sleeps past the ceiling");
    assert!(err.contains("timed out"), "got: {err}");

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
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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
        .execute_file_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &script,
            vec![],
        )
        .await
        .expect_err("an in-place script must be bounded too");
    assert!(err.contains("timed out after 1s"), "got: {err}");
}

#[tokio::test]
async fn a_fast_script_still_succeeds_and_still_stages() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    let runtime = warmed_runtime(ws, Duration::from_secs(30)).await;

    let staging = ws.join(".lucidos/staging/fast-run");
    let out = runtime
        .execute_staged(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
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

#[tokio::test]
async fn a_timeout_leaves_a_note_in_the_exhaust_dir() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let runtime = warmed_runtime(&ws, Duration::from_secs(1)).await;

    runtime
        .execute(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &format!("import time\ntime.sleep({RUNAWAY_SLEEP_SECS})"),
        )
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

fn backdate(path: &std::path::Path, age: Duration) {
    let when = std::time::SystemTime::now() - age;
    std::fs::File::open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
}

fn spent_run_dir(exhaust: &std::path::Path) -> std::path::PathBuf {
    let run_dir = exhaust.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("stderr.txt"), "boom\n").unwrap();
    run_dir
}

#[test]
fn engine_start_drops_run_dirs_past_the_retention_window() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let exhaust = ws.join(".lucidos/exhaust");
    std::fs::create_dir_all(&exhaust).unwrap();

    let hour = Duration::from_secs(3600);
    let stale = spent_run_dir(&exhaust);
    backdate(&stale, EXHAUST_RETENTION + hour);
    let recent = spent_run_dir(&exhaust);
    backdate(&recent, EXHAUST_RETENTION - hour);

    PythonRuntime::new(ws).unwrap();

    assert!(
        !stale.exists(),
        "a run dir past the window must be reclaimed: {}",
        stale.display()
    );
    assert_eq!(
        std::fs::read_to_string(recent.join("stderr.txt")).unwrap(),
        "boom\n",
        "a run dir inside the window is still the documented debugging affordance"
    );
}

#[cfg(unix)]
#[test]
fn the_sweep_spares_every_entry_it_cannot_name_as_a_run_dir() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();
    let exhaust = ws.join(".lucidos/exhaust");
    std::fs::create_dir_all(&exhaust).unwrap();

    let target = ws.join("keep");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("precious.txt"), "mine\n").unwrap();

    let note = exhaust.join("README.txt");
    std::fs::write(&note, "hand-written\n").unwrap();
    let odd_name = exhaust.join("not-a-run-id");
    std::fs::create_dir_all(&odd_name).unwrap();
    let link = exhaust.join(uuid::Uuid::new_v4().to_string());
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let ancient = EXHAUST_RETENTION + Duration::from_secs(3600);
    for path in [&note, &odd_name, &target] {
        backdate(path, ancient);
    }

    PythonRuntime::new(ws).unwrap();

    assert!(note.exists(), "a loose file is not a run dir");
    assert!(
        odd_name.exists(),
        "a dir the engine never minted is not one"
    );
    assert!(
        link.symlink_metadata().is_ok(),
        "a symlink is not a run dir, whatever it points at"
    );
    assert!(
        target.join("precious.txt").exists(),
        "following that link would have deleted a tree outside the sink"
    );
}

async fn capture_one_request_headers(
    listener: tokio::net::TcpListener,
) -> std::collections::HashSet<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut socket, _) = listener.accept().await.expect("accept");
    let mut buf = Vec::with_capacity(2048);
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
            ENV_AGENT_ORIGIN_TOKEN.to_string(),
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

    let out = runtime
        .execute_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
            env,
        )
        .await
        .expect("execute");
    assert!(out.contains("done"), "python output was: {out}");

    let headers = capture.await.expect("capture task");
    assert!(
        headers.contains(HEADER_AGENT_ORIGIN_TOKEN),
        "expected {HEADER_AGENT_ORIGIN_TOKEN} in captured headers, got: {headers:?}"
    );
    assert!(
        !headers.contains("x-lucidos-source-thread-id"),
        "the shim must send ONE origin header: the thread id rides inside the \
         token, and a separate claim of it is what the binding removed. Got: {headers:?}"
    );
}

#[tokio::test]
async fn agent_origin_shim_does_not_forward_token_when_env_missing() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    let env = vec![
        (ENV_AGENT_ORIGIN_TOKEN.to_string(), String::new()),
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

    runtime
        .execute_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
            env,
        )
        .await
        .expect("execute");

    let headers = capture.await.expect("capture task");
    assert!(
        !headers.contains(HEADER_AGENT_ORIGIN_TOKEN),
        "token must not leak when env vars are empty, got: {headers:?}"
    );
}

#[tokio::test]
async fn agent_origin_shim_does_not_forward_token_on_non_engine_port() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let real_port = listener.local_addr().unwrap().port();
    let capture = tokio::spawn(capture_one_request_headers(listener));

    let fake_engine_port = real_port.wrapping_add(1);
    let env = vec![
        (
            ENV_AGENT_ORIGIN_TOKEN.to_string(),
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

    runtime
        .execute_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
            env,
        )
        .await
        .expect("execute");

    let headers = capture.await.expect("capture task");
    assert!(
        !headers.contains(HEADER_AGENT_ORIGIN_TOKEN),
        "token must not leak to a non-engine localhost port, got: {headers:?}"
    );
}

#[tokio::test]
async fn agent_origin_shim_survives_shadowing_sitecustomize() {
    let dir = tempdir().unwrap();
    let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();

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
            ENV_AGENT_ORIGIN_TOKEN.to_string(),
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

    runtime
        .execute_with_env(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            &code,
            env,
        )
        .await
        .expect("execute");

    assert!(
        marker.exists(),
        "shadow sitecustomize did not load — PYTHONPATH precedence assumption is wrong, test proves nothing"
    );
    let headers = capture.await.expect("capture task");
    assert!(
        headers.contains(HEADER_AGENT_ORIGIN_TOKEN),
        "token forwarding must survive a shadowing sitecustomize (the Homebrew bug), got: {headers:?}"
    );
}

#[test]
fn agent_origin_shim_fills_every_placeholder() {
    let shim = agent_origin_shim_py();
    assert!(
        !shim.contains("{ENV_") && !shim.contains("{HEADER_"),
        "{shim}"
    );
    assert!(shim.contains(ENV_AGENT_ORIGIN_TOKEN));
    assert!(shim.contains(HEADER_AGENT_ORIGIN_TOKEN));
}

mod guard_tests {
    use super::*;

    #[test]
    fn the_default_ceiling_is_the_one_the_docs_promise() {
        let dir = tempdir().unwrap();
        let runtime = PythonRuntime::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            runtime.execution_timeout,
            Duration::from_secs(EXECUTION_TIMEOUT_SECS)
        );
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let doc = crate::core::knowhow::doc_family_text(&repo, "system-knowhow/running-python.md");
        for quoted in [
            format!("{EXECUTION_TIMEOUT_SECS} s ceiling"),
            format!("timed out after {EXECUTION_TIMEOUT_SECS}s"),
        ] {
            assert!(
                doc.contains(&quoted),
                "system-knowhow/running-python.md must quote the ceiling as {quoted:?}"
            );
        }
        assert_eq!(
            EXECUTION_TIMEOUT_SECS,
            crate::llm::tools::MAX_TIMEOUT_SECS,
            "the python ceiling reads the one bash constant, it does not restate it"
        );
    }
}
