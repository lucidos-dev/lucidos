# 0769: The transcript window opens and expands by a step budget and holds its top as an edge

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0261](0261-a-fill-round-that-draws-nothing-is-not-charged.md), [0152](0152-a-reading-position-names-a-turn.md), [0234](0234-a-position-behind-the-page-opens-at-the-newest-page.md)

## Context

A thread can hold far more rendered steps than one first paint should pay for.
`threadWindow.ts` decides which exchanges render, and how much more arrives on
each scroll up.

One reported coding-agent thread holds five turns costing 1, 65, 684, 425 and 1
steps. The budget there is 160. Every turn is larger than the whole budget. So the rule "draw at least one turn" was the only one that applied, and the window did nothing.

This record restates the design comments of
`crates/lucidos-app/src/components/chat/threadWindow.ts`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- The first paint and each scroll-up expansion are bounded by a budget of
  rendered steps, not by milliseconds.
- The oldest exchange on screen, the floor exchange, draws only its tail. Its
  head arrives through the same expansion that brings older turns.
- The floor budget is counted in rows the reader's view draws
  (`rowsDrawnByClamp`), not in raw events.
- The window's top is an edge in two dimensions, turn and row. It is not a
  count.
- A scroll-up expansion adds one more budget's worth of older exchanges, capped
  at the total.
- Fill expansions that drew something are capped per thread as a backstop. A
  round that drew nothing is not charged (`settleFillLedger`).

## Rationale

Step count is O(1) per exchange through `exchangeRenderCost`. What really
drives cost is the prose in a turn's text events. That varies by more than 10x
for the same step count. Reading it would mean walking every payload on every
fold. Counting steps bounds the worst case, which is what the budget is for.

A row that draws nothing renders `null`. It costs nothing and fills no height.
With steps hidden, a coding-agent turn is mostly such rows. A budget that
counted them could fill with rows the reader cannot see. With every row drawn,
80 is about what 160 raw events cost.

The top is an edge because no turn and no row the reader has been shown may
leave the DOM. A count slides forward as the live turn grows. See
`renderCountFromFloor`.

Expansion is budgeted for the same reason the seed is. A flat step of turns
hands the reader the rest of a step-heavy thread in one scroll. That moves the
block the seed avoided onto the scroll.

## Consequences

- A turn larger than the whole budget still draws, so the transcript is never
  blank. It draws its tail first.
- `rowsAt` is a callback, because only the floor exchange's rows are needed.
  Folding every exchange to find them would cost what the window exists to
  avoid.
- `exchangeRenderCost` stays O(1) and keeps counting raw events. That is a
  different unit from drawn rows and cannot answer the floor question.
- A fill round takes either `ROW_BUDGET` drawn rows of the floor turn, one
  oversized turn, or up to `WINDOW_STEP` ordinary turns. Each fills a pane, so
  real threads take one round, sometimes two.
- A round that drew nothing is nearly free. The fill can cross hundreds of null rows between two lines of prose to reach something visible.

## Alternatives considered

- Budgeting by milliseconds, or by reading the prose in each turn's text
  events. Rejected: it would mean walking every payload on every fold.
- Keeping only the floor of "at least one turn". Rejected: on a coding-agent
  thread it was the only rule that applied, and the window did nothing.
- Counting raw events for the floor exchange. Rejected: rows that draw nothing
  could fill the budget with content the reader cannot see.
- A count for the window's top edge. Rejected: it slides forward as the live
  turn grows, and turns or rows already shown would leave the DOM.
- A flat step of turns per scroll-up expansion. Rejected: it hands the reader
  the rest of a step-heavy thread in one scroll.
