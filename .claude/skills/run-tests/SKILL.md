---
name: run-tests
description: Use when asked to "run tests", "build & test", "check the engine builds", or any variant — runs `cargo build --release && cargo test -p lucidos-engine` plus the frontend Vitest suite (`cd crates/lucidos-app && npx tsc --noEmit && npm test`), fixes failures at root cause, never bypasses with #[ignore], reports exact pass/fail counts.
---

# Run tests (engine + frontend unit)

Build + test both layers — Rust engine and the lucidos-app TypeScript
unit tests. Fix any failure at the root cause; never skip, ignore, or
comment out a failing test.

## Commands

Run both phases. If either fails, the whole skill FAILED.

1. **Rust engine.** `cargo build --release` then `cargo test -p lucidos-engine`.
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
exit code of the *last* command, not `cargo`/`npm` — so
`cargo test -p lucidos-engine | tail` exits 0 (tail's success) even when
a Rust test failed, and the skill silently reports PASSED on a red run.
This false-green has actually shipped a failing nightly. The sibling
`/clean-build` skill documents the full mechanism under "Reading exit
codes honestly — beware piped output".

The rule: run each phase un-piped and read its real exit code. If the
output is too large, redirect to a log file and capture `$?` directly,
then grep the log — never let a pipe stand between you and the exit
status:

```sh
cargo test -p lucidos-engine > /tmp/cargo-test.log 2>&1; echo "EXIT: $?"
tail -100 /tmp/cargo-test.log   # inspect AFTER capturing the real exit
```

A "PASSED" claim requires the echoed `EXIT: 0` you printed yourself AND
the `test result: ok.` line with `0 failed` — a trimmed tail alone can lie.

## Documented #[ignore] exceptions

Two tests in `crates/lucidos-engine/src/engine/thread_lifecycle_tests/tests.rs`
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
