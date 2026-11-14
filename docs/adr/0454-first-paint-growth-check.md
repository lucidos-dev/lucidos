# 0454: Every change reports its first-paint growth against a build of its merge base; growth over 2 kB raw needs a First-paint-growth trailer; amends 0436

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

ADR 0436 budgets the first-paint set at 1400 kB raw, with a soft line at
1260 kB. The set grows about 24 kB a day, so a trim buys about a day before the
nightly clean build warns again. A budget says when the set is too big. It says
nothing about which change made it so, and by then the growth is spread over
dozens of changes nobody looked at.

We measured every Apply of the five days before this decision. Apply
fast-forwards main to the branch tip, so main's reflog holds one entry per
change: 251 in all. 171 touched first-paint inputs, and each was built and
measured with the ADR 0436 set:

| | Value |
|---|---|
| Growing changes | 97 (14 shrank, 60 flat) |
| Median growing change | 0.39 kB raw |
| 90th percentile growing change | 2.29 kB raw |
| Changes over 2 kB | 13, carrying 94.0 of 134.6 kB grown |
| Changes over 5 kB | 8, carrying 81.4 kB |
| Largest | +23.13 kB raw (+8.04 kB gzip) |

## Decision

1. **Every change reports its growth**: the set at HEAD minus the set at its
   merge base with main, raw, with gzip beside it.
2. **The baseline is a build of the merge base.** `first-paint-growth.mjs`
   extracts that tree with `git archive` and builds it with its own Vite config.
   Both builds measure through the checking tree's own `chunkBudget.ts`. A
   wrapper config adds it to each tree's Vite config in report mode, and it
   emits the measurement it judged as `first-paint-report.json`.
3. **Growth over 2 kB raw needs a reason**: a `First-paint-growth: <why>`
   trailer on a commit of the change. Without one the gate fails. A shrink
   needs nothing and shows as negative.
4. **No baseline, no pass.** A missing merge base, or a base that fails to
   build or has no app, fails with the instruction to merge main.
5. **The nightly ranks the day's changes** from main's reflog, with their
   reasons, and the morning notice lists the biggest adders.

The 1400 kB budget, the 1260 kB soft line and the 600 kB entry budget are
unchanged.

## Rationale

**A merge-base build cannot race or be gamed.** A committed baseline would be
shared state: parallel branches would each refresh it and conflict, and the
last writer would decide everyone's number. A branch could also refresh it to
hide its own growth. A merge base is fixed by history, so each branch measures
against its own, and one landing never moves another's. The cost is one more
Vite build, about four seconds, cached per base.

**The whole set's delta, not a chunk's.** Moving code between first-paint
chunks still loads it before first paint, so it shows as nothing. Moving it
behind a dynamic import shows as the shrink it is.

**Why 2 kB.** Nine in ten growing changes stay under it and pass without a
word. The 13 over it in five days, under three a day, carried 70 % of all the
growth. A reason from those few reaches most of the growth for little friction.
Raw, as ADR 0436 measures.

**Why a trailer.** It travels with the commit, so a reviewer reads it in
`git log` and the nightly reads it per change. A ledger file would be one more
shared file for parallel branches to conflict on. A trailer on any commit of
the change counts, since the threshold is per change.

**Why one ruler for both trees.** Each side measured by its own copy of the
plugin would compare two rulers whenever a change edits the plugin. It would
also leave every base from before this ADR with nothing to report, this
change's own included. The cached base result is keyed by the ruler too.

**Why fail closed.** A gate that passes when it cannot measure is a gate a
broken setup turns off silently. `/harden` merges main before it measures, so
the base is always recent and buildable in practice.

**Why the reflog for the nightly.** Apply moves main with `update-ref`, so each
reflog entry is exactly one change. Walking commits instead would mix in the
catch-up merges that carry other changes.

## Consequences

- `/harden` runs a `first-paint` suite whenever a first-paint input changes,
  and the clean-build skill runs `check` on a branch and `report` on main.
- A change that adds over 2 kB carries its reason in history, where the
  nightly and any reviewer find it.
- The reflog lives on the machine that runs Apply. A clone without it gets no
  nightly ranking, and the report says so.
- A change that edits `chunkBudget.ts` measures its base with the edited
  ruler. A reviewer reads such a diff as a change to the measurement itself.
- A base whose lockfile matches this checkout's reuses its `node_modules`. A
  base with a different lockfile gets its own `npm ci` in the temp tree, so a
  dependency bump measures. A failed install fails the check.

## Alternatives considered

- **A committed baseline file refreshed on main.** Shared state: parallel
  branches race on it, and a branch can rewrite it.
- **A hand-typed baseline.** Goes stale at once, and is the number this check
  exists to replace.
- **Per-commit thresholds.** Splitting one 20 kB addition into small commits
  would pass. The change is the unit.
- **A per-chunk threshold.** Rewards moving bytes between first-paint chunks.
- **A ledger file of reasons.** Conflicts across parallel branches.
- **Raising the budget.** Out of scope, and the problem was never the number.
