/** Thread-render windowing (perf): a large focused thread used to render — and
 *  markdown-parse — every exchange synchronously on open and on each re-render,
 *  blocking the main thread for hundreds of ms (measured: ~270–500ms of pure JS,
 *  style/layout 0ms). ThreadView now renders only a contiguous TAIL of the
 *  exchange list and grows it as the user scrolls toward the top. These pure
 *  helpers own the window arithmetic so it's unit-tested independently of the
 *  component.
 *
 *  **The window is measured in STEPS, not turns.** Counting turns alone left the
 *  mechanism doing nothing for the shape that needs it most: a chat thread with
 *  few user messages and hundreds of tool calls between them. One reported
 *  thread held 653 events in 19 exchanges, so it sat under the 20-exchange cap
 *  and rendered whole. It blocked long enough that the skeleton's delay gate
 *  never got a frame (ADR 0081), which is the blank pane the reader saw.
 *  Measured costs and the budget they picked:
 *  docs/plans/2026-08-26-the-transcript-window-counts-steps-not-turns.md. */

/** Most trailing exchanges to render on first open, whatever the step budget
 *  allows. On a step-heavy thread the budget is the binding limit. This cap is
 *  what holds an ordinary chat to the window it has always had.
 *
 *  A thread with no saved position opens at the TOP of this slice (see
 *  ThreadView's `resetOnEmpty`), so the window is what the reader can reach by
 *  scrolling DOWN without waiting on anything; older exchanges materialize as
 *  they scroll up past `WINDOW_EXPAND_MARGIN_PX`. */
export const INITIAL_WINDOW = 20;

/** Most exchanges one scroll-up expansion may reveal, whatever the step budget
 *  allows. `INITIAL_WINDOW`'s counterpart: both caps hold an ordinary chat to
 *  the chunk sizes it has always had, and the budget takes over on a
 *  step-heavy thread. */
export const WINDOW_STEP = 20;

/** How many rendered steps the first paint may cost, and how much each
 *  scroll-up expansion adds.
 *
 *  A bound on node count rather than on milliseconds. What really drives the
 *  cost is how much prose a turn's text events carry. That varies by more than
 *  10x for the same step count, and reading it would mean walking every payload
 *  on every fold. Counting steps is O(1) per exchange and bounds the worst
 *  case, which is what the budget is for. */
export const STEP_BUDGET = 160;

/** Distance (px) from the top of the scroll container at which to grow the
 *  window — a buffer so older exchanges are ready before the user reaches the
 *  very top. */
export const WINDOW_EXPAND_MARGIN_PX = 600;

/** How much overflow (px) a container must have before anybody can scroll it. A
 *  hair from a border or a rounded line height does not count.
 *
 *  ONE definition, read by `windowNeedsFill` below and by
 *  `scrollState.isScrollable`, which asks the same question for the up chevron.
 *  The fill grows the window until the transcript scrolls, so the two must mean
 *  the same thing by the number. A matching literal in each is not that. It
 *  lives here because this module imports nothing, so `scrollState` can read it
 *  without a cycle. */
export const SCROLLABLE_SLACK_PX = 10;

/** Most fill expansions one thread may take. A backstop, not a budget.
 *
 *  A round takes either ONE oversized turn or up to `WINDOW_STEP` ordinary
 *  ones. An oversized turn blew the budget alone, so it fills a pane by itself,
 *  and `WINDOW_STEP` ordinary ones fill it between them. Real threads therefore
 *  take one round, sometimes two.
 *
 *  The cap covers the degenerate case where neither holds: a reader who has
 *  FOLDED oversized turns leaves rows that parse their markdown and draw no
 *  height. Four bounds that at a few times the seed's own cost. Reaching it
 *  leaves the up chevron, which renders the whole thread in one press. */
export const MAX_FILL_EXPANSIONS = 4;

/** One exchange's share of the budget: its steps plus its own user bubble. */
export function exchangeRenderCost(exchange: { steps: readonly unknown[] }): number {
  return exchange.steps.length + 1;
}

/** How many of the FLOOR exchange's rendered rows the window may draw.
 *
 *  The budget above picks WHICH turns render, and it has one hard floor: a turn
 *  larger than the whole budget must still draw, or the transcript is blank.
 *  On a coding-agent thread every turn is larger than the whole budget. The
 *  reported one holds five turns costing 1, 65, 684, 425 and 1 steps, against a
 *  budget of 160. So "at least one" was the only rule that ever applied, and
 *  the window did nothing.
 *
 *  This is that floor made finer. The oldest turn on screen draws its TAIL. Its
 *  head arrives as the reader scrolls toward it, through the same expansion
 *  older turns arrive through.
 *
 *  Counted in RENDERED rows, not raw events, unlike `STEP_BUDGET`. In a
 *  tool-heavy turn about half the raw events draw no row of their own, a result
 *  settling the row its call opened. So 80 rows is about what 160 raw events
 *  cost. */
export const ROW_BUDGET = 80;

/** The window's top edge. Two dimensions, because one turn can outweigh a whole
 *  transcript.
 *
 *  An EDGE rather than a count, in both dimensions, and for one reason: no turn
 *  and no row the reader has already been shown may leave the DOM. A count
 *  slides forward as the live turn grows. See `renderCountFromFloor`. */
export interface WindowEdge {
  /** Index of the oldest exchange rendered. */
  exchange: number;
  /** How many of THAT exchange's leading rendered rows are left out. Always 0
   *  for every other exchange in the window: they were admitted whole. */
  rowsHidden: number;
}

/** The whole window, rendered. What a deep link and the scroll-to-top chevron
 *  set, and the only edge that is safe to compare against by value. */
export const WHOLE_THREAD: WindowEdge = { exchange: 0, rowsHidden: 0 };

/** Must a deep link's render-all be WRITTEN to this thread's stored edge?
 *
 *  `undefined` is the case worth naming, and it answers YES. A cold push tap
 *  finds `threadMap` empty, so the claim lands before the seed does. Read as
 *  `WHOLE_THREAD` instead, an absent edge would skip the write. The seed would
 *  then store the tail, and the thread would snap back to it the moment the
 *  claim cleared.
 *
 *  A function rather than an inline test because that is the arm the inline
 *  test got wrong, and it is invisible on a warm thread. */
export function deepLinkMustPersist(stored: WindowEdge | undefined): boolean {
  return !stored || edgeHasMoreAbove(stored);
}

/** How many leading rows to hide in an exchange drawing `rowCount` of them.
 *
 *  Never all of them: a turn admitted to the window draws something, the same
 *  floor `countWithinBudget` keeps one level up. */
export function seedRowsHidden(rowCount: number, budget = ROW_BUDGET): number {
  return Math.max(0, rowCount - Math.max(1, budget));
}

/** Is anything left above this edge, whether a whole turn or the head of one? */
export function edgeHasMoreAbove(edge: WindowEdge): boolean {
  return edge.exchange > 0 || edge.rowsHidden > 0;
}

/** The edge a thread opens at.
 *
 *  `rowCountAt` folds one exchange and reports how many rows it draws. A
 *  callback rather than a number, because only the FLOOR exchange's row count
 *  is ever needed. Folding every exchange to find out would be the cost the
 *  window exists to avoid. `exchangeRenderCost` stays O(1) for that reason and
 *  counts raw events, which is a different unit and cannot answer this. */
export function seedWindowEdge(
  costs: readonly number[],
  rowCountAt: (index: number) => number,
): WindowEdge {
  const exchange = computeRenderFromIndex(costs.length, seedRenderCount(costs));
  return { exchange, rowsHidden: seedRowsHidden(rowCountAt(exchange)) };
}

/** The edge after one scroll-up round.
 *
 *  Rows first, then turns. Scrolling up into a turn is a request for its head,
 *  and uncovering that is the cheaper of the two moves. Only once the turn is
 *  whole does the window reach past it, and the turn it reaches is clamped in
 *  its own turn.
 *
 *  Returns the same edge when nothing is left, which is how every caller's
 *  re-entrancy guard tells a real grow from a no-op. */
export function expandWindowEdge(
  edge: WindowEdge,
  costs: readonly number[],
  rowCountAt: (index: number) => number,
): WindowEdge {
  if (edge.rowsHidden > 0) {
    return { exchange: edge.exchange, rowsHidden: Math.max(0, edge.rowsHidden - ROW_BUDGET) };
  }
  const current = renderCountFromFloor(costs.length, edge.exchange);
  const next = expandRenderCount(costs, current);
  if (next === current) return edge;
  const exchange = computeRenderFromIndex(costs.length, next);
  return { exchange, rowsHidden: seedRowsHidden(rowCountAt(exchange)) };
}

/** Is the exchange at `index` rendered WHOLE?
 *
 *  Whole, not merely present, and that is what the *reading position* needs.
 *  ADR 0152 restores a turn by its own `relTop`, measured from its top edge. A
 *  turn whose head is still clamped off cannot place the reader. */
export function edgeReachesIndex(edge: WindowEdge, index: number): boolean {
  if (index > edge.exchange) return true;
  return index === edge.exchange && edge.rowsHidden === 0;
}

/** Does the window owe another round to reach `index` whole?
 *
 *  ThreadView's `reachAnchor` decision in one place, so the walk's termination
 *  is answerable without a component. Three ways it is already done, and each
 *  ends the walk for good. A negative index is a saved position naming a turn
 *  this thread has not got. The turn is rendered whole. Nothing is left above.
 *
 *  It shrinks monotonically under `expandWindowEdge`, which always takes either
 *  a budget of rows or at least one exchange while any remain. So a walk driven
 *  off this terminates. */
export function edgeMustReachIndex(edge: WindowEdge, index: number): boolean {
  if (index < 0) return false;
  if (edgeReachesIndex(edge, index)) return false;
  return edgeHasMoreAbove(edge);
}

/** How many contiguous exchanges ending just before `endExclusive` fit in
 *  `budget`, walking backwards from the newest.
 *
 *  Always takes at least one when there is anything to take. A turn bigger than
 *  the whole budget must still render, rather than leaving the transcript
 *  blank. `max` caps the count independently of cost. */
export function countWithinBudget(
  costs: readonly number[],
  endExclusive: number,
  budget: number,
  max: number,
): number {
  let taken = 0;
  let spent = 0;
  for (let i = Math.min(endExclusive, costs.length) - 1; i >= 0 && taken < max; i--) {
    const cost = costs[i] ?? 0;
    if (taken > 0 && spent + cost > budget) break;
    spent += cost;
    taken++;
  }
  return taken;
}

/** The window a thread opens at: the newest exchanges that fit in `STEP_BUDGET`,
 *  never more than `INITIAL_WINDOW` of them.
 *
 *  ThreadView SEEDS this once per thread and stores it, rather than deriving it
 *  per render. A live turn grows its step count, so a derived window would push
 *  older turns off the top while the reader watches. */
export function seedRenderCount(
  costs: readonly number[],
  budget = STEP_BUDGET,
  max = INITIAL_WINDOW,
): number {
  return countWithinBudget(costs, costs.length, budget, max);
}

/** How many trailing exchanges a window whose TOP EDGE sits at `floor` holds.
 *  The inverse of `computeRenderFromIndex`, and the reason ThreadView stores the
 *  edge rather than the count.
 *
 *  A count is a SIZE and the window's promise is about POSITION: no turn the
 *  reader has already been shown may leave the DOM. Appending a turn grows
 *  `total`, so a held count slides the window forward and evicts the oldest one
 *  it was rendering. The fill below cannot grow the window back once the
 *  transcript scrolls, which a narrow pane reaches in two turns. So the
 *  transcript pins itself to the newest few, however many the reader adds. A
 *  held EDGE keeps them all.
 *
 *  An edge PAST the end still renders one exchange, which is the floor
 *  `countWithinBudget` keeps on the other side. A fold can return fewer
 *  exchanges than the one that seeded the edge: an optimistic pending message
 *  becomes part of the turn it opened. Clamping to `rows` alone would answer
 *  zero there, and a transcript with content would draw nothing. */
export function renderCountFromFloor(total: number, floor: number): number {
  const rows = Math.max(0, total);
  if (rows === 0) return 0;
  return rows - Math.min(Math.max(0, floor), rows - 1);
}

/** The terms `canSeedRenderWindow` reads, named so the rule below can be read
 *  without ThreadView. */
export interface RenderWindowSeedState {
  hasExchanges: boolean;
  eventsLoaded: boolean;
  eventsLoadFailed: boolean;
}

/** May ThreadView fix this thread's window now?
 *
 *  Only once the event load has SETTLED, either way. A thread shows partial
 *  content before its snapshot lands: a pending message the send inserted
 *  optimistically, or an SSE event that beat the fetch. Seeding off that
 *  fragment would pin the window at one exchange. The write happens once, so
 *  the full history would then land into a window sized for the fragment.
 *
 *  A FAILED load settles it too. Nothing further is coming, so the SSE-only
 *  content on screen is all there is. It needs a fixed window as much as a
 *  loaded thread does. */
export function canSeedRenderWindow(state: RenderWindowSeedState): boolean {
  return state.hasExchanges && (state.eventsLoaded || state.eventsLoadFailed);
}

/** First index of the full exchanges array to RENDER. `renderCount` is how many
 *  trailing exchanges to show; `Infinity` (deep-link "render all") → 0. Always a
 *  contiguous tail, so the streaming/active last exchange is always included. */
export function computeRenderFromIndex(total: number, renderCount: number): number {
  if (!Number.isFinite(renderCount)) return 0;
  return Math.max(0, total - Math.max(0, Math.floor(renderCount)));
}

/** Next `renderCount` after a scroll-up expansion: one more budget's worth of
 *  older exchanges, capped so it never exceeds the total (keeps the value stable
 *  once everything is shown).
 *
 *  Budgeted rather than a flat count of turns, for the same reason the seed is.
 *  A flat step hands the reader the rest of a step-heavy thread in one scroll.
 *  That is the block the seed just avoided, moved onto the scroll. */
export function expandRenderCount(costs: readonly number[], renderCount: number): number {
  const total = costs.length;
  const shown = Math.min(Math.max(0, renderCount), total);
  return Math.min(total, shown + countWithinBudget(costs, total - shown, STEP_BUDGET, WINDOW_STEP));
}

/** Must the window grow because the reader cannot REACH what it left out?
 *
 *  The scroll-up expansion is the only way older exchanges enter the window,
 *  and only a scroll event fires it. A slice shorter than the pane produces no
 *  scroll event. So the transcript freezes on whatever the seed took, and the
 *  rest of the thread is unreachable.
 *
 *  The seed cannot prevent that, because it budgets STEPS and steps are a poor
 *  proxy for height. A coding-agent thread ends on a small `ChangeApplied`
 *  boundary sitting right behind a huge working turn. The budget takes the
 *  boundary, the turn behind it blows the budget, and one card draws. Both
 *  reported threads are exactly that shape (see `threadWindow.test.ts`).
 *
 *  So measure. ThreadView grows the window until this answers false, which is
 *  either a transcript that scrolls or a thread rendered whole. */
export function windowNeedsFill(
  view: { scrollHeight: number; clientHeight: number },
  edge: WindowEdge,
): boolean {
  if (!edgeHasMoreAbove(edge)) return false;
  return view.scrollHeight <= view.clientHeight + SCROLLABLE_SLACK_PX;
}

/** Whether a "scroll to top" must render the FULL thread before scrolling.
 *
 *  The transcript is windowed — only a trailing slice is in the DOM — so a plain
 *  smooth `scrollTo(top:0)` only reaches the top of that slice. Worse, as the
 *  smooth scroll nears the top it trips the scroll-up window-expand (see
 *  ThreadView's onScroll), which prepends a chunk and re-anchors the viewport,
 *  stalling the scroll partway. The user then had to click the chevron once per
 *  chunk to crawl to the genuine top (the "needed N clicks" bug). When older
 *  exchanges remain above the window, the chevron renders everything first (set
 *  the edge to `WHOLE_THREAD`) and only then scrolls, landing at the true top
 *  in one action. Equivalent to `edgeHasMoreAbove`, named for the
 *  scroll-to-top contract that depends on it.
 *
 *  A clamped floor turn counts as "more above" too. One smooth scroll can no
 *  more reach its head than it can reach an older turn, and the true top is
 *  that turn's first row. */
export function scrollToTopNeedsRenderAll(edge: WindowEdge): boolean {
  return edgeHasMoreAbove(edge);
}
