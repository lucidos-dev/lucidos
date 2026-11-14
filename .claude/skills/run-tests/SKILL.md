---
name: run-tests
description: Use when asked to "run tests", "build & test", or "check the engine builds". Builds the engine, runs the full engine suite via `./scripts/test-engine.sh --full`, then the frontend suite. Bare `cargo test -p lucidos-engine` panics every DB-backed test, so never use it. Fixes failures at root cause, never bypasses with #[ignore], and reports exact pass/fail counts.
---

# Run tests (engine + frontend unit)

Build + test both layers — Rust engine and the lucidos-app TypeScript
unit tests. Fix any failure at the root cause; never skip, ignore, or
comment out a failing test.

## Commands

Run both phases. If either fails, the whole skill FAILED.

1. **Rust engine.** Three steps:
   1. **Build:** `cargo build -p lucidos-engine --release` — verifies the engine
      compiles in the profile it ships in (a separate compilation from the test
      build below, which is debug + `cfg(test)`).
   2. **Eval crate:** `cargo test --locked -p lucidos-eval`, run before the
      engine suite because it needs no Postgres and takes under a second.
      **`./scripts/test-engine.sh --full` is NOT equivalent to `make
      test-full`**: only the make target carries the `test-eval` prerequisite,
      so the script alone drops this phase and its ~270 tests silently.
      The crate is bin-only, so this runs the binary's own unit tests.
      `scripts/check-eval-not-a-test.sh` (in `make lint`) keeps those tests
      unable to reach anything that spends money or boots a workspace
      (ADR 0087).
   3. **Test:** `./scripts/test-engine.sh --full`.
      Runs the whole crate: lib + integration + doctests.
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
2. **Frontend unit (Vitest + tsc).** `cd crates/lucidos-app && npm run typecheck && npm test`. Both run `./scripts/codegen.sh` first, so neither checks a stale generated file.
   - `npm test` runs `vitest run` (single pass, no watch).
   - The type check catches type regressions that Vitest alone misses.
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

The build and the full engine suite outlast a 4-minute foreground call. No
call may block longer, or the prompt cache expires. Run
each as a polled background task, and wait on it in the foreground with
`timeout: 240000`:

```sh
lucidos background-task run --poll --description "the full engine suite" \
  -- 'mkdir -p .lucidos && ./scripts/test-engine.sh --full > .lucidos/engine-test.log 2>&1'
lucidos background-task wait <task_id>   # re-issue until it reports finished
tail -100 .lucidos/engine-test.log       # inspect AFTER reading the real exit
```

Each `wait` returns within 230 s. The final record's `exit_code` is the
script's own, since the engine runs the command un-piped. A "PASSED" claim
requires that `exit_code: 0` AND the `test result: ok.` line with `0 failed`
— a trimmed tail alone can lie. Cross-check both: a non-zero exit with no `test result:` line at all
usually means the *infra* failed (Docker down, port in use), not a test —
report that distinctly.

## Documented #[ignore] exceptions

Expect **`8 ignored` in the lib run and `0 ignored` in the doctest run**,
and nothing else. Any other ignored test is a real skip and must be
fixed. (`crates/lucidos-engine/tests/`, the integration binaries, has
no `#[ignore]` at all.) The eight are four different things:
**one codegen writer**, **four diagnostic printers**, **two
live-provider checks**, and **one subprocess body**.

### The one codegen writer

`codegen::tests::write_generated_outputs` writes every generated file
into `$LUCIDOS_CODEGEN_OUT`. It is not a test: `scripts/codegen.sh`
runs it to render a cache miss when no freshly built `lucidos-codegen`
is at hand, because it reuses the compile `make test` already paid for.
It satisfies clause (a): the non-ignored
`every_generated_file_renders_once_into_a_generated_folder` runs every
renderer on every suite run, so a renderer that refuses still reds it.
Generated files are build outputs, so there is no staleness guard to
pair it with: `scripts/codegen.sh` regenerates them on every use.

### The four diagnostic printers

Three live in `engine::chat::process::system_prompt::tests`. The
first two arrived with the 2026-08-07 prompt-budget trim, the third with
capability-gated tool families
(`docs/plans/2026-08-18-capability-gated-tool-families-and-two-volatile-values.md`).
The fourth, `engine::text_search::tests::timing_against_a_real_workspace`,
arrived with Text search. It times searches over the workspace named by
`TEXT_SEARCH_TIMING_WORKSPACE`, so it cannot run without one.
None asserts anything, so
none has a pass/fail to skip: they are on-demand dumps, shaped as tests
only because `cargo test` is how you run a thing in a Rust crate. The
`#[ignore]` keeps their output out of every ordinary suite run.

| `#[ignore]` printer | What asserts over the same data |
|---|---|
| `print_full_tool_schema_ranking` | `no_single_tool_schema_dominates_the_always_loaded_budget` (per-tool ceiling) and `always_loaded_context_stays_under_budget` (total budget) |
| `print_gated_array_sizes` | the same two budget guards. They measure the gate-blind engine-authored surface, so they bound every gate setting this printer dumps |
| `print_frozen_tool_contract` | nothing automated: it exists for a manual before/after diff across a prose-only trim |
| `timing_against_a_real_workspace` | the rest of `engine/text_search_tests.rs`, over fixture workspaces |

So `print_frozen_tool_contract` is the one entry that does not satisfy
clause (a) below. It stays as-is because un-ignoring it would add a
multi-thousand-character dump to every suite run while still asserting
nothing, which buys no coverage. Giving it a real guard means checking
in a frozen contract fixture and failing the suite on every deliberate
schema change; that is a policy call for the maintainer, not something
to decide from inside a test run.

### The two live-provider checks

Both are named `a_real_session_accepts_the_opening_payload`, one per voice
seam. `voice::realtime::tests` arrived with the voice talker/doer refactor
(`docs/plans/2026-08-29-a-voice-session-opens-behind-one-seam.md`) and
opens a real session on the OpenAI realtime API. `voice::live::tests`
arrived with the live-model transcriber picker
(`docs/plans/2026-09-01-voice-transcriber-picker-gains-the-live-model.md`)
and opens a real session on the live model. Each needs a credential and a
network. With no `OPENAI_API_KEY` each self-skips, printing a line rather
than failing. They are the only tests that can tell us the live provider
still accepts each seam's opening payload.

Both satisfy clause (a): the non-ignored sibling tests in the same file
pin each one's payload construction and event mapping. Nothing is lost by
keeping them out of the ordinary suite. Run one deliberately when you
touch its seam:

```sh
cargo test -p lucidos-engine --lib voice::realtime -- --ignored --nocapture
cargo test -p lucidos-engine --lib voice::live -- --ignored --nocapture
```

| `#[ignore]` live check | Non-ignored siblings over the same data |
|---|---|
| `voice::realtime::…::a_real_session_accepts_the_opening_payload` | `the_instructions_reach_the_opening_payload`, `the_talker_is_opened_with_one_tool_and_it_delegates`, `end_of_turn_is_decided_semantically`, and the rest of `voice/realtime_tests.rs` |
| `voice::live::…::a_real_session_accepts_the_opening_payload` | `the_opening_frame_names_the_model_and_starts_a_session`, `the_session_delegates_to_us_and_never_to_a_rented_backend`, `the_opening_frame_declares_no_tools`, and the rest of `voice/live_tests.rs` |

### The one subprocess body

`runtime::agent_run_marker::tests::sleeper_body` is not a test. It is the
process that `the_sweep_reaps_its_own_marker_and_nothing_else` spawns and
then reaps. macOS hides the environment of `/bin/sleep`, so the test
binary sleeps instead. That sweep test carries every assertion, which
satisfies clause (a).

**One doctest.** The doc run reports `1 passed; 0 failed; 0 ignored`.
It is a ```` ```compile_fail ```` example on `Pref` in
`crates/lucidos-engine/src/core/preference_catalog.rs`. It proves that a
flag handle on a text spec does not compile, so it is a real type-level
guard, not a skip. If the doctest count changes, update this line.

If a future change introduces a *new* `#[ignore]`, it must come with
either (a) a sibling non-ignored verification test, or (b) a script
under `./scripts/` that runs it as part of the nightly pipeline.
Otherwise, fix the underlying issue or report it as an unfixable
failure. A pure diagnostic is the third shape, and its bar is that it
stays pure: the moment it grows an assertion, it is a real test being
skipped and clause (a) applies.

Whichever shape it is, **add it to the inventory above in the same
change and bump the expected count**. A stale count is exactly how a
real skip hides: the next run sees a mismatch it cannot attribute, and
either re-derives this entire analysis from scratch or waves it
through. That is not hypothetical, it is what happened here. The two
printers went undocumented for a day and the next run had to go read
their source to find out whether the suite had been quietly holed.

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

- **Rust engine:** library / integration / doc-test passes, failures, ignored.
- **Eval crate:** passes, failures, ignored.
- **Frontend:** test files run, tests passed, tests failed, tests skipped,
  plus the `tsc --noEmit` exit status.
