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

2. **ShellCheck + rustfmt + clippy, deny warnings, all targets, all features:**
   ```sh
   make lint
   ```
   `make lint` IS the canonical lint gate: `lint-shell` (ShellCheck over every
   tracked `*.sh`), then `lint-fmt` (`cargo fmt --all --check`, which fails if
   any tracked Rust file is not rustfmt-clean; `make fmt` is the fix), then
   `lint-rust` (clippy). The clippy flag list lives in
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

The repo carries annotations that look like silencers but are not. Each
one must have a comment explaining why it is there. If you find one
without a justification, fix the underlying code instead and remove the
annotation.

**Re-derive the inventory; do not trust the list below on sight.** It is
a snapshot of file names, and file names move. By 2026-08-03 it had
drifted three ways at once: a whole `#[allow(deprecated)]` category was
missing, `UrlPreviewInline.tsx` was still listed for an `eslint-disable`
it no longer carried, and three of the four real `eslint-disable` sites
were absent. Nothing had ever checked it. So a clean-build run
regenerates the real inventory first and updates this section in the same
run whenever the two disagree:

```sh
# Rust: every allow attribute, grouped by lint
git ls-files '*.rs' | xargs grep -ho '#\[allow([^)]*)\]' | sort | uniq -c | sort -rn
# Rust: allows wrapped in cfg_attr, which the grep above cannot see. Must print nothing.
git ls-files '*.rs' | xargs grep -n 'cfg_attr(.*allow('
# TS: eslint-disable sites (the *.ts/*.tsx filter keeps prose mentions out)
git ls-files '*.ts' '*.tsx' | xargs grep -n 'eslint-disable'
# TS: @ts-expect-error scale
git ls-files '*.ts' '*.tsx' | xargs grep -c '@ts-expect-error' | grep -v ':0$'
# TS: any @ts-expect-error outside a *.test.ts file (the claim below that drifted)
git ls-files '*.ts' '*.tsx' | xargs grep -l '@ts-expect-error' | grep -vE '\.test\.ts$'
# TS: the SDK typecheck no phase invokes (see Known exceptions). Restate, do not gate on it.
(cd packages/lucidos-sdk && npx tsc --noEmit -p tsconfig.json); echo "SDK EXIT: $?"
```

The currently-accepted categories, re-counted on 2026-09-25. Every Rust
allow and every `eslint-disable` site was unchanged, and the cfg_attr grep
printed nothing. Only `@ts-expect-error` moved, up 21 with the test suite.
The 2026-09-24 run removed five `cfg_attr(..., allow(dead_code))`
silencers from the gateway's slowness watcher, which the first grep above
had missed. The cfg_attr grep now covers that form.
Anything not on this list is fair game to remove and re-fix:

- **`#[allow(clippy::too_many_arguments)]`**, 82 sites across 52 files,
  by far the largest category. Internal helpers that legitimately need
  that many parameters (event constructors, runtime spawn helpers,
  scheduler entry points, `LucidosEngine::new`'s boot wiring). The
  refactor cost outweighs the lint value. Each occurrence stays in place;
  the justification is the function's role, and the strongest form of it,
  which `LucidosEngine::new` carries, is that no two parameters share a
  type, so the argument swap the lint guards against cannot compile.
  One of the 82 shares an attribute with `format_in_format_args`, which is
  the same site that entry counts. Grepping the bare form alone therefore
  reports 81 across 51 files. Both numbers here count the shared attribute.
- **`#[allow(dead_code)]`**, 4 sites: the `SpawnTrigger` taxonomy enum
  (`agent_session/spawn_dispatcher.rs`, one attribute on the enum) and test
  scaffolding (`thread_lifecycle_tests/scenario_tests.rs`,
  `change_ops_engine_origin_stamping_tests.rs`, `tools/plugins/mod.rs`).
  One-line comment required at each site. The count was 5 until 2026-08-28,
  when the second `spawn_dispatcher.rs` site turned out to be gone.
- **`#[allow(clippy::large_enum_variant)]`**, 1 site: `BusEvent` in
  `engine/event_bus/mod.rs`. The variant size is dominated by the inner
  event payload; boxing every variant to flatten the enum would hurt
  every hot-path emit.
- **`#[allow(clippy::format_in_format_args)]`**, 3 sites, all in
  `bin/populate_memory.rs` test-data generation, where readability of the
  nested `format!` calls trumps the lint. One of the three shares an
  attribute with `too_many_arguments`, so a naive per-lint count reports
  only two.
- **`#[allow(deprecated)]`**, 1 site: `lucidos-app/src/notifications.rs`,
  on `activateIgnoringOtherApps:`. Its replacement, the parameterless
  `activate()`, exists only on macOS 14+ while the app targets macOS 11+
  (see `tauri.conf.json`), so the deprecated cross-version call is the
  correct one to keep.
- **`// @ts-expect-error`, Node APIs available at runtime via Vitest, no
  `@types/node` in project**, 709 sites across 245 files, every one of them
  test-only code: 235 `*.test.ts`, nine `*.test.tsx`
  (`components/chat/__tests__/question-card.test.tsx`,
  `components/chat/__tests__/welcome-onboarding.test.tsx`,
  `components/chat/__tests__/event-wait-surfaces.test.tsx`,
  `components/chat/__tests__/the-bubble-pulses-before-the-words.test.tsx`,
  `components/chat/__tests__/pure-voice-draws-no-turn-header.test.tsx`,
  `components/picker/__tests__/pairing-code-boxes.test.tsx`,
  `components/settings/__tests__/mcp-servers-page.test.tsx`,
  `components/shared/__tests__/apps-glyph-single-source.test.tsx` and
  `components/shared/__tests__/system-attention-badge.test.tsx`), and one
  Vitest-only helper, `styles/__tests__/css-rule-helpers.ts`, which imports
  `node:fs` for two of the sites. The
  expectation is real: TS does not know about Node globals, but Vitest
  provides them. Adding
  `@types/node` to the project would contaminate the browser type-graph.
  Only the first site in a file spells the reason out; the rest say
  `same`, which counts as justified because it points at an explanation
  in the same file. The `.test.tsx` sites are why the regeneration
  snippet above filters on `\.test\.ts$` and prints the leftovers: the
  older wording asserted there were none, and nothing checked it. The
  count grows with the test suite, so treat a mismatch here as ordinary
  drift to restate rather than as a finding, and check only that every
  leftover the snippet prints is still test-only code. The helper is why
  that says test-only code rather than a test file: it lives under
  `__tests__/` and nothing else imports it.
- **`// eslint-disable-next-line`**, 9 sites across 5 files and 5 rules:
  `react-hooks/exhaustive-deps` in `hooks/useLoadableFetch.ts` (the deps
  list is intentionally narrow), `no-console` five times in
  `utils/perfProbe.ts` (permanent console-based perf instrumentation,
  whose module doc says exactly that), `@typescript-eslint/no-implied-eval`
  in `sw.test.ts` (the test evaluates service-worker source through `new
  Function`), `@typescript-eslint/no-explicit-any` in
  `components/chat/__tests__/prompt-vdom-keys.test.ts` (a `VNode<any>`
  alias for VDOM-key assertions), and `no-new-func` in
  `__tests__/cold-start-fast-path.test.ts`, which runs the inline
  cold-start program through `new Function` the way `sw.test.ts` runs the
  service worker. **No eslint config ships in this repo**,
  so phase 5 always skips and none of these suppress anything today. They
  are kept rather than deleted because each would be correct the moment a
  config lands. Do not "clean them up" on the grounds that they are
  currently inert.

The audit rule: every `#[allow(...)]`, `@ts-expect-error`, or
`// eslint-disable*` MUST sit directly under context that explains it,
either a `///` doc comment whose content makes the lint's allowance
self-evident (e.g. a doc that names the schema columns the wide function
mirrors), or an explicit `//` line that explains why the lint applies
here. A bare annotation with no comment above is forbidden. If you find
one, fix the code and remove the annotation.

Check that mechanically rather than by eye. This prints every Rust allow
whose preceding non-attribute line is not a comment, and must print
nothing:

```sh
for f in $(git ls-files '*.rs'); do awk -v F="$f" '
  { l[NR]=$0 }
  END { for (i=1;i<=NR;i++) if (l[i] ~ /#\[allow\(/ && l[i] !~ /\/\//) {
      j=i-1; while (j>=1 && l[j] ~ /^[[:space:]]*#\[/) j--
      if (!(j>=1 && l[j] ~ /^[[:space:]]*(\/\/|\*|\/\*)/)) printf "%s:%d:%s\n", F, i, l[i]
    } }' "$f"; done
```

### Known exceptions

Where "When to give up" (below) sends an unfixable finding. Kept inside
`## Documented exceptions` so the two inventories read as one list.

- **`packages/lucidos-sdk`'s own `npm run typecheck` RUNS now, and exits 0.
  No phase above invokes it.** Recorded 2026-08-04 as unrunnable, reopened
  and cleared on the 2026-09-15 run, and still exit 0 on 2026-09-23. The entry
  stays because the coverage gap it describes is still real: no phase reads the
  SDK's test files.

  The old blocker is gone. It was a resolution gap: the SDK's test files
  import `vitest`, which only `crates/lucidos-app` declares, and the lockfile
  put it at `crates/lucidos-app/node_modules/vitest`. The SDK never reaches
  that path. The vitest path-traversal bump (`fix(deps): npm audit fix, vitest
  path-traversal advisory`) hoisted it to the root `node_modules/vitest`,
  which every member resolves through. So a security bump closed the gap as a
  side effect, and the 16 `TS2307: Cannot find module 'vitest'` errors are
  gone.

  **Running it then exposed 5 real type errors**, which the 2026-09-15 run
  fixed at source: two untyped `vi.fn` mocks whose `mock.calls` tuples were
  empty, in `src/_fetch.test.ts` and `src/openExternal.test.ts`. Both now
  carry the repo's `vi.fn<Signature>()` idiom, and the two casts to
  `typeof fetch` are gone with it. The 29 tests in those files still pass.

  **Do not "fix" a future failure here by trimming the SDK tsconfig's
  `include`.** That would drop the test files from type checking rather
  than type check them.

  Here is the coverage today, so the gap is neither overstated nor
  understated. The SDK's **non-test** sources are type checked
  transitively by phase 3, because
  `node_modules/@lucidos/sdk` symlinks to the package and its `types` field
  points at `src/index.ts`. The SDK's **test** files are read by no phase,
  though they do execute: the app's `vite.config.ts` adds
  `../../packages/lucidos-sdk/src/**/*.test.ts` to the vitest include list,
  so `/run-tests` runs them.

  **`src/worker/` is the hole in that transitive coverage**, found on the
  2026-08-29 run. `src/index.ts` never imports it, so phase 3 never reaches
  it. `sseWorker.build.mjs` bundles it with esbuild into
  `src/generated/sse-worker.js`, which the engine `include_str!`s and serves,
  and esbuild type checks nothing. So it is shipping code that no phase reads.
  It had one real error, `SharedWorkerGlobalScope` undeclared, which that run
  fixed by adding `WebWorker` to the SDK's `lib`. The SDK typecheck does reach
  it, which is why the regeneration block above now runs that command.

  **Promoting it to a sixth phase is the maintainer's call, not this
  skill's.** It would widen the nightly gate. It also runs only because npm
  hoists a dependency the SDK does not declare. Declaring `vitest` as a
  devDependency of `packages/lucidos-sdk` would make that solid. That is a
  dependency plus lockfile change (ADR 0020), and belongs in its own commit.

- **Phase 4's entry chunk is no longer an exception.** It sat at 857.58 kB
  against its 600 kB ceiling until 2026-09-26, when the first-paint split
  took it to 488 kB (ADR 0288). The entry chunk is now the data layer and
  startup. The UI is the shell chunk, loaded beside it under the boot splash.

  `entryChunkBudget` (`crates/lucidos-app/vite/entryChunkBudget.ts`) now
  FAILS a single-shot `vite build` whose entry chunk passes
  `chunkSizeWarningLimit`, naming the measured size. So this phase fails
  outright rather than printing an advisory, and never lands here again.

  When it fires, move the next thing the first frame does not need behind a
  dynamic import. Never raise the number. The usual cause is a new static
  import from the data layer into UI code. It pulls that code and its imports
  back into the entry.

  - **Attribute built bytes, not source bytes.** Run
    `npx vite build --sourcemap --outDir /tmp/<dir>` in `crates/lucidos-app`
    and decode the entry map's `mappings`, charging each segment to its
    source. Use a scratch outDir, never a `build.sourcemap` config edit.
  - **Ask the two regression questions.** Does any module sit in two chunks?
    Does any `import()` target sit in the entry chunk?
  - **Do not reach for `manualChunks` for our own code.** It moves shared code
    into a chunk the entry imports statically, so first paint loads the same
    bytes in series.

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
