---
name: run-e2e
description: Use when asked to "run e2e", "run the full e2e suite", "run end-to-end tests", or any variant — runs `./scripts/e2e.sh` (API + browser + WASM + embedder), iterates with targeted sub-scripts, never bypasses a failing test, reports exact per-phase counts.
---

# Run the full e2e suite

Run `./scripts/e2e.sh` — this runs the full e2e suite in four phases.
Iterate to green; never bypass a failing test. Zero failed AND zero
skipped is the bar.

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

## Iteration tip: background the run, and send its output to a log file

Do NOT block on long e2e commands with `sleep + tail`. A fixed sleep ignores an
early exit, so it wastes whatever time the suite finished ahead of your guess.
Spawn them with the Bash tool's `run_in_background: true` and wait with
`TaskOutput`, which is per-session and returns the instant the task exits.

Redirect the output at spawn time, into the worktree's own gitignored
`.lucidos/` rather than a shared `/tmp` name another session could truncate:

```
mkdir -p .lucidos && ./scripts/e2e.sh > .lucidos/e2e.log 2>&1
```

**`TaskOutput` is not a delta.** Every call replays the task's ENTIRE
accumulated output, not just what arrived since your last call, so an
un-redirected e2e run pours its whole log into your context on every single
wait. With the output in a file the task itself stays quiet, each wait costs
almost nothing, and you read exactly what you want:

```
tail -40 .lucidos/e2e.log
grep -nE "passed|failed|Error|✘" .lucidos/e2e.log | tail -20
```

The suite's exit code still comes back on the `TaskOutput` result, so
redirecting costs you nothing you were using. If you already launched a run
without redirecting, don't re-call `TaskOutput` to inspect it: the Bash result
printed the file the output is being written to, and `tail`-ing that path is
bounded where `TaskOutput` is not.

## Waiting on long-running tasks: `<retrieval_status>timeout</retrieval_status>` is NOT failure

`<retrieval_status>timeout</retrieval_status>` paired with `<status>running</status>`
means the task did not finish inside that retrieval window. The task is fine.
Don't tear it down, restart it, or tight-loop re-poll: each immediate re-poll
burns a round-trip for nothing. Wait with `block: true` and `timeout: 600000`
and re-issue that call until the task is done, rather than ticking on a short
interval. The canonical slow case is the e2e workspace probe:
`ensure_workspace_running` in `scripts/lib/e2e.sh` printing "Starting e2e
workspace (LUCIDOS_MODEL=mock)... Probing" while it builds and boots the engine.
The first build can take several minutes before the probe responds; full
`cargo build` behaves the same way.
(The e2e scripts boot that workspace themselves: never pre-start it with
`web-dev.sh`, which launches the machine-global gateway and is refused from a
coding-agent worktree. See ADR 0021.)

## Targeted sub-scripts

After the first full run identifies failures, iterate with the targeted
sub-scripts — don't keep re-running the full suite while debugging:

- `./scripts/e2e-api.sh --no-reset -f <test_name>` — single Rust API test.
- `./scripts/e2e-browser.sh --no-reset -f <file.spec.ts> [-- --grep "<name>"]` — single Playwright test.
- `./scripts/e2e-wasm.sh` — WASM phase (rebuilds + reruns).
- `./scripts/e2e-embedder.sh` — embedder phase.

After fix iteration, run the full `./scripts/e2e.sh` once for the final
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
