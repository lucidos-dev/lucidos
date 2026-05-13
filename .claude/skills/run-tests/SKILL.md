---
name: run-tests
description: Use when asked to "run tests", "build & test", "check the engine builds", or any variant — runs `cargo build --release && cargo test -p lucidos-engine`, fixes failures at root cause, never bypasses with #[ignore], reports exact pass/fail counts.
---

# Run tests (lucidos-engine)

Build + test the engine. Fix any failure at the root cause; never skip,
ignore, or comment out a failing test.

## Commands

`cargo build --release` then `cargo test -p lucidos-engine`.

If anything fails to compile or any test fails, fix the ROOT CAUSE in
source code and re-run until green. NEVER skip, ignore, mark as
`#[ignore]`, comment out, or otherwise bypass a failing test — every
test must run and pass.

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

Final status: PASSED or FAILED. Include exact counts: library /
integration / doc-test passes, failures, ignored.
