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
# TS: eslint-disable sites (the *.ts/*.tsx filter keeps prose mentions out)
git ls-files '*.ts' '*.tsx' | xargs grep -n 'eslint-disable'
# TS: @ts-expect-error scale
git ls-files '*.ts' '*.tsx' | xargs grep -c '@ts-expect-error' | grep -v ':0$'
# TS: any @ts-expect-error outside a *.test.ts file (the claim below that drifted)
git ls-files '*.ts' '*.tsx' | xargs grep -l '@ts-expect-error' | grep -vE '\.test\.ts$'
# TS: the SDK typecheck no phase invokes (see Known exceptions). Restate, do not gate on it.
(cd packages/lucidos-sdk && npx tsc --noEmit -p tsconfig.json); echo "SDK EXIT: $?"
```

The currently-accepted categories, re-counted on 2026-09-16. Every Rust number
below was confirmed unchanged; the `@ts-expect-error` count moved by three.
Anything not on this list is fair game to remove and re-fix:

- **`#[allow(clippy::too_many_arguments)]`**, 81 sites across 52 files,
  by far the largest category. Internal helpers that legitimately need
  that many parameters (event constructors, runtime spawn helpers,
  scheduler entry points, `LucidosEngine::new`'s boot wiring). The
  refactor cost outweighs the lint value. Each occurrence stays in place;
  the justification is the function's role, and the strongest form of it,
  which `LucidosEngine::new` carries, is that no two parameters share a
  type, so the argument swap the lint guards against cannot compile.
  One of the 81 shares an attribute with `format_in_format_args`, which is
  the same site that entry counts. Grepping the bare form alone therefore
  reports 80 across 51 files. Both numbers here count the shared attribute.
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
  `@types/node` in project**, 565 sites across 197 files, every one of them
  test-only code: 188 `*.test.ts`, eight `*.test.tsx`
  (`components/chat/__tests__/question-card.test.tsx`,
  `components/chat/__tests__/welcome-onboarding.test.tsx`,
  `components/chat/__tests__/event-wait-surfaces.test.tsx`,
  `components/chat/__tests__/the-bubble-pulses-before-the-words.test.tsx`,
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
  and cleared on the 2026-09-15 run, and still exit 0 on 2026-09-16. The entry
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

- **Phase 4's entry chunk is 752.92 kB against its 600 kB ceiling, and the
  2026-09-16 run left it there.** `vite build` exits 0 and prints no code
  diagnostic. What fires is Rollup's size advisory against
  `chunkSizeWarningLimit: 600`, the repo's own number, whose comment in
  `crates/lucidos-app/vite.config.ts` says to code-split rather than raise
  it. Both halves of that instruction stand. This entry reports one run,
  and never licenses the next one to skip the phase.

  **Clearing it IS clean-build's job when a clean cut exists.** Look for
  that shape first: eagerly imported, never mounted on the path that pays
  for it. `94d1dd817` found it in the workspace picker and took the chunk
  from 607 kB to 585 kB. The 2026-08-24 run found one more of that shape in
  `PairingGate`, which `main.tsx` still imported statically while rendering
  it only under `IS_PICKER`. Splitting it took 664.98 kB to 657.02 kB.

  That cut costs the picker one extra round trip, which `main.tsx` explains
  at the split.

  **The on-demand surfaces can no longer close the gap, and the shortfall is
  widening.** That is new since 2026-08-19, when the same list was 36 kB
  against a 36 kB gap. It stopped there on a product call. Sourcemap
  attribution now puts the whole list at 38.37 kB, against a 152.92 kB gap,
  re-measured on 2026-09-16:

  | Surface | kB of the built chunk |
  |---|---|
  | `PermissionCard` | 10.01 |
  | `CodingAgentControlMenu` | 8.60 |
  | `ThreadFilterPanel` | 5.92 |
  | `WorkspaceSwitcher` | 4.38 |
  | `QuestionCard` | 3.93 |
  | `TodoListPanel` | 2.96 |
  | `OverflowMenu` | 2.56 |

  So paying the loading-flash trade on every permission prompt would still
  leave the advisory firing, and would now leave 115 kB of it. The next
  cut has to come out of first-paint code instead, which is a wider decision
  than this skill makes.

  Two smaller menus of the same shape sit beside them, `ThreadOverflowMenu` at
  1.26 kB and `DraftOverflowMenu` at 0.41 kB. They are left out of the table so
  its total stays comparable with the `manualChunks` measurement below, which
  covers the seven.

  Growth is diffuse rather than one mistake. The 2026-08-30 run added 21.18 kB
  and re-ran the check for the usual culprit, a component gone from lazy to
  eager. There was none, for the fifth run running. That run's growth arrived
  with a wide batch of merged feature work, not from one module.

  Every run since has re-run the check and found no lazy-to-eager regression:

  | Run | Entry chunk | Change |
  |---|---|---|
  | 2026-08-30 | 713.53 kB | +21.18 kB |
  | 2026-09-05 | 733.21 kB | +19.68 kB |
  | 2026-09-06 | 743.87 kB | +10.66 kB |
  | 2026-09-07 | 743.87 kB | 0 kB |
  | 2026-09-08 | 744.63 kB | +0.76 kB |
  | 2026-09-11 | 744.77 kB | +0.14 kB |
  | 2026-09-12 | 744.75 kB | -0.02 kB |
  | 2026-09-15 | 750.02 kB | +5.27 kB |
  | 2026-09-16 | 752.92 kB | +2.90 kB |

  The 2026-09-16 run makes thirteen in a row with no regression. Sourcemap
  attribution put 396 of our own modules in the entry chunk, three more than
  the run before, and zero `node_modules` bytes. No module sits in both the
  entry chunk and a separate one. All 32 non-test relative `import()` targets
  have their own emitted chunk. They are reached from `App.tsx`, `main.tsx`,
  `PairingGate.tsx`, `ContentPane.tsx` and `InlineForm.tsx`, and not one of
  the 32 sits in the entry chunk. The per-surface figures above were
  re-measured on this run.

  **That check is two questions, not one.** Does any module sit in both the
  entry chunk and a separately emitted chunk? And does any target of a
  `lazy(() => import(...))` sit in the entry chunk at all? The second catches
  a lazy view that a static import has quietly pulled forward. That shape
  leaves no duplicate, so the first question misses it.

  The 2026-08-29 run widened the second question to every relative `import()`
  in the app source and the SDK. Scan the whole file list, not
  a `src/**/*.ts` pathspec: git's default globbing lets `*` cross a slash, so
  `**/` costs you the top-level files, and `main.tsx` holds the lazy views.

  **Two filters make that scan answerable, and the 2026-08-30 run needed both.**
  It counted 462 relative `import()` sites, of which 420 sit in test files. A
  test importing a module dynamically says nothing about bundling, so drop
  `*.test.*`, `__tests__/` and `*.spec.ts` first. That leaves 42 sites across 9
  files.

  Then drop TypeScript's type-position `import('...').Type`, which is erased at
  compile time and reaches no bundle. All 6 apparent entry-chunk hits were that
  form, in `api/threads.ts`, `api/types.ts`, `store/actions/navigation.ts` and
  `store/store.ts`. So the real answer was zero. The 2026-09-15 and 2026-09-16
  runs both found the same four files holding 8 of them.

  **Write that second filter carefully.** "The character after the closing
  paren is a dot" also drops every `import('./x').then(...)` site, which is
  the entire value-import population. The 2026-09-15 run hit that and read 1
  site where there were 32. Keep `.then`, `.catch` and `.finally`; drop only
  the other dotted forms.

  The top is unchanged, re-measured on 2026-09-16: `icons.tsx` at 21.24 kB,
  `ThreadDrawer.tsx` at 17.97 and `store.ts` at 17.51. The three
  `thread-events/exchange*` modules add 47.12 kB between them, and the whole
  `store/thread-events/` directory puts six modules and 53.44 kB in the chunk.
  A first paint reaches all of them.

  The next tier did reorder: `PromptInput.tsx` at 15.46 kB, then
  `ChatExchange.tsx` at 15.12 and `chat/scrollState.ts` at 14.55. Those two
  swapped places since 2026-08-30, when they sat 0.05 kB apart. A margin that
  thin was never a ranking, so do not read the swap as a signal.

  **Attribute built bytes, not source bytes.** Ranking the sourcemap's
  `sourcesContent` lengths answers a different question and reorders the
  table. It put `scrollState.ts` first at 175 kB, because the file is heavily
  commented and minification strips all of that. Decode the `mappings` field
  and charge each generated segment's span to its source instead. That
  reproduces the figures above and accounts for 99.5% of the chunk.

  **Two units meet here, so do not chase the gap between them.** Vite divides
  by 1000, so the 752.92 kB it reports is 752,910 bytes, which is 735.26 KiB.
  Sourcemap columns count UTF-16 units, and every per-module figure above is
  KiB. The history table quotes vite and the attribution does not, so the two
  never sum to the same number.

  **The entry chunk carries no `node_modules` code at all**, measured again on
  the 2026-09-16 run by grouping the sourcemap's sources. Every byte of it is
  code we wrote, so no vendor-chunking idea can buy anything here.

  **The SDK's `tooltip.ts` is 6.25 kB of the entry chunk and is NOT a cut**,
  measured on the 2026-08-28 run. It looks like one. `ui.ts` pulls the whole
  module in for `disableTooltips`, and `ui` rides the `lucidos` barrel that
  `api/client/settings.ts` imports at first paint. But the host shell installs
  tooltips itself, through `hooks/useTooltip.ts`. So the bytes are used rather
  than dragged, and moving the opt-out to its own module would free none.

  The whole SDK is 14.53 kB of the entry chunk across 14 modules, measured on
  the 2026-09-16 run. That bounds the barrel: dropping every SDK byte still
  leaves the advisory firing.

  **`icons.tsx` is a barrel, and the 2026-08-25 run measured it. It is not
  the lever.** A barrel is the one shape that splits with no loading flash.
  The entry chunk holds every icon a lazy view reaches. Moving those out
  costs no round trip, because the lazy chunk already loads.

  Only five of the 64 icons are reached by lazy chunks alone. They are
  `FolderUpIcon`, `FolderIcon`, `EyeOffIcon`, `ChevronLeftIcon` and
  `ChevronRightIcon`, worth 4.17 kB of source and less once minified. The
  2026-08-30 run re-measured and found the same five. Re-measure before
  spending the churn, rather than assuming the split is free money.

  **`api/client.ts` is a second barrel, and the 2026-08-27 run measured it. It
  is not a lever either.** 25.70 kB of `api/*` lands in the entry chunk across 17
  modules, and tree-shaking still works, though it now keeps less out: only
  `mcp.ts` and `data.ts`. `webhooks.ts` joined the entry chunk by the
  2026-08-29 run, at 0.55 kB. The biggest resident is `settings.ts` at 5.64 kB,
  which first paint genuinely needs. It exports `getPreferences`,
  `setPreference` and the notification calls beside the backup, memory and
  OAuth ones. Splitting it is an API-client refactor rather than a clean cut.

  To attribute bytes, run
  `npx vite build --sourcemap --outDir dist.smap` in `crates/lucidos-app`,
  read `dist.smap/assets/index-*.js.map`, then delete `dist.smap`. Use the
  CLI flag, not `build.sourcemap` in `vite.config.ts`. A config edit can be
  left behind, and a scratch outDir keeps the served `dist/` untouched.

  Each on-demand split also needs a preparatory move, because the panels
  export first-paint state from the same module as the component:
  `hooks/useThreadsHeaderState.ts` imports `filterButtonState` from
  `ThreadFilterPanel`, and `PromptInput.tsx` imports
  `codingAgentMenuOpenRequest` from `CodingAgentControlMenu`. The signal
  has to move to its own module first or the component stays eager.

  **Do not reach for `manualChunks` here.** Measured: forcing those eight
  components into a named chunk drops the entry chunk to 243.26 kB, which
  looks like a fix and is not one. Rollup relocates the shared core into
  the named chunk, which the entry statically imports, so first paint
  downloads the same bytes in two files. It clears the advisory while
  changing nothing the advisory is about.

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
