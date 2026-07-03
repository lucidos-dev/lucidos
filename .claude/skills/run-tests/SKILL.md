---
name: run-tests
description: Use when asked to "run tests", "build & test", "check the engine builds", or any variant — builds the engine (`cargo build -p lucidos-engine --release`) and runs the full engine suite via `./scripts/test-engine.sh --full` (which provisions a disposable Postgres — bare `cargo test -p lucidos-engine` panics every DB-backed integration test), plus the frontend Vitest suite (`cd crates/lucidos-app && npx tsc --noEmit && npm test`). Fixes failures at root cause, never bypasses with #[ignore], reports exact pass/fail counts.
---

# Run tests (engine + frontend unit)

Build + test both layers — Rust engine and the lucidos-app TypeScript
unit tests. Fix any failure at the root cause; never skip, ignore, or
comment out a failing test.

## Commands

Run both phases. If either fails, the whole skill FAILED.

1. **Rust engine.** Two steps:
   1. **Build:** `cargo build -p lucidos-engine --release` — verifies the engine
      compiles in the profile it ships in (a separate compilation from the test
      build below, which is debug + `cfg(test)`).
   2. **Test:** `./scripts/test-engine.sh --full` (equivalently `make test-full`).
      Runs the whole crate — lib + integration + doctests.
      **Do NOT run bare `cargo test -p lucidos-engine`.** The engine's
      integration tests (`setup_test_db` in `src/test_support.rs`) need a real
      Postgres: each `CREATE`s a throwaway `lucidos_test_*` database, migrates,
      and drops it, reading the connection from `TEST_DATABASE_URL`. With no
      `TEST_DATABASE_URL` and no PG up, every DB-backed test panics on connect
      (`.expect("admin connect")`) — that's *hundreds of false failures*, not
      regressions, and it has blocked this skill before. `test-engine.sh`
      provisions a dedicated, disposable `lucidos-pg-test` container (pgvector,
      port `LUCIDOS_TEST_PG_PORT` / default 5510), exports `TEST_DATABASE_URL`,
      then runs `cargo test`. It is isolated from every workspace's PG and never
      broad-kills (touches only its own container by exact name). Requires Docker
      running — if Docker is down the script exits 1 before any test; report that
      as FAILED (infra), not green. To narrow to a filter while iterating a fix:
      `./scripts/test-engine.sh -- -- <test_name>` (the double `--` is required so
      the names reach the test binary, not cargo).
2. **Frontend unit (Vitest + tsc).** `cd crates/lucidos-app && npx tsc --noEmit && npm test`.
   - `npm test` runs `vitest run` (single pass, no watch).
   - `npx tsc --noEmit` catches type regressions that Vitest alone misses.
   - Run from the `crates/lucidos-app` directory — npm workspaces resolve
     correctly from there.

If anything fails to compile or any test fails, fix the ROOT CAUSE in
source code and re-run until green. NEVER skip, ignore, mark as
`#[ignore]` / `.skip` / `.todo`, comment out, or otherwise bypass a
failing test — every test must run and pass.

## Reading exit codes honestly — never trust a piped exit

Do NOT run the test commands through `| tail`, `| head`, `| grep`, or
any other pipe to trim output. Under zsh / bash a pipeline reports the
exit code of the *last* command, not `cargo`/`npm`/the script — so
`./scripts/test-engine.sh --full | tail` exits 0 (tail's success) even
when a Rust test failed, and the skill silently reports PASSED on a red
run. This false-green has actually shipped a failing nightly. The sibling
`/clean-build` skill documents the full mechanism under "Reading exit
codes honestly — beware piped output".

`test-engine.sh` is safe to capture from directly: it runs `set -euo
pipefail` and its *last* command is the `cargo test`, so the script's
own exit code IS cargo's exit code — no wrapper masks it.

The rule: run each phase un-piped and read its real exit code. If the
output is too large, redirect to a log file and capture `$?` directly,
then grep the log — never let a pipe stand between you and the exit
status:

```sh
./scripts/test-engine.sh --full > /tmp/engine-test.log 2>&1; echo "EXIT: $?"
tail -100 /tmp/engine-test.log   # inspect AFTER capturing the real exit
```

A "PASSED" claim requires the echoed `EXIT: 0` you printed yourself AND
the `test result: ok.` line with `0 failed` — a trimmed tail alone can
lie. Cross-check both: a non-zero exit with no `test result:` line at all
usually means the *infra* failed (Docker down, port in use), not a test —
report that distinctly.

## Documented #[ignore] exceptions

Two tests in `crates/lucidos-engine/src/engine/thread_lifecycle_tests/contract.rs`
are intentionally `#[ignore]`d as contract-artifact generators
(`generate_typescript_file` and `generate_cross_validation_fixture_file`).
Running them rewrites generated source files; sibling non-ignored tests
verify staleness. Expect `Ignored: 2` matching these names — anything
else is a real skip and must be fixed.

If a future change introduces a *new* `#[ignore]`, it must come with
either (a) a sibling non-ignored verification test, or (b) a script
under `./scripts/` that runs it as part of the nightly pipeline.
Otherwise, fix the underlying issue or report it as an unfixable
failure.

## Out of scope

Heavy integration suites with external setup (WASM signers, real-embedder)
live behind cargo features and run via `./scripts/e2e-wasm.sh` and
`./scripts/e2e-embedder.sh` in a separate phase — don't run them from
this skill.

## When to give up

Only stop if the failure is genuinely unfixable from this session
(e.g. missing toolchain, infra outage, environmental).

## Reporting

Final status: PASSED or FAILED (FAILED if either phase failed).
Include exact counts per phase:

- **Rust:** library / integration / doc-test passes, failures, ignored.
- **Frontend:** test files run, tests passed, tests failed, tests skipped,
  plus the `tsc --noEmit` exit status.
