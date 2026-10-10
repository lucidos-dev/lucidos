# 0353: UI-only listeners install from shell startup; the entry chunk warns past 90% of its budget; the build-watch reports an overrun through a parent-bound signal

- **Status**: Accepted
- **Date**: 2026-10-03

## Context

ADR 0288 took the entry chunk from 858 kB to 488 kB. One week later it was
600.23 kB, over its 600 kB budget, and the dev frontend stopped building.

Two things went wrong at once.

- **The budget blocked the dev build-watch.** ADR 0288 had the guard report
  rather than fail under `vite build --watch`, keyed on Rollup's `watchMode`.
  The build-watch had since moved to a fresh one-shot `vite build` per change,
  so `watchMode` was always false there. One overrun stranded every later
  Apply, which is the failure ADR 0288 meant to rule out.
- **The growth was diffuse.** Sourcemap attribution against the split showed
  +112 kB: 71 kB in new modules, 41 kB in grown ones. A large share was UI-only
  code reached from the data layer through one narrow import. One store action
  read a single constant out of the linkifier. Effects that only read the voice
  call signal pulled in the call runner. Client startup and `main.tsx` installed
  listeners that only a drawn UI can need.

## Decision

1. **Shell startup.** `startShell()` (`src/shellStartup.ts`) runs in
   `main.tsx`'s shell loader, after the shell chunk resolves and before
   `<App/>` first renders. It installs the listeners only a drawn UI can need:
   the three diagnostic probes, the app keybindings sync, the app frame
   messages, and the pending upload and unsent message restores.
2. **A narrow import never carries a UI module into the data layer.** When the
   store needs one export of a UI module, that export moves into its own small
   module.
3. **A soft line at 90 % of the budget.** Between 540 kB and 600 kB every build
   warns, naming the headroom left. Past 600 kB single-shot builds fail.
4. **The build-watch names itself.** It spawns Vite directly and sets
   `LUCIDOS_DEV_BUILD_WATCH` to its own pid. The guard reports instead of
   failing only when that value is its parent's pid. The watcher records the
   measurement in `.build-watch/status.json` and alerts on each budget edge.

## Rationale

**Shell startup has no window.** Nothing it installs can matter before the
shell renders. Nobody presses a composer button, types, opens an app frame or
reads a restored message before `<App/>` exists. And `lazyComponent` renders
only what the loader returns, so the installs always run first. Both restores
already waited on the first served thread list, a network round trip. An idle
prefetch would have been wrong here: it starts after the splash lifts, and the
restores and probes must be live on the first frame.

**The soft line puts the warning where it is read.** A warning alone rotted for
weeks before ADR 0288, so it cannot be the only signal. But the nightly clean
build already fails on any `vite build` warning, while Applies keep landing. So
creep turns the nightly red with 60 kB still to spend, rather than blocking the
next Apply with none.

**The signal is bound to the parent, not just present.** An environment
variable alone leaks: exported in a shell, inherited by a script. Only a
process that names itself and then spawns Vite directly can match, and only the
build-watch does that. `/harden`, e2e, the nightly and the release all run Vite
through `npx`, so they stay strict whatever they inherit.

## Consequences

- The entry chunk measured 509.13 kB (162.07 kB gzipped) after the change, from
  600.23 kB (191.95 kB gzipped). The shell chunk grew from 432.36 kB to
  522.46 kB, which is the point: those bytes now load beside the startup
  fetches, not ahead of them. First paint loads the same total.
- A new listener that only the UI needs belongs in `startShell`, not in client
  startup or `main.tsx`. `store/startup.test.ts` pins the current set to it.
- The overrun is visible three ways under the build-watch: the build log,
  `status.json`'s `entryChunk`, and one notification per edge.
- Attribution over budget needs the guard to report. The clean-build skill
  carries a recipe that names its own process as the parent.

## Alternatives considered

- **Raise the budget.** Rejected by the maintainer, as in ADR 0288.
- **A per-module allowlist test of the entry chunk.** It would fire on every new
  store module, so it gets rubber-stamped. It also conflicts across concurrent
  branches, since every branch edits the same list.
- **A committed size ratchet.** The same branch conflicts, and it hides growth
  below the step it allows.
- **Key report-only on a bare environment variable.** One leaked export would
  quietly loosen `/harden` and the release.
- **Idle-prefetch the moved code.** It starts after the splash lifts, so a
  restore card or a probe could miss the first frame.
- **Split `themeParts`.** It validates theme part tokens on the boot paint path,
  so moving it would flash the theme.
