---
name: run-e2e
description: Use when asked to "run e2e", "run the full e2e suite", "run end-to-end tests", or any variant — runs `./scripts/e2e.sh` (API + browser + WASM + embedder), iterates with targeted sub-scripts, never bypasses a failing test, reports exact per-phase counts.
---

# Run the full e2e suite

Commit, then run `./scripts/e2e.sh`. It runs the full e2e suite in four
phases, on GitHub's runners by default (see the GitHub section below). Iterate to
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
rather than stack. A Chromium-only local run takes one of two *e2e slots*
instead (`e2e-test`, `e2e-test-2`), so two worktrees can run side by side. A
WebKit run or the umbrella takes both slots, which is the old exclusive lock. So if `./scripts/e2e.sh` exits non-zero with an
"another e2e run is in progress" or "orphaned processes" message, that is the
lock doing its job — investigate/clean up the prior run; do NOT retry-loop to
force a launch.

**Never wait for the lock** (ADR 0541). A busy lock exits 75: rerun a
committed selection with `--github`, or else rely on the other tests and the
main check, and report that the spec did not run. No `await-event`, no `sleep`,
no retry loop. The `e2e-lock-busy` skill has the rule.

## Four phases

1. **API** — `cargo test -p lucidos-e2e --test api`.
2. **Browser** — Playwright across `chromium` + `mobile` + `mobile-webkit`.
3. **WASM signers** — `./scripts/e2e-wasm.sh` builds `signers/*/foo.wasm`
   artifacts then runs `crates/lucidos-e2e/tests/wasm_signers.rs`.
4. **Real-embedder** — `./scripts/e2e-embedder.sh` runs the gated tests
   in `lucidos-engine` via `--features real-embedder-tests` (downloads
   ~465 MB from huggingface.co on first run; cached after).

## Run it as a background task, and send its output to a log file

A full run outlives a 4-minute foreground call, the most a call may block
before the prompt cache expires. A job you background yourself dies when your
turn ends. So hand the run to the engine as a *background task*.

Poll the run. Wait on it in the foreground with `timeout: 240000`, and
re-issue `wait` until it reports finished. Each `wait` returns within 230 s,
so the cache stays warm:

```
lucidos background-task run --poll --description "the full e2e suite" \
  -- 'mkdir -p .lucidos && ./scripts/e2e.sh > .lucidos/e2e.log 2>&1'
lucidos background-task wait <task_id>
```

A run queued behind a held lock has an open-ended wait. It drops `--poll`
and ends the turn instead. The engine re-opens the thread when the
suite finishes, with its exit status and the tail of its output.

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

Do NOT wait with `sleep + tail`, and do not poll the log: `wait` already
blocks until the suite is done or its 230 s pass, and hands you the new
output.

The slow start is normal. `ensure_workspace_running` in `scripts/lib/e2e.sh`
prints "Starting e2e workspace (LUCIDOS_MODEL=mock)... Probing" while it builds
and boots the engine, and the first build can take several minutes. Do not
stop and restart a run for that.
(The e2e scripts boot that workspace themselves: never pre-start it with
`web-dev.sh`, which launches the machine-global gateway and is refused from a
coding-agent worktree. See ADR 0021.)

**Inside `/harden`, stay in the turn.** Apply reads the next idle as a finished
`/harden`, so there use the exit-file join `/harden` Phase 4.5 describes.

## A filtered run stays here, a whole suite goes to GitHub

A whole suite runs on GitHub's runners when it can (ADRs 0382, 0386). A
*filtered run* stays on this host (ADR 0536): `-f`, or after `--` a
Playwright `--grep`, `-g`, `--grep-invert` or a positional argument. The
first line of output says where it runs.

**Iterate on unit and component tests, not on specs.** Run the changed spec
locally once to reproduce and once to confirm. A local filtered run is lean:
a debug engine that compiles incrementally, no mobile-webkit unless `--webkit`
asks for it, and no `npm ci` when the dependencies already match.

- **`--local` runs a whole suite here.** A local-only flag such as
  `--no-reset` or `--headed` implies it, and so does a screenshot switch such
  as `THREAD_SHOTS=1`, since GitHub would never see it.
- **`--github` sends a filtered run to GitHub.** Use it when both slots are
  busy, for a mobile-webkit spec, or to confirm a fix on Linux and macOS
  (`./scripts/e2e-browser.sh --github -f <spec>`). Wait for the lock only for
  a `@real-claude-code` spec.
- **It tests the committed HEAD, so commit first.** A dirty tree runs locally
  instead, and says so. So does a checkout without the release libraries, the
  `lucidos` remote or `gh`. `--github` turns each of those into an error.
- **Only spec files with a test tagged `@real-claude-code` run here**, as the
  local leg, under the e2e lock (ADR 0388). The leg waits up to an hour for a
  held lock rather than failing. A selection without them takes no lock and
  builds nothing here.
- **The wait depends on the engine.** A GitHub run reuses an engine whose
  *engine input manifest* matches the tree. Then one spec takes about 3 min,
  or 4.5 min with WebKit. A Rust change compiles incrementally, and a WebKit
  run then takes 10-13 min. Run it as a polled background task either way.
- **Exit 76 means no verdict**: the push was refused, the run never started or
  was cancelled, or a shard left no output. Report it. Never read it as green.
- **The output stays** under `test-results/github/<run-id>/`. For a failed
  shard, `gh run download <run> --repo lucidos-dev/lucidos -n e2e-traces-<shard>`
  fetches its Playwright traces and engine log.
- **Fix a failure locally.** Reproduce it with the targeted sub-scripts below.
  Rerun one spec on GitHub with `./scripts/e2e-browser.sh --github -f <spec>`
  to confirm the fix there.

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
targeted sub-scripts. A filter keeps them local, and `--no-reset` also keeps
the workspace up between runs. Don't keep re-running the full suite while
debugging:

- `./scripts/e2e-api.sh --no-reset -f <test_name>` — single Rust API test.
- `./scripts/e2e-browser.sh --no-reset -f <file.spec.ts> [-- --grep "<name>"]` — single Playwright test.
- `./scripts/e2e-wasm.sh --local` — WASM phase (rebuilds + reruns).
- `./scripts/e2e-embedder.sh --local` — embedder phase.

Once a fix is committed, confirm it on GitHub with the same filter plus
`--github`, minus `--no-reset`. Then run the full `./scripts/e2e.sh` once on GitHub for the final
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
