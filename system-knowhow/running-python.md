---
name: Running Python
description: How to use run_python, run_python_background and bash_output: picking the right one, the venv layout, importing from app scripts, and the antipatterns (sleep-poll, sys.path thrashing). Load BEFORE writing run_python code if you haven't already this thread.
---

# Running Python

How the chat agent runs Python in a workspace, and which mistakes silently burn context.

## Pick the right tool

| Tool | When | Ceiling |
|------|------|---------|
| `run_python` | Quick scripts: data prep, plotting, file conversion, one-off transforms. Returns stdout synchronously when the script finishes: the first 100 KB, then a `[truncated: N bytes total]` marker. Write bigger output to a file. | 300 s hard, not adjustable |
| `run_python_background` | Anything that may run longer than ~30 s: backtests, model training, large data sweeps, batch downloads. Returns a `task_id` immediately; drain with `bash_output(task_id, wait_secs=…)`. | watchdog `timeout_secs` (default 600 s, max 3600 s) |
| `bash_output(task_id, wait_secs?)` | The drain tool for the two above AND for `run_bash_background`. Pass `wait_secs` to BLOCK server-side for that many seconds, or until the task finishes. | up to 120 s per call |
| `bash_kill(task_id)` | Cancel a running background task. Its completion does not re-open your thread: the stop stands down your own waits on it first. No-op if already finished. | n/a |

Decision rule: if you'd reach for `time.sleep` inside `run_python`, use `run_python_background` plus `bash_output(task_id, wait_secs=N)` instead. Hand-rolled polling loops are the most common context waster in chat threads.

`run_python`'s 300 s ceiling is enforced. At it the interpreter is SIGKILLed,
and the call returns a failed tool result naming the ceiling. `run_python` has
no `timeout_secs` to raise. So a script you can't confidently size belongs in
`run_python_background` from the start. A killed run commits nothing: its
`data/` writes were only staged, so you lose the work but never half of it.

## The drain pattern: never poll with `time.sleep`

WRONG (burns two tool calls per wait, doubles context, stalls the turn):

```python
# Tool call 1: spawn
run_python_background(code="result = expensive_thing(); print(result)",
                      description="the expensive calculation")
# → task_id = "abc"

# Tool call 2: sleep-poll  ← ANTIPATTERN
run_python(code="import time; time.sleep(120); print('waited')")

# Tool call 3: drain
bash_output(task_id="abc")
```

RIGHT (one drain call does the wait AND the read):

```python
# Tool call 1: spawn
run_python_background(code="result = expensive_thing(); print(result)",
                      description="the expensive calculation")
# → task_id = "abc"

# Tool call 2: drain with server-side wait. Blocks the full 60 s,
# or returns early the moment the task finishes.
bash_output(task_id="abc", wait_secs=60)
# → { stdout: "...", finished: true, exit_code: 0, signal: null,
#     status: "exit code 0", elapsed_secs: 58, waited_secs: 58 }
```

`wait_secs` semantics:
- Up to 120 s per call (engine clamps higher values silently).
- Blocks for the **full** duration. Only two things end a wait early: the task finishing, and a user message. Read the message, answer it, then drain again.
- **New output does not wake you.** Chatty tasks (a cargo build, `notarytool`, an npm install) emit every few hundred milliseconds. Waking on output would turn "wait two minutes" into a poll every two seconds.
- On timeout it returns whatever accumulated, with `finished: false`. Decide whether to call again.
- Use the full 120 s for anything long you're following; 30–60 s when you expect it to finish soon; 0 (or omit) for a quick liveness check between other actions.

The engine keeps a completed task drainable for a few minutes. So a drain that
lands as the task completes still returns the final tail with `finished: true`.

If `finished: true`, STOP polling. Nothing new can arrive. Inside that
few-minute window a repeat call returns an empty window. After it, calls fall
back to the event store and re-return the full final stdout/stderr each time.
Either way it wastes context.

### You do not have to sit through it: ending the turn is a valid wait

A background task **outlives your turn**. When you end a turn with one still
running, the engine subscribes the thread to that task's
`BackgroundBashCompleted`. It re-opens the thread with a new turn the moment
the event lands, and you drain the result then. The thread's indicator shows
the subscription to you and the user, so they can tell a sleeping thread from
a stalled one.

So for anything long (a release build, a full test suite, a notarization):
spawn it, drain once or twice to confirm it started cleanly, then **report
where things stand and end the turn**. Twenty 120-second drains spend twenty
turns of context to learn what one wake tells you for free.

Never promise to "check back later". You do not run between turns, and only a
subscription re-opens the thread. Use `await_event` to watch something *other*
than the task's completion. The completion subscription is armed for you.

### A restart ends the task, and you will be told so

A background task is a child of the engine process. An engine restart, crash
or OOM ends it mid-flight, and nothing in the thread can prevent that.

You are guaranteed to hear. The engine records the task's
`BackgroundBashCompleted` itself, with `abandoned: true`, no `exit_code` and
no `signal`. Your subscription resolves on the next boot rather than waiting
out its deadline.

**The two ways it ends differ, and the `stderr` line says which.** An orderly
shutdown kills the task, then records it, so the output is everything the task
wrote. A crash records only that the loss happened, and no output survives.

**Neither promises the work stopped.** A crash kills nothing. An orderly
shutdown kills the task's process group, but not a process that detached into
its own session. Either can leave a child running under init. How to report
it: see `abandoned` in the next section.

### Never estimate elapsed time: read it

Every drain reports two clocks, and they are the only ones you have:

| Field | Meaning |
|---|---|
| `elapsed_secs` | How long the task has been running, or its total runtime once `finished` (frozen at completion). `null` only when the result came from an old `BackgroundBashCompleted` record written before the engine stored the timestamp pair: an honest "unknown" rather than a fabricated `0`. |
| `waited_secs` | How long **this one call** actually blocked. Well short of your `wait_secs`, with `finished: false`? The user sent a message, which cuts the wait so you can answer. Nothing is broken. |

Never infer elapsed time from how long you *asked* to wait, or from how many
drains you've made. One agent did, and narrated "roughly 20 minutes in Apple's
queue" ninety seconds into a release. Quote `elapsed_secs`, or say nothing
about timing.

### Oversized windows keep the tail

A drain is capped at 100 KB per stream. When a window exceeds it, you get the
**most recent** bytes behind a leading marker. It opens `[truncated`, then a
dash, then `N earlier bytes dropped, showing the most recent M of T total]`
verbatim. So the
failure at the end of a build log is never dropped.

`N` is the whole gap, not only the part the drain trimmed. A task that emits
more than ~2 MB on one stream between two drains outruns the engine's buffer,
and `N` counts those bytes too. So a chatty task drained on a long `wait_secs`
can lose middle output, and the marker tells you. If you need all of it, have
the task `tee` to a file and `read_file` that.

## Deciding whether a background task succeeded

`bash_output` returns three status fields. **Read `status`**: it is the
one-line human phrase, the same one the completion summary uses, so the two
never disagree.

| Field | Meaning |
|---|---|
| `exit_code` | The normal exit status, and **only** that. `null` whenever there wasn't one. |
| `signal` | The Unix signal that killed the **shell Lucidos spawned**, if one did. A watchdog timeout or `bash_kill` gives `15` (SIGTERM), or `9` if the shell ignored SIGTERM through the 3 s grace. `null` otherwise, including when a signal killed a stage *inside* your pipeline, which arrives as an `exit_code` of `128 + signum`. |
| `status` | Rendered phrase: `"exit code 101"`, `"killed by SIGKILL (signal 9)"`, `"exit code 141 (probable SIGPIPE)"`, or `"exit code unknown"`. `null` while the task is still running. |

The success test is **`exit_code == 0`**, nothing weaker:

- `exit_code: null` is **never** success. The child died on a signal (see
  `signal`), or the engine got no status at all. Both are failures.
- A signal death is not an exit code. A task killed by SIGKILL reports
  `exit_code: null, signal: 9`, never `0`, `137` or `-1`.
- `timed_out: true` (watchdog) and `killed: true` (`bash_kill`) mean the engine
  ended the task. Both usually carry `signal: 15`. A command that traps SIGTERM
  and exits on its own carries that exit code instead, and the flag still says
  who ended it.
- `abandoned: true` means the engine STOPPED while the task ran. Nothing
  cancelled the work, and no status was ever reaped, so `exit_code` and `signal`
  are `null` and `status` reads `"exit code unknown"`. **`abandoned` overrides
  the rule above**: this unknown status is an interruption, not a failure.
  Report the run as interrupted, never as failed and never as done. Before
  starting the same work again, check it is not still running.
- While the task runs, every one of these is `null` / absent. Absence of a
  status is not a passing status.

**A failing pipeline stage is never masked by a later succeeding one.** The
engine runs commands under `bash -o pipefail`, so `pytest … | tee run.log`
reports pytest's status, not `tee`'s `0`. You do NOT need a sidecar file for
the exit code: `exit_code` is trustworthy on its own.

`pipefail` reports the **rightmost failing** stage, and `0` only when every
stage succeeded. So `sh -c 'exit 42' | sh -c 'exit 7'` reports `7`, not `42`.
It tells you *that* a pipeline failed. To learn *which* of several stages
failed, run them as separate commands.

A producer whose consumer closes the pipe early (`yes | head -1`,
`long_output | head -20`) dies of SIGPIPE, and the pipeline reports it. It
arrives as `exit_code: 141, status: "exit code 141 (probable SIGPIPE)"`,
**not** as `signal: 13` (see the `signal` row above). That is a real non-zero
status, not an engine bug. To ignore it, terminate the pipeline yourself, for
example `{ long_output || true; } | head -20`.

Some waits are unbounded: a 30-minute backtest, an all-night sweep, a download
you can't size. When several `wait_secs=120` drains show no end in sight
(judge by `elapsed_secs`, not call count), stop draining. **`await_event` on `BackgroundBashCompleted`, with a `condition` on the `task_id`**, then end the turn. A finished background task persists that event, so the subscription costs nothing and re-opens the thread when the task ends. Prose like "I'll report when it finishes" cannot wake you.

Do **not** use a one-option `ask_user_question` to get resumed instead. That makes the human your scheduler, lights the Blocked badge, and blocks Apply until they tap. The one-option *wake question* is only for an unbounded wait with **no** Lucidos event to subscribe to.

## The command guard (dangerous commands are gated)

When a workspace turns on the command guard, every `run_bash` / `run_bash_background` / `run_python` / `run_python_background` call is checked before it runs. A fast static pass settles the obvious cases (catastrophic vs. obviously-safe). A cheap LLM **judge** classifies everything in between. The lanes:

- **Catastrophic → refused.** These are **refused without running**, with a failed tool result explaining why:
  - recursive deletion of the filesystem root or home directory (`rm -rf /`, `rm -rf ~`);
  - fork bombs;
  - formatting or overwriting a raw block device (`mkfs`, `dd of=/dev/…`, `> /dev/sd…`).

  Don't retry a refused command; pick a different, safe approach.
- **Irreversible real-world side-effect or out-of-workspace destruction → asks the user.** This covers a command that looks like it:
  - sends mail, or makes a mutating HTTP request (`curl -X POST …`, a data upload);
  - runs a cloud-service mutation (`gh`/`aws`/`gcloud`), or spends money;
  - does the same from Python (`requests.post(…)`, `smtplib`);
  - **deletes/overwrites files outside the workspace**.

  Such a command **pauses and shows the user a permission card**, and the thread waits like `ask_user_question`. If the user allows, the command runs. If they deny (or pick "Allow for this thread" / "Always allow" for similar commands), a tool result tells you the outcome. A denied command was NOT run. Don't retry it: explain what you intended, or choose a side-effect-free alternative.
- **In-workspace deletion/overwrite → runs, with a one-click Undo.** Destroying files *inside* the workspace (`rm -rf data/tmp`, clobbering `data/artifacts/x`) is recoverable, so the guard doesn't prompt. It snapshots the workspace, runs the command, and leaves a one-click **Undo** on the command's card. You need do nothing. Undo removes files the command created and restores files it deleted or overwrote. It keeps any file edited since the command ran.

**In a scheduled trigger there's no one to ask.** On a trigger fire, the irreversible lane checks the trigger's *side-effect grant* instead of prompting. The grant lists the categories the user authorized in the trigger's settings. A granted category runs. An ungranted one is **blocked and fails the trigger run**, and the user gets a failure notification naming the blocked command and the missing grant. So a trigger that must send mail or call a mutating API needs that grant first: see `system-knowhow/triggers.md` § Side-effect grant.

The guard is deliberately narrow. These all run with no prompt: reads anywhere on the machine (outside the workspace too, by design), a plain `curl https://…` GET, writes under `data/`, and redirects to `/dev/null`. Pure-compute Python that installs no packages runs unprompted too. An in-workspace *deletion* also runs unprompted, with the Undo above. The judge asks only when it can't tell a command is safe. The guard is off by default, so most workspaces never see it.

## The per-workspace venv

Lucidos provisions one Python venv per workspace at `.lucidos/runtime/python/venv/`. The `run_python*` tools run scripts inside it automatically. Don't activate it, reference it, or reach for `subprocess.run(["python", ...])`. Pass plain Python as the `code` arg.

- **Packages**: declare in the tool call's `packages: ["numpy", "pandas", ...]` arg. They're installed into the workspace venv before the script runs; already-installed packages are no-ops. Give PyPI names with an optional version (`numpy>=1.26`, `requests[socks]`). A URL, a path, an archive file or a pip flag is refused. A call with packages goes to the guard's judge, because installing one runs its build code. Don't `pip install` from inside `code`.
- **Working directory**: scripts execute with cwd = workspace root. So `open("data/artifacts/foo.csv")` is correct; `open("/Users/.../data/...")` is brittle.
- **Env vars**: `LUCIDOS_WORKSPACE` (workspace root, absolute), `CRED_*` for credentials, `OAUTH_*_ACCESS_TOKEN` for connected OAuth accounts. All are auto-injected.

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

1. **Sleep-poll**: `run_python(code="time.sleep(N)")` to wait for a background task. Use `bash_output(task_id, wait_secs=N)` instead. The engine also has a repeated-call guard. It buckets `run_python` calls by the first non-blank non-comment non-import line of `code` (truncated to 80 chars). It fires only on repeated **failures**: three ERRORs in a row with the SAME first actionable line trip it, three successes do not. A sleep-poll burns a turn either way.
2. **`os.chdir` + `sys.path.insert(0, ".")`** for importing app scripts. Use the absolute path via `$LUCIDOS_WORKSPACE` once. Don't try four variants.
3. **`subprocess.run(["python", "-c", code])`** from inside `run_python` to get a "fresh interpreter". You already are one. The subprocess won't see the venv's site-packages.
4. **Re-spawning a background task to read its result** instead of calling `bash_output(task_id)`. The task is still running with a known id; drain it.
5. **Polling `bash_output` after `finished: true`**. Nothing new can arrive (see § The drain pattern).
6. **Draining a long task to the end of the turn rather than ending the turn.** See "You do not have to sit through it" above.
7. **Spawning a shell loop to watch something** (`for i in $(seq 1 200); do … sleep 60; done`). It cannot re-open a thread, it burns a process for an hour, and it dies unread with your turn.
8. **Detaching with `&` or `nohup` from `run_bash`.** `&` binds looser than `&&`. So `cd x && thing & echo ok` backgrounds the whole chain as one subshell, which keeps holding the tool's output pipes. You get the shell's real exit status and a note that a detached process survived, then nothing more. The process runs on with no task id, no watchdog and no completion event. Use `run_bash_background` instead, and drain it with `bash_output(task_id, wait_secs=N)`.

## Errors

`run_python` (the sync foreground tool) auto-trims long tracebacks. It keeps the first and last `File "..."` frame and the final `ExceptionClass: message` line, and drops the middle. The full traceback stays on disk at `.lucidos/exhaust/<run_id>/stderr.txt` for seven days.

`run_python_background` does NOT auto-trim. Its stderr flows through `bash_output` raw, and a chained import error can dump many KB of frames into your context. For a backtest or long script that might crash loudly, wrap the body in your own `try / except`. Re-raise a short fingerprint: `print(f"FAIL: {type(e).__name__}: {e}", file=sys.stderr); raise`. The full traceback still lands on disk at `.lucidos/exhaust/<task_id>/stderr.txt`, kept for seven days.

A `run_python` call that hits the 300 s ceiling fails with `Python script
timed out after 300s`. The code was fine; the budget was not. Never retry it
as-is: move it to `run_python_background`, or cut the work down.

The trimmed (or short) form is enough to act on: diagnose the exception class and the user-frame line, fix the script, retry once. Don't cycle through sys.path / chdir variants; check what's importable first.
