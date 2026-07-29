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

## Four phases

1. **API** — `cargo test -p lucidos-e2e --test api`.
2. **Browser** — Playwright across `chromium` + `mobile` + `mobile-webkit`.
3. **WASM signers** — `./scripts/e2e-wasm.sh` builds `signers/*/foo.wasm`
   artifacts then runs `crates/lucidos-e2e/tests/wasm_signers.rs`.
4. **Real-embedder** — `./scripts/e2e-embedder.sh` runs the gated tests
   in `lucidos-engine` via `--features real-embedder-tests` (downloads
   ~465 MB from huggingface.co on first run; cached after).

## Iteration tip — never block on long e2e commands

Do NOT block on long e2e commands with `sleep + tail`. Spawn them with
the Bash tool's `run_in_background: true` and poll with `TaskOutput`.
That's per-session, doesn't time out at 10 minutes, and lets you keep
working while the suite runs.

## Polling long-running tasks — `<retrieval_status>timeout</retrieval_status>` is NOT failure

When polling a still-running task with `TaskOutput`,
`<retrieval_status>timeout</retrieval_status>` paired with `<status>running</status>`
means "no new output within the retrieval window" — the underlying task is
fine. Don't tear it down, restart it, or tight-loop re-poll: each immediate
re-poll burns a turn for nothing. Pass a longer retrieval `timeout`
(30000–60000 ms) AND wait at least 30s before re-polling. The canonical
slow case is the e2e workspace probe — `ensure_workspace_running` in
`scripts/lib/e2e.sh` printing "Starting e2e workspace (LUCIDOS_MODEL=mock)...
Probing" while it builds and boots the engine. The first build can take several
minutes before the probe responds; full `cargo build` behaves the same way.
(The e2e scripts boot that workspace themselves — never pre-start it with
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
