---
name: run-e2e
description: Use when asked to "run e2e", "run the full e2e suite", "run end-to-end tests", or any variant — runs `./scripts/e2e.sh` (API + browser + WASM + embedder), iterates with targeted sub-scripts, never bypasses a failing test, reports exact per-phase counts.
---

# Run the full e2e suite

Run `./scripts/e2e.sh` — this runs the full e2e suite in four phases.
Iterate to green; never bypass a failing test. Zero failed AND zero
skipped is the bar.

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
the Bash tool's `run_in_background: true` and poll with `BashOutput`.
That's per-session, doesn't time out at 10 minutes, and lets you keep
working while the suite runs.

## Polling long-running tasks — `<retrieval_status>timeout</retrieval_status>` is NOT failure

When polling a still-running task with `TaskOutput` (or `BashOutput`),
`<retrieval_status>timeout</retrieval_status>` paired with `<status>running</status>`
means "no new output within the retrieval window" — the underlying task is
fine. Don't tear it down, restart it, or tight-loop re-poll: each immediate
re-poll burns a turn for nothing. Pass a longer retrieval `timeout`
(30000–60000 ms) AND wait at least 30s before re-polling. The canonical
slow case is the e2e workspace probe (`./scripts/web-dev.sh -w e2e-test -b`,
"Starting e2e workspace (LUCIDOS_MODEL=mock)... Probing") — the first build
can take several minutes before the probe responds; full `cargo build`
behaves the same way.

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
