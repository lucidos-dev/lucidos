---
name: run-e2e
description: Use when asked to "run e2e", "run the full e2e suite", "run end-to-end tests", or any variant — runs `./scripts/e2e.sh` (API + browser + WASM + embedder), iterates with targeted sub-scripts, never bypasses a failing test, reports exact per-phase counts.
---

# Run the full e2e suite

Commit, then run `./scripts/e2e.sh`. It runs the full e2e suite in four
phases, on GitHub's runners by default (see GitHub mode below). Iterate to
green; never bypass a failing test. Zero failed AND zero skipped is the bar.

## Never run e2e on top of itself

Do NOT launch a second e2e run while one may still be alive — not
concurrently, and not by re-spawning over a believed-dead prior run. The
shared e2e-test workspace OOMs the host when two sets of Playwright/WebKit
browsers stack (2026-04-19 reboot; 2026-06-21 nightly pile-up to 23.5 GB +
14 GB swap). This is enforced structurally by the lock in
`scripts/lib/e2e_lock.sh`, which every entry point acquires before any
workspace/browser work: a second run with a live owner **hard-fails (exit 1)**,
and a stale lock left by an interrupted run is reclaimed only after its
orphaned browsers/engine are swept — if they can't be swept it **refuses**
rather than stack. So if `./scripts/e2e.sh` exits non-zero with an
"another e2e run is in progress" or "orphaned processes" message, that is the
lock doing its job — investigate/clean up the prior run; do NOT retry-loop to
force a launch.

**Waiting for the lock is free, so never sleep on it.** Subscribe with
`lucidos await-event --on E2ELockReleased` and end your turn; the engine
re-opens the thread when the holder releases. The `e2e-lock-wait` skill has the
full rules, including how to pick the timeout and the two cases where nothing
will wake you. A `sleep`/retry loop around the entry script is the anti-pattern
it exists to replace.

## Four phases

1. **API** — `cargo test -p lucidos-e2e --test api`.
2. **Browser** — Playwright across `chromium` + `mobile` + `mobile-webkit`.
3. **WASM signers** — `./scripts/e2e-wasm.sh` builds `signers/*/foo.wasm`
   artifacts then runs `crates/lucidos-e2e/tests/wasm_signers.rs`.
4. **Real-embedder** — `./scripts/e2e-embedder.sh` runs the gated tests
   in `lucidos-engine` via `--features real-embedder-tests` (downloads
   ~465 MB from huggingface.co on first run; cached after).

## Run it as a background task, and send its output to a log file

A full run outlives a foreground call's 10-minute ceiling, and a job you
background yourself dies when your turn ends. Claude Code has no blocking wait
tool either. So hand the run to the engine as a *background task*, then end
your turn: the engine re-opens the thread when the suite finishes, with its
exit status and the tail of its output.

```
lucidos background-task run --description "the full e2e suite" \
  -- 'mkdir -p .lucidos && ./scripts/e2e.sh > .lucidos/e2e.log 2>&1'
```

Redirect into the worktree's own gitignored `.lucidos/` rather than a shared
`/tmp` name another session could truncate. The log is where you read the
detail once the thread re-opens.

**Do not end the command with `; echo $? > file`.** The engine already reports
the suite's own exit status, and that `echo` always succeeds, so a red run
comes back as exit 0. The exit-file join belongs to `/harden`'s in-turn run
only. To keep a copy in a file anyway, pass the status on:

```
lucidos background-task run --description "the full e2e suite" \
  -- './scripts/e2e.sh > .lucidos/e2e.log 2>&1; rc=$?; echo $rc > .lucidos/e2e.exit; exit $rc'
```

Read the log like this:

```
tail -40 .lucidos/e2e.log
grep -nE "passed|failed|Error|✘" .lucidos/e2e.log | tail -20
```

Do NOT wait with `sleep + tail`, and do not stay in the turn polling the log:
each check re-reads your whole context, and the re-open already tells you when
the suite is done. `lucidos background-task output <task_id>` shows progress if
you have a reason to look before then.

The slow start is normal. `ensure_workspace_running` in `scripts/lib/e2e.sh`
prints "Starting e2e workspace (LUCIDOS_MODEL=mock)... Probing" while it builds
and boots the engine, and the first build can take several minutes. Do not
stop and restart a run for that.
(The e2e scripts boot that workspace themselves: never pre-start it with
`web-dev.sh`, which launches the machine-global gateway and is refused from a
coding-agent worktree. See ADR 0021.)

**Inside `/harden`, stay in the turn.** Apply reads the next idle as a finished
`/harden`, so there use the exit-file join `/harden` Phase 4.5 describes.

## GitHub mode is the default

Every e2e script runs on GitHub's runners when it can (ADRs 0382, 0386), and
its filters mean the same there. The first line of output says where it runs.

- **`--local` runs here.** Use it to iterate on uncommitted edits while the
  e2e lock is free. A local-only flag such as `--no-reset` or `--headed`
  implies it, and so does a screenshot switch such as `THREAD_SHOTS=1`, since
  GitHub would never see it.
- **Re-run committed work on GitHub, never in the lock queue.** When the lock
  is held, commit and send the re-run to GitHub
  (`./scripts/e2e-browser.sh -f <spec>`, `./scripts/e2e-api.sh -f <filter>`).
  Wait for the lock only for uncommitted edits or a `@real-claude-code` spec.
- **It tests the committed HEAD, so commit first.** A dirty tree runs locally
  instead, and says so. So does a checkout without the release libraries, the
  `lucidos` remote or `gh`. `--github` turns each of those into an error.
- **Only spec files with a test tagged `@real-claude-code` run here**, as the
  local leg, under the e2e lock (ADR 0388). The leg waits up to an hour for a
  held lock rather than failing. A selection without them takes no lock and
  builds nothing here.
- **It is a long wait.** A cold run builds the engine before any test starts.
  Run it as a background task, exactly like a local run.
- **Exit 76 means no verdict**: the push was refused, the run never started or
  was cancelled, or a shard left no output. Report it. Never read it as green.
- **The output stays** under `test-results/github/<run-id>/`. For a failed
  shard, `gh run download <run> --repo lucidos-dev/lucidos -n e2e-traces-<shard>`
  fetches its Playwright traces and engine log.
- **Fix a failure locally.** Reproduce it with the targeted sub-scripts below.
  Rerun one spec on GitHub with `./scripts/e2e-browser.sh -f <spec>` to confirm
  the fix there.

## A memory stop is the harness's to recover, never yours

The harness waits out a host in memory trouble and resumes on its own, so one
run gives one verdict (ADR 0351). Read the exit code, then the `[e2e-resume]`
lines at the end of the log:

| Exit | Meaning | What you do |
|---|---|---|
| 71 | memory stop the harness could not resume | report the unverified range it printed as the carry-over |
| 72 | the build memory gate refused: nothing was built or tested | report it; the host was in trouble |
| 75 | the host-load guard refused the launch | report it; nothing ran |

**After one of these, never start a second leg, a discharge or a retry.** Never
start a cargo build or `/harden` suites while the host reads NO-GO either. Each
of those is how a night failed before the harness owned its recovery. The
unverified range goes to a later run on a cold host.

## Targeted sub-scripts

After the first full run identifies failures, debug on this host with the
targeted sub-scripts while the lock is free. `--no-reset` keeps them local;
don't keep re-running the full suite while debugging:

- `./scripts/e2e-api.sh --no-reset -f <test_name>` — single Rust API test.
- `./scripts/e2e-browser.sh --no-reset -f <file.spec.ts> [-- --grep "<name>"]` — single Playwright test.
- `./scripts/e2e-wasm.sh --local` — WASM phase (rebuilds + reruns).
- `./scripts/e2e-embedder.sh --local` — embedder phase.

Once a fix is committed, confirm it on GitHub with the same filter, minus
`--no-reset`. Then run the full `./scripts/e2e.sh` once on GitHub for the final
clean verification.

## No bypassing — every test must run and pass

If any test fails, investigate and fix the ROOT CAUSE — either the
underlying bug or a genuinely broken test — then iterate until green.

Forbidden in any phase (Rust or Playwright):

- `test.skip()`, `test.fixme()`, `.skip`, `.fixme`, `xit()`, `xdescribe()`
- `#[ignore]`, `#[cfg(skip)]`, conditional `return` early-outs that pretend the test passed
- `--grep-invert`, `--project=<single>`, narrowing globs, or any flag that hides a failing test from the run
- Commenting out an assertion, the test body, the test, or its file
- Marking a test as "flaky" and retrying past a real failure (flaky-recovered is OK only when the test ultimately passes on its own)

If a test or the code under test looks "wrong", the fix is to **correct
the test or the code under test** — never to mute it. If you can't fix
the root cause from this session, report it as an unfixable failure.

## When to give up

Only stop if the failure is genuinely unfixable from this session
(e.g. missing infra, environmental, repeated apply conflicts).

## Reporting

Final status: PASSED or FAILED. Include exact pass/fail counts per
phase: API, each browser project (chromium, mobile, mobile-webkit),
WASM signers, real-embedder.
