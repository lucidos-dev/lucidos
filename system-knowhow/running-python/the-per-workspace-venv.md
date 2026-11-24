---
name: Running Python, The per-workspace venv
description: Part of Running Python. Its index lists the sections it covers.
---

One part of `system-knowhow/running-python`, which lists every part.

## The per-workspace venv

On macOS, the sandbox keeps ancestor metadata readable so framework Python
can resolve the venv inside a gateway workspace. Hidden contents stay denied.
See `agent-sandbox` for the boundary and workspace-scoped opt-out.

Lucidos provisions one Python venv per workspace at `.lucidos/runtime/python/venv/`. The `run_python*` tools run scripts inside it automatically. Don't activate it, reference it, or reach for `subprocess.run(["python", ...])`. Pass plain Python as the `code` arg.

If setup reports that ensurepip is unavailable on Debian or Ubuntu, have the
workspace operator install `python3-venv` for the host interpreter. Lucidos
repairs the environment in place on the next call, keeping installed packages.

- **Packages**: declare in the tool call's `packages: ["numpy", "pandas", ...]` arg. They're installed into the workspace venv before the script runs; already-installed packages are no-ops. Give PyPI names with an optional version (`numpy>=1.26`, `requests[socks]`). A URL, a path, an archive file or a pip flag is refused. A call with packages goes to the guard's judge, because installing one runs its build code. Don't `pip install` from inside `code`.
- **Working directory**: scripts execute with cwd = workspace root. So `open("data/artifacts/foo.csv")` is correct; `open("/Users/.../data/...")` is brittle.
- **Env vars**: `LUCIDOS_WORKSPACE` (workspace root, absolute) is always set. A `CRED_*` credential or an `OAUTH_*_ACCESS_TOKEN` is set only when `code`, or a workspace script it runs, names it: write `os.environ["CRED_GITHUB"]`, not a name built at run time. See `system-knowhow/best-practices/workspace-layout.md` § Credentials in subprocesses.

## Importing from `data/apps/<x>/scripts/`

Apps that ship Python helpers put them under `data/apps/<app-id>/scripts/`. The venv has no PYTHONPATH for these. Add the directory yourself, using `$LUCIDOS_WORKSPACE` so the path survives a cwd change:

```python
import os, sys
sys.path.insert(0, os.path.join(os.environ["LUCIDOS_WORKSPACE"], "data/apps/habit-tracker/scripts"))

import strategy_params      # now resolves
import big_candle_backtest  # now resolves
```

Don't `os.chdir` into the scripts dir instead. Relative paths inside the imported modules then resolve from there, which breaks any `open("data/...")` inside them.

## Anti-patterns that burn context

Each looks reasonable alone, fails the same way every time, and wastes a turn:

1. **Sleep-poll**: `run_python(code="time.sleep(N)")` to wait for a background task.
   Use `bash_output(task_id, wait_secs=N)` instead.
   The repeated-call guard covers both Python tools and groups calls by their
   first actionable code line. After two failed executions, identical attempts
   three and four warn without executing; attempt five forces a break.
   A success or a different call resets the streak.
2. **`os.chdir` + `sys.path.insert(0, ".")`** for importing app scripts. Use the absolute path via `$LUCIDOS_WORKSPACE` once. Don't try four variants.
3. **`subprocess.run(["python", "-c", code])`** from inside `run_python` to get a "fresh interpreter". You already are one. The subprocess won't see the venv's site-packages.
4. **Re-spawning a background task to read its result** instead of calling `bash_output(task_id)`. The task is still running with a known id; drain it.
5. **Polling `bash_output` after `finished: true`**. Nothing new can arrive (see § The drain pattern).
6. **Draining a long task to the end of the turn rather than ending the turn.** See "You do not have to sit through it" above.
7. **Spawning a shell loop to watch something** (`for i in $(seq 1 200); do … sleep 60; done`). It cannot re-open a thread, it burns a process for an hour, and it dies unread with your turn.
8. **Detaching with `&` or `nohup` from `run_bash`.** `&` binds looser than `&&`. So `cd x && thing & echo ok` backgrounds the whole chain as one subshell, which keeps holding the tool's output pipes. You get the shell's real exit status and a note that a detached process survived, then nothing more. The process runs on with no task id, no watchdog and no completion event. Use `run_bash_background` instead, and drain it with `bash_output(task_id, wait_secs=N)`.

## Errors

`run_python` (the sync foreground tool) auto-trims long tracebacks. It keeps the first and last `File "..."` frame and the final `ExceptionClass: message` line, and drops the middle. Then it caps the error at its last 4 KB, behind a `[... N bytes of stderr omitted ...]` marker, so one huge line cannot flood your context. The stderr tail stays on disk at `.lucidos/exhaust/<run_id>/stderr.txt` for seven days. Many scripts log errors to stdout. So when stderr is empty, the error shows the last 4 KB of stdout, under a label.

`run_python_background` does not auto-trim. Its stderr flows through `bash_output`
raw, so chained import errors can fill the context.
For a long script, catch exceptions and print a short failure summary.
The engine stages `script.py` under `.lucidos/exhaust/<run_id>/`; that run id
differs from the background task id. It does not write `stderr.txt` there.
Save a full traceback explicitly when the work needs one.

A `run_python` call that hits the 300 s ceiling fails with `Python script
timed out after 300s`. The code was fine; the budget was not. Never retry it
as-is: move it to `run_python_background`, or cut the work down.

The trimmed (or short) form is enough to act on: diagnose the exception class and the user-frame line, fix the script, retry once. Don't cycle through sys.path / chdir variants; check what's importable first.

## Diagnostic evidence

Legacy engine log lines for tool execution remain private in maintainer diagnostics.
Tool output is not automatically classified as shareable metadata.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
