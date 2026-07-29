---
name: clean-build
description: Use when asked for a "clean build", "fix warnings", "clippy clean", "lint", "no warnings", or any variant — enforces warnings-as-errors across Rust (rustc + clippy, all targets, all features) and the frontend (tsc + vite build, plus eslint if config exists). Fixes every warning at source; never allowlists, `#[allow]`s, `@ts-ignore`s, or `// eslint-disable`s to silence.
---

# Clean build — warnings-as-errors gate

This skill is the build-cleanliness gate. Sibling skill `/run-tests` covers
test *correctness* — do not duplicate it here. This skill cares only about
warnings, lints, and compiler / bundler diagnostics.

Every command below must exit 0 with zero warnings printed. Any non-zero
exit, any warning, any clippy lint = FAIL.

## Commands

Run all five from the repo root. If any one fails, the skill FAILED. Do
not stop at the first failure — collect them all, fix at the source, and
re-run until every phase is clean.

1. **Rust build, deny warnings (all targets):**
   ```sh
   RUSTFLAGS="-D warnings" cargo build --release --all-targets
   ```
   Covers lib, bins, tests, and examples — catches `dead_code` / `unused`
   that a release-only build misses.

2. **ShellCheck + clippy, deny warnings, all targets, all features:**
   ```sh
   make lint
   ```
   `make lint` IS the canonical lint gate: `lint-shell` (ShellCheck over every
   tracked `*.sh`) then `lint-rust` (clippy). The clippy flag list lives in
   exactly one place — `CLIPPY_FLAGS` in the `Makefile`, where each flag is
   justified inline. This skill and `/harden` Phase 4.5 — the gate that runs
   per change — both *call* `make lint` rather than restating the flags, so it cannot mean
   two different things in two places (it used to: this skill and the Makefile
   disagreed until 2026-07-26). Never paste a literal `cargo clippy --…` here —
   not even in prose. Change the Makefile.

3. **Frontend type check:**
   ```sh
   cd crates/lucidos-app && npx tsc --noEmit
   ```

4. **Frontend production build:**
   ```sh
   cd crates/lucidos-app && npm run build
   ```
   `vite build` surfaces warnings that `tsc` misses — unused imports per
   the bundler, dynamic-import collisions, CSS issues. `npm run build`
   internally re-runs `tsc --noEmit` before `vite build`; that is
   intentional — keep both phases in this skill so a future change to the
   `build` script does not silently drop type checking.

5. **Eslint, if config present:**
   ```sh
   cd crates/lucidos-app && \
     ( test -f eslint.config.js || test -f eslint.config.mjs \
       || test -f eslint.config.cjs || test -f eslint.config.ts \
       || test -f .eslintrc.js   || test -f .eslintrc.cjs \
       || test -f .eslintrc.json || test -f .eslintrc.yml \
       || test -f .eslintrc.yaml || test -f .eslintrc ) \
     && npx eslint . --max-warnings=0 \
     || echo "[clean-build] no eslint config — skipping"
   ```
   Skip cleanly when no config exists; this is not a failure.

## Reading exit codes honestly — beware piped output

Pipes lie about exit codes, and the Bash background-task harness will
silently report the pipe-tail exit as the command's exit. A real failure
mode observed in this skill: clippy emitted 4 compile errors, but the
background-task completion notification reported `exit_code 0` because
the invocation was `cargo clippy ... 2>&1 | tail -300` — `tail` exited
0 even though `cargo` exited non-zero, and that 0 propagated. The agent
nearly reported PASSED on a failing run.

**The rules:**

- Never trust a `pipe | tail` exit code. Zsh / Bash report the exit of
  the *last* command in the pipeline, not the first.
- For each cargo / npm phase, redirect to a log file and capture `$?`
  directly:

  ```sh
  make lint > /tmp/clippy.log 2>&1; echo "EXIT: $?"
  grep -cE "^(error|warning)" /tmp/clippy.log
  ```

- If you must pipe for live tailing, use zsh's `${pipestatus[1]}` (or
  bash's `${PIPESTATUS[0]}`) and echo it after the pipeline. Print it
  on its own line so the harness summary cannot mask it.
- When the Bash tool's `run_in_background: true` returns its summary,
  cross-check by reading the captured log for `^error` / `^warning`
  lines — those are the source of truth for clippy/cargo. Vite and tsc
  use different prefixes; `grep -iE "(error|warning)"` catches both.
- A "phase passed" claim requires BOTH: (a) the echoed `EXIT: 0` you
  printed yourself, AND (b) zero matches for `^error` / `^warning` in
  the captured log. Either alone can lie.

## The default-deny stance — fix at source

- NEVER allowlist a warning.
- NEVER add `#[allow(...)]` to silence a clippy lint.
- NEVER add `// eslint-disable` / `// eslint-disable-next-line`.
- NEVER add `@ts-ignore` or `@ts-expect-error` just to silence.
- NEVER widen a clippy / eslint config rule to mute a real signal.

Fix the underlying code. If a lint is *genuinely* wrong for this
codebase (provably wrong for the whole repo, not just inconvenient for
one file), that is a clippy or eslint **config** change — but the
default-deny stance must hold and the config change is its own commit
with a one-line justification.

## Documented exceptions

The repo carries a small number of attributes that look like silencers
but are not. Each one must have a one-line comment explaining why it is
there. If you find one without a justification, fix the underlying code
instead and remove the annotation.

The currently-accepted categories — anything not on this list is fair
game to remove and re-fix:

- **`#[allow(clippy::too_many_arguments)]`** on internal helpers that
  legitimately need that many parameters (event constructors, runtime
  spawn helpers, `LucidosEngine::new`'s boot wiring). The refactor cost
  outweighs the lint value. Each occurrence stays in place; the
  justification is the function's role — and the strongest form of it,
  which `LucidosEngine::new` carries, is that no two parameters share a
  type, so the argument swap the lint guards against cannot compile.
- **`#[allow(dead_code)]`** on test scaffolding (`scenario_tests.rs`,
  `plugins.rs` test helpers) and intentionally-unused dispatcher
  variants (`spawn_dispatcher.rs`). One-line comment required at each
  site.
- **`#[allow(clippy::large_enum_variant)]`** on `BusEvent` — the variant
  size is dominated by the inner event payload; boxing every variant to
  flatten the enum would hurt every hot-path emit.
- **`#[allow(clippy::format_in_format_args)]`** in `populate_memory.rs`
  test-data generation where readability of the nested `format!` calls
  trumps the lint.
- **`// @ts-expect-error — Node APIs available at runtime via Vitest,
  no @types/node in project`** in `crates/lucidos-app/**/*.test.ts`.
  The expectation is real: TS does not know about Node globals, but
  Vitest provides them. Adding `@types/node` to the project would
  contaminate the browser type-graph.
- **`// eslint-disable-next-line react-hooks/exhaustive-deps`** in
  `UrlPreviewInline.tsx` and `useLoadableFetch.ts` where the deps list
  is intentionally narrow.

The audit rule: every `#[allow(...)]`, `@ts-expect-error`, or
`// eslint-disable*` MUST sit directly under context that explains it —
either a `///` doc comment whose content makes the lint's allowance
self-evident (e.g. a doc that names the schema columns the wide function
mirrors), or an explicit `//` line that explains why the lint applies
here. A bare annotation with no comment above is forbidden. If you find
one — fix the code and remove the annotation.

## Out of scope

- Test correctness — `/run-tests` and `/run-e2e` own that.
- Runtime errors / panics — `/bugfix` and `/systematic-debugging` own that.
- Production-only build flavors (Tauri bundles, Docker, signing) —
  those have their own pipelines.

## When to give up

Only stop if a warning is genuinely unfixable from this session:

- An upstream crate emits the warning from inside its own `macro_rules!`
  expansion and there is no `#[allow]` site we control.
- The toolchain has a known false-positive that the next stable release
  already fixes.
- A clippy lint requires a breaking public-API change that is out of
  scope for the current branch (rare — usually the fix is local).

In those cases, document the exact warning, the affected file/line, and
the upstream issue link in a "Known exceptions" addendum under this
file's `## Documented exceptions` heading — never silently `#[allow]` it
in the code.

## Reporting

Final status: **PASSED** or **FAILED** (FAILED if any phase emitted a
warning or returned non-zero).

For each phase, report:

- Phase name (`cargo build`, `make lint`, `tsc`, `vite build`, `eslint`).
- Exit code.
- Number of warnings emitted (must be 0).
- Number of errors (must be 0).
- For `eslint`: SKIPPED if no config was present.

If FAILED, list every warning with `file:line` and the lint name so the
next iteration can target them directly.
