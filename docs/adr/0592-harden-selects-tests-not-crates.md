# 0592: /harden selects the tests a test-only include feeds, not the crate

- **Status**: Accepted
- **Date**: 2026-10-28
- **Amends**: [0292](0292-harden-early-suite-run.md). The selection it moved
  into the script now narrows a test-only include to its tests, and the
  frontend suites leave the cargo lane.

## Context

A four-file frontend fix paid more than 25 minutes in `/harden` Phase 4.5. One
of the four files was `system-knowhow/plugins.md`. Two `#[test]` fns
`include_str!` it, so it was a compile input. ADR 0292's rule made every
compile input select the full `rust` suite (1277 s). The `ts` suite then
waited for `rust` to finish, because `ts`, `vitest` and `knowhow-generated`
sat in the one cargo lane.

Plan: `docs/plans/2026-10-28-harden-suites-proportional-to-diff.md`.

## Decision

1. **A test-only include selects its tests, not the crate.** A path is
   test-only when every `include_str!`/`include_bytes!` site that names it sits
   inside a fn with a `#[test]`-style attribute. Such a fn only compiles under
   `cfg(test)`, so no production build can break. In `lucidos-engine` the path
   selects `engine-filtered`, with each enclosing test fn as a filter. In
   `lucidos-cli`, `lucidos-gateway` and `lucidos-app` it selects that crate's
   suite.
2. **Every other site keeps selecting `rust`.** That covers a module-level
   `const` (one inside `mod tests` too), a helper fn, and a site the scan
   cannot resolve. It also covers a crate with no suite of its own, and a path
   a `crates/*build.rs` names.
3. **`compile-inputs` keeps every input.** The new `test-inputs` state file only
   refines the selection. A test-only include still voids an early result.
4. **`engine-filtered` checks each filter.** The verdict FAILs and names any
   filter that matched no passing test, so a dead filter cannot hide behind a
   live one.
5. **Codegen runs once, as the first step of the cargo lane.** When a frontend
   suite is selected, the lane runs `./scripts/codegen.sh` behind the memory
   gate. `ts`, `vitest` and `knowhow-generated` wait for it, then run outside the
   lane with commands that never compile. A failed step fails each of them, and
   a refused gate (exit 72) voids each of them.
6. **`check-knowhow-generated.sh --cached-only` compares with the cached
   rendering.** The cache key covers every codegen input, so a hit is
   byte-identical to a fresh render. A miss fails closed instead of compiling.
7. **Timings record the queue.** Each suite carries `queued`, the second it was
   handed to its lane, and the payload carries a `codegen_step` entry. The
   step's state files are `codegen-step.*`, because the hermetic `codegen` suite
   already owns `codegen.*`.

## Rationale

The rule finds the enclosing fn by indentation, which rustfmt makes reliable
and `make lint` enforces. On code rustfmt would reject, the scan can only fail
to find a test fn, and the path then selects `rust`. So the heuristic fails
safe.

A `const` in `mod tests` stays production on purpose. Several tests may read
it, and the scan would have to resolve every use to name them. Moving the
include into the one test that reads it is the cheap fix, and
`voice/language.rs` now does that for `LocaleSection.tsx`.

The cargo lane exists so two cargo processes never run in one worktree (ADR
0351). The frontend suites only reached cargo through `codegen.sh`. Running
it once at the head of the lane keeps that guarantee and lets the frontend
suites run beside the engine tests.

## Consequences

- A docs or frontend edit that only a test reads costs that test, not 20
  minutes of engine suite.
- The frontend suites no longer queue behind `rust`.
- A production include still selects `rust`. That is the deliberate *no*:
  `CHANGELOG.md`, `release-notices.toml` and `.claude/skills/lucidos-cli/SKILL.md`
  ship inside the engine binary, so an edit to them can break the build.
- Every full round runs the codegen step, because `knowhow-generated` is always
  selected. Only a retry of non-frontend suites skips it.

## Alternatives considered

- **Treat every `*_tests.rs` file as test-only.** It is simpler, but a module
  `const` there is shared by several tests, and the scan could not name them.
  The fn-level rule is exact.
- **Keep the frontend suites in the lane.** Correct, but they wait for the
  slowest cargo suite, which was most of the triggering 25 minutes.
- **Let each frontend suite run `codegen.sh` itself, outside the lane.** On a
  cache miss two of them compile at once beside `rust`, which is the overlap
  ADR 0351 forbids.
