# 0798: The eval analyser compares arms in differences and voids pairs where arms disagree on retrieval

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0110](0110-context-handling-benchmark.md)

## Context

`analyse.rs` reads eval runs and reports on them. A run can carry one or two
arms, and a sweep pools several runs. The comments record the rules that keep
those comparisons honest.

This record restates the design comments of
`crates/lucidos-engine-bin/src/bin/lucidos-eval/analyse.rs`, written before the comment strip removes
them. Nobody checked it line by line against the code.

## Decision

- Axis 6 reports how the model wrote its working understanding. It is
  aggregated across threads. `beside_a_call` states the claim that the document
  rides along with a tool call as one rate.
- Totals carry a per-task mean beside them.
- Refusals are counted per arm, in total and per task.
- The two-arm block reports differences, not ratios. It prints only when a run
  carried two arms.
- The analyser reports whether the arms ran inside one invocation, task by
  task. It reports a mismatch and does not refuse it.
- I6 checks that every run row agrees on six defining things. `engine_commit`
  is not one of them.
- A sweep exists only for an arm that ran at more than one window. Its answer
  is the smallest window that held.
- A pair is voided when the arms disagree about retrieval on a task. Both
  skipping counts as agreement and is kept. A missing arm is not a
  disagreement. Only arms on one memory module are compared.
- The number of arms a pair needs to count as measured is set per run.

## Rationale

- The three working-understanding figures were collected per thread but never
  aggregated. The design question they were added to answer went unanswered.
- Two arms rarely run the same number of threads once a pair is voided, so a
  per-task mean sits beside the totals.
- A refusal voids nothing on the other arm, so the pair census cannot show it.
  A recovered refusal leaves no other trace.
- A difference leaves both absolute figures readable. A ratio replaces them
  with one number whose denominator must then be found.
- Separate runs, which the baseline workflow produces, let provider drift land
  on one side. I7's interleave check cannot see it, because each run has one
  arm per task. The two absolute blocks are still good, so the case is
  reported.
- A sweep pools several runs, so all six I6 properties are checked, not the
  fixture alone. A sweep whose model changed halfway is a model comparison
  wearing a budget curve's clothes.
- Rebuilding between runs is normal and mostly harmless. `guidance_hash`
  already catches the one engine change that alters what the model is told.
- A set of size one would print a smallest budget that held, and that number
  would be only the one window there was.
- A failed row does not disqualify a smaller row that passed. The table shows
  both, and a non-monotone sweep is a result worth seeing.
- The engine's query classifier decides whether memory is retrieved, and it is
  an LLM call. A task one arm retrieved for and the other did not measures that
  call, not the configuration.
- A Tree turn never runs memory recall. Tree and Classic differing on it is the
  module, not the classifier.
- A run of one configuration wants one arm. Asking for two would report every
  pair as an upstream failure.

## Consequences

- A reader sees absolute figures beside every difference.
- A mixed-source comparison still prints, and it is flagged instead of refused.
- A sweep with a failing row between passing rows is shown as it is.
- Pairs where retrieval differed between arms are dropped from the comparison.
- A sweep may pool single-arm runs with a baseline run without the baseline
  raising the bar for the others.
- Two runs that differ in `engine_commit` alone still pool.

## Alternatives considered

- Reporting ratios between arms. Rejected because the denominator then has to
  be found.
- Refusing a comparison whose arms came from separate runs. Rejected because
  the absolute blocks are still good.
- Reporting a one-window set as a sweep. Rejected because its smallest budget
  would be meaningless.
- Smoothing a non-monotone sweep. Rejected because the result is worth seeing.
- Checking only the fixture, or adding `engine_commit` to I6. Rejected because
  a sweep pools several runs and rebuilds are mostly harmless.
- Pooling the measured-arm threshold across runs. Rejected because one two-arm
  run would write off every single-arm run.
