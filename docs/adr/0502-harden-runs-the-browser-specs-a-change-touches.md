# 0502: /harden runs the browser specs a diff can break, on GitHub; amends 0382

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0382](0382-e2e-runs-on-github-from-a-stripped-temp-branch.md)

## Context

ADR 0382 left one question open: whether `/harden` should run e2e through
GitHub mode.

Three of the main check's first twelve reds were a spec that missed a UI
rename: the Home rail, the dashboard rail and the Canvas title. Typecheck and
Vitest passed each change. Only the main check runs the browser specs, so each
rename landed and then failed on `main`.

## Decision

**`/harden` runs the browser specs its diff can break, through
`./scripts/e2e-browser.sh`, which hands the run to GitHub by default.**

A new suite row, `e2e-touched`, selects a spec when either holds:

- the branch changes the spec file;
- a removed line of `crates/lucidos-app/src/` held a string literal that no
  longer appears anywhere under that directory, and the spec names it. The
  spec may name it directly, or call an e2e helper export that does.

A literal with spaces also counts word by word, so one class name leaving a
class list is seen. `touched_specs.sh` holds the selector. A diff with no such
spec selects no e2e at all.

The run uses `-f` once per spec, so `e2e-browser.sh` and the GitHub driver now
take `-f` more than once. A GitHub precondition that fails falls back to a
local run, as every e2e script does. The row's exit code is its verdict.

The fast tier drops the row: the integrating parent, which runs the full
tier, runs it once.

## Rationale

The cheapest place to catch a missed rename is before it lands. The diff
already says which words left the UI, and a spec that names one of them is the
spec at risk. Running only those specs adds minutes to a frontend change,
where the whole suite adds an hour.

A helper export is the usual hiding place: `hideHomeRail` held the label, and
fifteen specs called it. Reading the export that holds the word, then the specs
that call it, finds those without selecting every spec that imports the
helper file.

## Consequences

- A frontend `/harden` waits for a GitHub run of a few specs, likely 8 to 15
  minutes, beside suites that already run in parallel.
- The selector misses a rename whose old name survives as a prefix of another
  name in `src/`. Its error is a missed spec, never a spec run that need not
  run. The main check stays the backstop for the miss.
- A change to an e2e helper alone selects nothing. A helper edit that breaks a
  caller is the helper's author's to run.
- `e2e-browser.sh` and `e2e-github.sh` take `-f` more than once.

## Alternatives considered

- **Run every spec in `/harden`.** About an hour on GitHub, for a risk that a
  few specs carry.
- **Select every spec that imports a helper file that holds the word.** The
  helper file is imported by nearly every spec, so this is the whole suite.
- **Select by a changed `src/` path.** There is no map from a component to the
  specs that drive it, and building one by hand rots.
- **Keep the main check as the only gate.** That is the cost this ADR removes:
  a fix thread per missed rename.

## Amendment: coverage selects specs too

The literal rule misses a behaviour change that renames nothing. Two reds on
the main check each broke about ten specs that way. "Select by a changed `src/`
path" was rejected above because nobody keeps a map by hand. The main check now
builds one by measurement, so it cannot rot.

- **What is recorded.** The main check's GitHub run builds the frontend with
  `LUCIDOS_E2E_COVERAGE=1`, which makes Vite emit hidden sourcemaps. On
  Chromium (`chromium` and `mobile`, never WebKit) an auto fixture records each
  test's JS coverage (`e2e/coverage.ts`). It maps every executed function to its
  source file. It never maps the script root, which spans every module in a
  chunk.
- **Where it lives.** Each shard writes per-spec files into its uploaded output.
  The main check merges them into the *coverage map*,
  `<git common dir>/lucidos/e2e-coverage-map.json`, so every worktree reads one
  map. It replaces the map atomically, and only when every Chromium shard
  reported.
- **How `/harden` uses it.** `touched_specs.sh` adds every spec whose recorded
  files include a path the branch changes. It only adds: the literal rule still
  runs. A missing or unreadable map adds nothing, and the `e2e-touched` log says
  which.
- **Startup files are ignored.** A file that 90% or more of the recording specs
  ran is app startup, and selects no spec by coverage. The first measured map
  had 315 such files out of 766. Replaying 80 frontend commits without the
  filter, 47 selected about 241 specs, nearly the whole suite. With it the
  median commit adds 3.5 specs and the 75th percentile 29. A startup change
  keeps the literal rule and the main check.
- **Recording never changes a test's outcome.** Every failure in the recorder is
  a warning, and only the main check turns it on. `LUCIDOS_E2E_COVERAGE=1` on an
  ordinary GitHub run records into its output and leaves the map alone.

Consequences:

- A change to a module most specs run, short of startup, can still select
  most of the suite. No cap applies: that change can break most of the suite.
- A module whose code runs only at load (top-level constants, say) is never
  recorded, since that code sits in the script root. The literal rule still
  covers its strings.
- Two specs import `test` from `@playwright/test` rather than the fixtures, and
  record nothing. So do Claude Code specs, which the main check does not run.
- The map trails `main` by one main check. A spec added since then has no entry.

Alternatives considered:

- **Map the script root as well.** Every module in the chunk would count, so
  every spec would select every change.
- **Count startup files too.** The plan's first shape. Most frontend changes
  then ran nearly the whole suite in `/harden`, an hour where the rest of it
  takes minutes.
- **Commit the map to the tree.** Every main check would then land a commit, and
  every worktree would merge it.
