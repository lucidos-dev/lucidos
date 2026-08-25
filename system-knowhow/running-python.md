---
name: Running Python
description: How to use run_python, run_python_background and bash_output: picking the right one, the venv layout, importing from app scripts, and the antipatterns (sleep-poll, sys.path thrashing). Load BEFORE writing run_python code if you haven't already this thread.
---

# Running Python

How the chat agent runs Python in a workspace, and which mistakes are silent context burners. Load this when you're about to reach for `run_python` or `run_python_background` and you haven't already loaded it earlier in the thread.

## Pick the right tool

| Tool | When | Ceiling |
|------|------|---------|
| `run_python` | Quick scripts: data prep, plotting, file conversion, one-off transforms. Returns stdout synchronously when the script finishes. | 300 s hard, not adjustable |
| `run_python_background` | Anything that may run longer than ~30 s: backtests, model training, large data sweeps, batch downloads. Returns a `task_id` immediately; drain with `bash_output(task_id, wait_secs=…)`. | watchdog `timeout_secs` (default 600 s, max 3600 s) |
| `bash_output(task_id, wait_secs?)` | The drain tool for the two above AND for `run_bash_background`. Pass `wait_secs` to BLOCK server-side for that many seconds, or until the task finishes. | up to 120 s per call |
| `bash_kill(task_id)` | Cancel a running background task. No-op if already finished. | — |

Decision rule: if you'd reach for `time.sleep` inside `run_python`, you almost certainly want `run_python_background` plus a `bash_output(task_id, wait_secs=N)` instead. The single most common context-waster in chat threads is hand-rolling polling loops.

`run_python`'s 300 s sync ceiling is enforced, not advisory. At it the
interpreter is SIGKILLed and the call comes back as a failed tool result
naming the ceiling. There is no `timeout_secs` on `run_python` to raise, so a
script you can't confidently size belongs in `run_python_background` from the
start. A killed run commits nothing: its `data/` writes were only ever staged,
so you lose the work but never half of it.

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

# Tool call 2: drain with server-side wait — blocks the full 60 s,
# or returns early the moment the task finishes.
bash_output(task_id="abc", wait_secs=60)
# → { stdout: "...", finished: true, exit_code: 0, signal: null,
#     status: "exit code 0", elapsed_secs: 58, waited_secs: 58 }
```

`wait_secs` semantics:
- Up to 120 s per call (engine clamps higher values silently).
- Blocks for the **full** duration. Only two things end a wait early: the task finishing, and the user sending a message (so their follow-up isn't stuck behind your block — read it, answer it, then drain again).
- **New output does not wake you.** That is deliberate: anything chatty (a cargo build, `notarytool`, an npm install) emits something every few hundred milliseconds, so waking on the first byte made `wait_secs` a no-op and turned "wait two minutes" into a poll every two seconds.
- Returns whatever accumulated on timeout with `finished: false` — decide whether to call again.
- Use the full 120 s for anything long you're following; 30–60 s when you expect it to finish soon; 0 (or omit) for a quick liveness check between other actions.

A drain that lands at the exact moment the task completes still returns the
final tail with `finished: true`. The engine keeps a completed task drainable
for a few minutes after it ends, so finishing mid-drain never costs you the
result.

If `finished: true`, STOP polling. Nothing new can arrive: inside that
few-minute window a repeat call returns an empty window (you already drained
the output), and after it, calls fall back to the event store and re-return the
full final stdout/stderr each time, which is wasted context either way.

### You do not have to sit through it: ending the turn is a valid wait

A background task **outlives your turn**, and so does the engine's interest in
it. When you end a turn with one still running, the engine subscribes the
thread to that task's `BackgroundBashCompleted` and re-opens the thread with a
new turn the moment it lands, so you drain the result then. You will see the
subscription in the thread's own indicator, and the user sees it too, which is
how they tell a sleeping thread from a stalled one.

So for anything genuinely long, a release build, a full test suite, a
notarization, the shape is: spawn it, drain once or twice to confirm it started
cleanly, **report where things stand and end the turn**. Twenty consecutive
120-second drains spend twenty turns of context to learn what one wake tells
you for free.

Two things this replaces outright:

- **A shell polling loop** (`for i in $(seq 1 200); do …; sleep 60; done`) spawned
  to watch something. It cannot re-open a thread, it burns a process for an
  hour, and it dies unread.
- **Promising to "check back later".** You do not run between turns. Either a
  subscription re-opens the thread or nothing does, so a promise with no
  subscription behind it is a promise the engine cannot keep.

You can also subscribe explicitly with `await_event` when you want to watch
something *other* than the task's completion. For the completion itself you do
not need to: it is armed for you.

### Never estimate elapsed time — read it

Every drain reports two clocks, and they are the only ones you have:

| Field | Meaning |
|---|---|
| `elapsed_secs` | How long the task has been running, or its total runtime once `finished` (frozen at completion). `null` only when the result came from an old `BackgroundBashCompleted` record written before the engine stored the timestamp pair: an honest "unknown" rather than a fabricated `0`. |
| `waited_secs` | How long **this one call** actually blocked. Well short of the `wait_secs` you asked for, with `finished: false`? The user sent a message — that cuts the wait so you can answer it. Nothing is broken. |

Do not infer elapsed time from how long you *asked* to wait, or from how many
drains you've made. A chat agent doing exactly that narrated "roughly 20 minutes
in Apple's queue" ninety seconds into a release. Quote `elapsed_secs`, or say
nothing about timing.

### Oversized windows keep the tail

A drain is capped at 100 KB per stream. When a window exceeds it you get the
**most recent** bytes, behind a leading
`[truncated — N earlier bytes dropped, showing the most recent M of T total]`
marker — the failure at the end of a build log is never what gets dropped.

`N` is the whole gap, not just the part the drain trimmed: a task that emits
more than ~2 MB on one stream between two drains also outruns the engine's
buffer, and those bytes are counted too. So a very chatty task drained on a
long `wait_secs` can genuinely lose middle output — the marker will tell you.
If you need all of it, have the task `tee` to a file and `read_file` that.

## Deciding whether a background task succeeded

`bash_output` returns three status fields. **Read `status`** — it is the one-line
human phrase, and it is the same phrase the completion summary uses, so the two
can never tell you different things.

| Field | Meaning |
|---|---|
| `exit_code` | The normal exit status, and **only** that. `null` whenever there wasn't one. |
| `signal` | The Unix signal that killed the **shell Lucidos spawned**, if one did (a watchdog timeout / `bash_kill` gives `9`). `null` otherwise — including when a signal killed a stage *inside* your pipeline, which arrives as an `exit_code` of `128 + signum`. |
| `status` | Rendered phrase: `"exit code 101"`, `"killed by SIGKILL (signal 9)"`, `"exit code 141 (probable SIGPIPE)"`, or `"exit code unknown"`. `null` while the task is still running. |

The success test is **`exit_code == 0`**, nothing weaker. Specifically:

- `exit_code: null` is **never** success. It means the child died on a signal
  (see `signal`) or the engine could not obtain a status at all. Both are
  failures.
- A signal death is not an exit code. A task killed by SIGKILL reports
  `exit_code: null, signal: 9` — never `0`, never `137`, never `-1`.
- `timed_out: true` (watchdog) and `killed: true` (`bash_kill`) mean the engine
  ended the task. Both also carry `signal: 9`, since that is how it ended it.
- While the task runs, all three are `null` / absent. Absence of a status is not
  a passing status.

**A failing pipeline stage is never masked by a later succeeding one.** The
engine runs commands under `bash -o pipefail`, so `pytest … | tee run.log`
reports pytest's status, not `tee`'s `0`. You do NOT need to write the exit code
into a sidecar file to find out whether a piped command failed — `exit_code` is
trustworthy on its own.

Know what `pipefail` does and doesn't promise: the status is that of the
**rightmost failing** stage, and `0` only when every stage succeeded. So
`sh -c 'exit 42' | sh -c 'exit 7'` reports `7`, not `42`. It reliably tells you
*that* a pipeline failed; if several stages can fail and you need to know
*which*, run them as separate commands.

One consequence worth knowing: a producer whose consumer closes the pipe early
(`yes | head -1`, `long_output | head -20`) is killed by SIGPIPE, and the
pipeline now reports that instead of `0`. It arrives as
`exit_code: 141, status: "exit code 141 (probable SIGPIPE)"` — **not** as
`signal: 13`. `signal` is set only when the shell Lucidos spawned was itself
killed; a signal that kills a stage *inside* the pipeline comes back as the
shell's own `128 + signum` exit code. That is a real non-zero status, not an
engine bug. If you want to ignore it, terminate the pipeline yourself — e.g.
`{ long_output || true; } | head -20`.

When the wait is genuinely unbounded, a 30-minute backtest, a sweep that might run all night, a download you can't size up front, and several `wait_secs=120` drains have gone by with no end in sight, **don't end your turn with "I'll report when it finishes"**. (Judge "no end in sight" from the `elapsed_secs` the drains report, not from how many calls you've made.) The engine has no way to wake you back up from prose alone.

Stop draining and **`await_event` on `BackgroundBashCompleted`, with a `condition` on the `task_id`**. A finished background task persists that event, so this is an ordinary state wait: the subscription costs nothing, you end the turn, and the engine re-opens the thread the moment the task ends, however long that takes. Do **not** step aside with a one-option `ask_user_question` to get resumed instead. That makes the human your scheduler for something the engine already knows, and it lights the needs-attention badge and blocks Apply until they tap. The one-option *wake question* is the fallback for an unbounded wait with **no** Lucidos event to subscribe to at all, which a background task is not.

## The command guard (dangerous commands are gated)

When a workspace turns on the command guard, every `run_bash` / `run_bash_background` / `run_python` / `run_python_background` call is checked before it runs. A fast static pass settles the obvious cases (catastrophic vs. obviously-safe) and a cheap LLM **judge** classifies everything in between. The lanes:

- **Catastrophic → refused.** Recursive deletion of the filesystem root or home directory (`rm -rf /`, `rm -rf ~`), fork bombs, and formatting or overwriting a raw block device (`mkfs`, `dd of=/dev/…`, `> /dev/sd…`) are **refused without running**, and you get a failed tool result explaining why. Don't retry a refused command; pick a different, safe approach.
- **Irreversible real-world side-effect or out-of-workspace destruction → asks the user.** A command that looks like it sends mail, makes a mutating HTTP request (`curl -X POST …`, a data upload), runs a cloud-service mutation (`gh`/`aws`/`gcloud`), spends money, does the same from Python (`requests.post(…)`, `smtplib`), or **deletes/overwrites files outside the workspace** **pauses and shows the user a permission card**. While it's pending the thread waits on the user, exactly like `ask_user_question`. If the user allows, the command runs; if they deny (or pick "Allow for this thread" / "Always allow" for similar commands), you get a tool result telling you the outcome. A denied command was NOT run — don't retry it; explain what you intended or choose a side-effect-free alternative.
- **In-workspace deletion/overwrite → runs, with a one-click Undo.** Destroying files *inside* the workspace (e.g. `rm -rf data/tmp`, clobbering `data/artifacts/x`) is recoverable, so the guard doesn't prompt — it snapshots the workspace first, runs the command, and leaves a one-click **Undo** on the command's card. You don't do anything special; just know the user can revert it (Undo restores deleted/overwritten files, but doesn't remove files the command newly created).

**In a scheduled trigger there's no one to ask.** When this same irreversible lane is hit on a trigger fire, the guard checks the trigger's *side-effect grant* (the categories the user authorized in the trigger's settings) instead of prompting: a granted category runs; an ungranted one is **blocked and fails the trigger run** (the user gets a failure notification naming the blocked command and the missing grant). So if a trigger's intent legitimately needs to send mail or call a mutating API, the user must grant that side-effect on the trigger first: see `system-knowhow/triggers.md` § Side-effect grant.

This is deliberately narrow: ordinary work is untouched. Reads anywhere on the machine (including outside the workspace — that's a wanted feature), a plain `curl https://…` GET, writing under `data/`, redirecting to `/dev/null`, and pure-compute Python all run with no prompt (an in-workspace *deletion* also runs with no prompt, just with the Undo affordance above). The judge errs toward asking only when it genuinely can't tell a command is safe. The guard is off by default — most workspaces never see it.

## The per-workspace venv

Lucidos provisions one Python venv per workspace at `.lucidos/runtime/python/venv/`. The `run_python*` tools run scripts inside it automatically — you don't activate it, don't reference it, don't reach for `subprocess.run(["python", ...])`. Just write Python and pass it as the `code` arg.

- **Packages**: declare in the tool call's `packages: ["numpy", "pandas", ...]` arg. They're installed into the workspace venv before the script runs; already-installed packages are no-ops. Don't `pip install` from inside `code`.
- **Working directory**: scripts execute with cwd = workspace root. So `open("data/artifacts/foo.csv")` is correct; `open("/Users/.../data/...")` is brittle.
- **Env vars**: `LUCIDOS_WORKSPACE` (workspace root, absolute), `CRED_*` for credentials, `OAUTH_*_ACCESS_TOKEN` for connected OAuth accounts — all auto-injected.

## Importing from `data/apps/<x>/scripts/`

Apps that ship Python helpers put them under `data/apps/<app-id>/scripts/`. The venv has no PYTHONPATH for these — you must add the directory yourself, using `$LUCIDOS_WORKSPACE` so the path survives any future cwd change:

```python
import os, sys
sys.path.insert(0, os.path.join(os.environ["LUCIDOS_WORKSPACE"], "data/apps/habit-tracker/scripts"))

import strategy_params      # now resolves
import big_candle_backtest  # now resolves
```

Don't `os.chdir` into the scripts dir as a substitute — relative paths inside the imported modules then resolve from there instead of the workspace root, which breaks any `open("data/...")` inside them.

## Anti-patterns that burn context

These all look reasonable in isolation; they fail the same way every time and waste a turn each:

1. **Sleep-poll**: `run_python(code="time.sleep(N)")` to wait for a background task. Use `bash_output(task_id, wait_secs=N)` instead. The structural fix is the `bash_output(wait_secs)` server-side block; on top of it the engine has a repeated-call guard that buckets `run_python` calls by the first non-blank non-comment non-import line of `code` (truncated to 80 chars) and fires only on repeated **failures** — so three calls with the SAME first actionable line that each ERROR in a row trip it, but three that succeed do not. Either way a sleep-poll burns a turn of context each, so use `wait_secs` regardless of whether the guard fires.
2. **`os.chdir` + `sys.path.insert(0, ".")`** for importing app scripts. Use the absolute path via `$LUCIDOS_WORKSPACE` once — don't try four variants.
3. **`subprocess.run(["python", "-c", code])`** from inside `run_python` to get a "fresh interpreter". You already are one. The subprocess won't see the venv's site-packages.
4. **Re-spawning a background task to read its result** instead of calling `bash_output(task_id)`. The task is still running with a known id; drain it.
5. **Polling `bash_output` after `finished: true`**. Nothing new can arrive. For the next few minutes each call returns an empty window, and after that each one replays the full final stdout/stderr from the event store, which is exactly the context bloat the drain semantics exist to avoid.
6. **Draining a long task to the end of the turn rather than ending the turn.** A build with forty minutes left costs twenty 120-second drains and twenty turns of context, and the last one still might not reach the end. End the turn instead: the engine subscribes you to the completion and re-opens the thread when it lands. See "You do not have to sit through it" above.
7. **Spawning a shell loop to watch something** (`for i in $(seq 1 200); do … sleep 60; done`). It cannot re-open a thread when it finds what it was looking for, so nobody ever reads it, and it dies with your turn anyway.
8. **Detaching with `&` or `nohup` from `run_bash`.** `&` binds looser than `&&`. So `cd x && thing & echo ok` backgrounds the whole chain as one subshell, which keeps holding the tool's output pipes. You get the shell's real exit status and a note that a detached process survived, then nothing more. The process runs on with no task id, no watchdog and no completion event. Use `run_bash_background` instead, and drain it with `bash_output(task_id, wait_secs=N)`.

## Errors

`run_python` (the sync foreground tool) auto-trims long tracebacks before returning: keeps the first and last `File "..."` frame and the final `ExceptionClass: message` line, drops the middle. The full traceback is on disk at `.lucidos/exhaust/<run_id>/stderr.txt` for debugging.

`run_python_background` does NOT auto-trim — its stderr flows through `bash_output` raw, which is fine for short outputs but means a verbose chained import error can dump many KB of frames into your context. When you spawn a backtest / long script you expect MIGHT crash with a multi-thousand-line traceback, wrap the body in your own `try / except` and re-raise a short fingerprint: `print(f"FAIL: {type(e).__name__}: {e}", file=sys.stderr); raise`. The full traceback still lands on disk at `.lucidos/exhaust/<task_id>/stderr.txt`.

A `run_python` call that hits the 300 s ceiling fails with `Python script
timed out after 300s`. That is not a crash to diagnose and never a script to
retry as-is: the code was fine, the budget was not. Move it to
`run_python_background` (or cut the work down) rather than running it again
and spending another 300 s on the same wall.

The trimmed (or short) form is enough to act on — diagnose the exception class + the user-frame line, fix the script, retry once. Don't retry with sys.path / chdir variants in the hope that one sticks; check what's importable first.
