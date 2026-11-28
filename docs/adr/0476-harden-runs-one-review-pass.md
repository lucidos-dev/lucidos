# 0476: harden runs one Claude review pass per iteration, and repeat iterations review only the fix

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amended by**: [0558](0558-harden-other-model-review.md). "Plus Codex"
  becomes "plus the other model": Claude reviews a Codex run.

## Context

Cost became a first-class concern. A study of 1,099 `/harden` runs over the
three weeks to 2026-10-14 found that hardening took 53% of coding-agent spend.
The main agent, carrying about 250k tokens per turn, was 75% of that.

- **Repeat iterations dominate.** Runs with two or more suite iterations took 58% of
  harden spend, and each iteration re-ran the whole review and a new Codex pass.
- **Most reviewers found little.** In a hand-read sample of 22 runs,
  `code-review` found a real bug in 9, the Phase 2 bug agent in 5, and Codex
  in 3. The compliance agent found none, and the regression agent at most one.
- **Validation ran twice.** The skill's verifier checked each finding, then
  the orchestrator checked it again. Phase 3's validator subagents ran 26
  times in 30 days.
- **Review fixes cost suite runs.** A compliance fix to a code comment voids
  the early suite run (ADR 0292).

The plan is `docs/plans/2026-10-14-harden-cheaper-rounds-and-one-review-pass.md`.

## Decision

Each iteration runs one Claude review, `code-review` at high effort, in a
fresh-context subagent, plus Codex on the first iteration. An iteration after a suite
failure reviews only the commits since the previous review. Hardening fixes
bugs and required rule violations, and reports nits and cleanup without fixing
them.

## Rationale

- **Another copy of the same model adds little.** Research on multi-agent
  review shows that same-model reviewers saturate quickly, while a different
  model family keeps adding catches. One strong Claude pass plus Codex keeps
  the decorrelated reviewer and drops the redundant ones.
- **The surviving duties are cheap as angles.** Glossary drift, temporary
  measures, private data and recent-history regressions need no agent of
  their own. The prose limits are already a script.
- **A fresh-context subagent keeps review reads out of the main context.**
  Each file the main agent reads is re-read on every later turn.
- **A fix is small.** Re-reviewing a whole branch to certify a ten-line test
  fix re-pays the full review for nothing new.
- **Nits do not justify an iteration.** Fixing one can void the early suite run
  and start another iteration.

## Consequences

- Phase 2's three agents, Phase 3's validator subagents and Phase 0.75 are
  gone. The plan marker still gates the first edit and Apply.
- Some bugs the separate bug agent caught may now slip through. Each
  iteration's `HardenRunCompleted` records findings by source, so a re-measure
  can show the cost.
- Phase 4.5, its gates and its suites are unchanged.

## Alternatives considered

- **Sonnet 5.5 for the review.** It costs half as much, but no review
  benchmark covers it, and we had no runs of our own. The structural cuts
  give most of the saving anyway. Revisit after a replay of the 22 sampled
  diffs on both models.
- **Codex as the only reviewer.** In the sample it caught 3 real bugs, against
  14 for the two Claude bug passes. It suits low-risk diffs only.
- **No Codex.** It is the only reviewer from another model family, so
  dropping it loses catches the Claude passes miss.
- **Keep the bug agent as a second pass.** Safer on catches, at about $1 more
  per iteration. The user chose the single pass.
- **A fresh hardening session first.** Its estimated saving is 5 to 11% of
  spend, and most of it overlaps with auto-compact.
