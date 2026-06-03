---
name: Running Python
description: How to use run_python, run_python_background, and bash_output — picking the right one, the venv layout, importing from app scripts, and the antipatterns to avoid (sleep-poll, sys.path thrashing). Load this BEFORE writing run_python / run_python_background code if you haven't already this thread.
---

# Running Python

How the chat agent runs Python in a workspace, and which mistakes are silent context burners. Load this when you're about to reach for `run_python` or `run_python_background` and you haven't already loaded it earlier in the thread.

## Pick the right tool

| Tool | When | Sync ceiling |
|------|------|--------------|
| `run_python` | Quick scripts: data prep, plotting, file conversion, one-off transforms. Returns stdout synchronously when the script finishes. | 300 s |
| `run_python_background` | Anything that may run longer than ~30 s — backtests, model training, large data sweeps, batch downloads. Returns a `task_id` immediately; drain with `bash_output(task_id, wait_secs=…)`. | watchdog (default 1 h, max 12 h) |
| `bash_output(task_id, wait_secs?)` | The drain tool for the two above AND for `run_bash_background`. Pass `wait_secs` to BLOCK server-side until output arrives or the task finishes. | up to 120 s per call |
| `bash_kill(task_id)` | Cancel a running background task. No-op if already finished. | — |

Decision rule: if you'd reach for `time.sleep` inside `run_python`, you almost certainly want `run_python_background` plus a `bash_output(task_id, wait_secs=N)` instead. The single most common context-waster in chat threads is hand-rolling polling loops.

## The drain pattern — never poll with `time.sleep`

WRONG (burns two tool calls per wait, doubles context, stalls the turn):

```python
# Tool call 1: spawn
run_python_background(code="result = expensive_thing(); print(result)")
# → task_id = "abc"

# Tool call 2: sleep-poll  ← ANTIPATTERN
run_python(code="import time; time.sleep(120); print('waited')")

# Tool call 3: drain
bash_output(task_id="abc")
```

RIGHT (one drain call does the wait AND the read):

```python
# Tool call 1: spawn
run_python_background(code="result = expensive_thing(); print(result)")
# → task_id = "abc"

# Tool call 2: drain with server-side wait — returns the moment
# new output arrives OR the task finishes OR 60 s pass.
bash_output(task_id="abc", wait_secs=60)
# → { stdout: "...", finished: true, exit_code: 0 }
```

`wait_secs` semantics:
- Up to 120 s per call (engine clamps higher values silently).
- Returns immediately if there's already buffered output OR the task is already finished.
- Returns whatever is buffered on timeout with `finished: false` — the LLM can decide whether to call again.
- Use 30–60 s for typical long-running scripts; use 0 (or omit) for a quick liveness check between other actions.

If `finished: true`, STOP polling. Subsequent calls fall back to the event store and re-return the full final stdout/stderr each time — wasted context.

When the wait is genuinely unbounded — a 30-minute backtest, a sweep that might run all night, a download you can't size up front — and you've already burned a couple of `wait_secs=120` drains with no end in sight, **don't end your turn with "I'll report when it finishes"**. The engine has no way to wake you back up; the user would have to type something to drag you back. Instead, call `ask_user_question` with one option whose label is the user-perspective wake prompt — e.g. `options: ["Show results"]`, `options: ["Stop sweep"]`, `options: ["Drain now"]`. The hint/context goes in the `question` text. This is a *wake question* (see glossary): it parks the thread with a "?" status, and the user taps when they're ready, which feeds the option string back as your next signal and lets you resume.

## The per-workspace venv

Lucidos provisions one Python venv per workspace at `.lucidos/runtime/python/venv/`. The `run_python*` tools run scripts inside it automatically — you don't activate it, don't reference it, don't reach for `subprocess.run(["python", ...])`. Just write Python and pass it as the `code` arg.

- **Packages**: declare in the tool call's `packages: ["numpy", "pandas", ...]` arg. They're installed into the workspace venv before the script runs; already-installed packages are no-ops. Don't `pip install` from inside `code`.
- **Working directory**: scripts execute with cwd = workspace root. So `open("data/artifacts/foo.csv")` is correct; `open("/Users/.../data/...")` is brittle.
- **Env vars**: `LUCIDOS_WORKSPACE` (workspace root, absolute), `CRED_*` for credentials, `OAUTH_*_ACCESS_TOKEN` for connected OAuth accounts — all auto-injected.

## Importing from `data/apps/<x>/scripts/`

Apps that ship Python helpers put them under `data/apps/<app-id>/scripts/`. The venv has no PYTHONPATH for these — you must add the directory yourself, using `$LUCIDOS_WORKSPACE` so the path survives any future cwd change:

```python
import os, sys
sys.path.insert(0, os.path.join(os.environ["LUCIDOS_WORKSPACE"], "data/apps/momentum/scripts"))

import strategy_params      # now resolves
import big_candle_backtest  # now resolves
```

Don't `os.chdir` into the scripts dir as a substitute — relative paths inside the imported modules then resolve from there instead of the workspace root, which breaks any `open("data/...")` inside them.

## Anti-patterns that burn context

These all look reasonable in isolation; they fail the same way every time and waste a turn each:

1. **Sleep-poll**: `run_python(code="time.sleep(N)")` to wait for a background task. Use `bash_output(task_id, wait_secs=N)` instead. The engine's repeated-call guard buckets `run_python` calls by the first non-blank non-comment non-import line of `code` (truncated to 80 chars) — so three calls with the SAME first actionable line in a row (e.g. `time.sleep(60)` thrice) trip the guard. Escalating arguments (`time.sleep(60)` then `time.sleep(120)` then `time.sleep(180)`) each bucket separately and do NOT trip — but they still burn three turns of context, so use `wait_secs` regardless.
2. **`os.chdir` + `sys.path.insert(0, ".")`** for importing app scripts. Use the absolute path via `$LUCIDOS_WORKSPACE` once — don't try four variants.
3. **`subprocess.run(["python", "-c", code])`** from inside `run_python` to get a "fresh interpreter". You already are one. The subprocess won't see the venv's site-packages.
4. **Re-spawning a background task to read its result** instead of calling `bash_output(task_id)`. The task is still running with a known id; drain it.
5. **Polling `bash_output` after `finished: true`**. Each subsequent call replays the full final stdout/stderr from the event store, which is exactly the context bloat the drain semantics exist to avoid.

## Errors

`run_python` (the sync foreground tool) auto-trims long tracebacks before returning: keeps the first and last `File "..."` frame and the final `ExceptionClass: message` line, drops the middle. The full traceback is on disk at `.lucidos/exhaust/<run_id>/stderr.txt` for debugging.

`run_python_background` does NOT auto-trim — its stderr flows through `bash_output` raw, which is fine for short outputs but means a verbose chained import error can dump many KB of frames into your context. When you spawn a backtest / long script you expect MIGHT crash with a multi-thousand-line traceback, wrap the body in your own `try / except` and re-raise a short fingerprint: `print(f"FAIL: {type(e).__name__}: {e}", file=sys.stderr); raise`. The full traceback still lands on disk at `.lucidos/exhaust/<task_id>/stderr.txt`.

The trimmed (or short) form is enough to act on — diagnose the exception class + the user-frame line, fix the script, retry once. Don't retry with sys.path / chdir variants in the hope that one sticks; check what's importable first.
