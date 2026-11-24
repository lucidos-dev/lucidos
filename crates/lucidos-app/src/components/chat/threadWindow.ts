
import { transcriptScrolls } from './transcriptScrolls';

export const INITIAL_WINDOW = 20;

export const WINDOW_STEP = 20;

export const STEP_BUDGET = 160;

export const WINDOW_EXPAND_MARGIN_PX = 1600;

export const MAX_FILL_EXPANSIONS = 4;

export const MAX_FILL_BACKFILLS = 4;

export interface FillReading {
  drawn: number;
  floor: number;
}

export interface FillLedger {
  grow: number;
  page: number;
  last: ({ kind: 'grow' | 'page' } & FillReading) | null;
}

export const EMPTY_FILL_LEDGER: FillLedger = { grow: 0, page: 0, last: null };

export function settleFillLedger(ledger: FillLedger, now: FillReading): FillLedger {
  const { last } = ledger;
  if (!last) return ledger;
  const progressed = last.kind === 'grow' || now.floor < last.floor;
  if (!progressed || now.drawn > last.drawn) return { ...ledger, last: null };
  return { ...ledger, [last.kind]: ledger[last.kind] - 1, last: null };
}

export function fillRoundAllowed(ledger: FillLedger, kind: 'grow' | 'page'): boolean {
  return ledger[kind] < (kind === 'grow' ? MAX_FILL_EXPANSIONS : MAX_FILL_BACKFILLS);
}

export function chargeFillRound(ledger: FillLedger, kind: 'grow' | 'page', now: FillReading): FillLedger {
  return { ...ledger, [kind]: ledger[kind] + 1, last: { kind, ...now } };
}

export function exchangeRenderCost(exchange: { steps: readonly unknown[] }): number {
  return exchange.steps.length + 1;
}

export const ROW_BUDGET = 80;

export const ROW_CEILING = ROW_BUDGET * 4;

export type RowsAt = (index: number) => readonly boolean[];

export interface WindowEdge {
  exchange: number;
  rowsHidden: number;
}

export const WHOLE_THREAD: WindowEdge = { exchange: 0, rowsHidden: 0 };

export type StoredWindowKind = 'claimed' | 'grown';

export function deepLinkEdge(current: WindowEdge, targetIndex: number): WindowEdge {
  if (targetIndex < 0 || edgeReachesRow(current, targetIndex, 0)) return current;
  return { exchange: targetIndex, rowsHidden: 0 };
}

export function seedRowsHidden(
  rows: readonly boolean[],
  budget = ROW_BUDGET,
  ceiling = ROW_CEILING,
): number {
  const wanted = Math.max(1, budget);
  const stop = Math.max(0, rows.length - Math.max(1, ceiling));
  let drawn = 0;
  for (let i = rows.length - 1; i > stop; i--) {
    if (rows[i] && ++drawn === wanted) return i;
  }
  return stop;
}

export function drawnRowsInWindow(edge: WindowEdge, total: number, rowsAt: RowsAt): number {
  let drawn = 0;
  for (let i = Math.max(0, edge.exchange); i < total; i++) {
    const rows = rowsAt(i);
    for (let r = i === edge.exchange ? edge.rowsHidden : 0; r < rows.length; r++) {
      if (rows[r]) drawn++;
    }
  }
  return drawn;
}

export function edgeHasMoreAbove(edge: WindowEdge): boolean {
  return edge.exchange > 0 || edge.rowsHidden > 0;
}

export function seedWindowEdge(costs: readonly number[], rowsAt: RowsAt): WindowEdge {
  const exchange = computeRenderFromIndex(costs.length, seedRenderCount(costs));
  return { exchange, rowsHidden: seedRowsHidden(rowsAt(exchange)) };
}

export function expandWindowEdge(
  edge: WindowEdge,
  costs: readonly number[],
  rowsAt: RowsAt,
): WindowEdge {
  if (edge.rowsHidden > 0) {
    const hidden = rowsAt(edge.exchange).slice(0, edge.rowsHidden);
    return { exchange: edge.exchange, rowsHidden: seedRowsHidden(hidden) };
  }
  const current = renderCountFromFloor(costs.length, edge.exchange);
  const next = expandRenderCount(costs, current);
  if (next === current) return edge;
  const exchange = computeRenderFromIndex(costs.length, next);
  return { exchange, rowsHidden: seedRowsHidden(rowsAt(exchange)) };
}

export function edgeReachesRow(edge: WindowEdge, index: number, row: number): boolean {
  if (index > edge.exchange) return true;
  return index === edge.exchange && edge.rowsHidden <= row;
}

export function edgeMustReachRow(edge: WindowEdge, index: number, row: number): boolean {
  if (index < 0) return false;
  if (edgeReachesRow(edge, index, row)) return false;
  return edgeHasMoreAbove(edge);
}

export const MAX_READING_CHASES = 2;

export const READING_CHASE_PAGE_SIZE = 2000;

export function readingChaseAction(state: {
  readInFlight: boolean;
  paneMeasurable: boolean;
  hasOlderEvents: boolean;
  chasesSpent: number;
}): 'wait' | 'chase' | 'give-up' {
  if (state.readInFlight || !state.paneMeasurable) return 'wait';
  if (state.hasOlderEvents && state.chasesSpent < MAX_READING_CHASES) return 'chase';
  return 'give-up';
}

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

export function seedRenderCount(
  costs: readonly number[],
  budget = STEP_BUDGET,
  max = INITIAL_WINDOW,
): number {
  return countWithinBudget(costs, costs.length, budget, max);
}

export function renderCountFromFloor(total: number, floor: number): number {
  const rows = Math.max(0, total);
  if (rows === 0) return 0;
  return rows - Math.min(Math.max(0, floor), rows - 1);
}

export interface RenderWindowSeedState {
  hasExchanges: boolean;
  eventsLoaded: boolean;
  eventsLoadFailed: boolean;
}

export function canSeedRenderWindow(state: RenderWindowSeedState): boolean {
  return state.hasExchanges && (state.eventsLoaded || state.eventsLoadFailed);
}

export function computeRenderFromIndex(total: number, renderCount: number): number {
  if (!Number.isFinite(renderCount)) return 0;
  return Math.max(0, total - Math.max(0, Math.floor(renderCount)));
}

export function expandRenderCount(costs: readonly number[], renderCount: number): number {
  const total = costs.length;
  const shown = Math.min(Math.max(0, renderCount), total);
  return Math.min(total, shown + countWithinBudget(costs, total - shown, STEP_BUDGET, WINDOW_STEP));
}

const AT_TOP_SLACK_PX = 1;

export const UPWARD_SCROLL_KEYS: ReadonlySet<string> = new Set([
  'ArrowUp', 'PageUp', 'Home',
]);

export function atScrollTop(view: { scrollTop: number }): boolean {
  return view.scrollTop <= AT_TOP_SLACK_PX;
}

export function clampedScrollTop(view: { scrollTop: number; scrollHeight: number; clientHeight: number }): number {
  return Math.max(0, Math.min(view.scrollTop, view.scrollHeight - view.clientHeight));
}

export function scrollAsksForOlder(previousTop: number, top: number): boolean {
  return top < previousTop && top <= WINDOW_EXPAND_MARGIN_PX;
}

export type FillAction = 'none' | 'grow' | 'page';

export function fillAction(
  view: { scrollHeight: number; clientHeight: number },
  edge: WindowEdge,
  hasOlderEvents: boolean,
): FillAction {
  if (transcriptScrolls(view)) return 'none';
  if (edgeHasMoreAbove(edge)) return 'grow';
  return hasOlderEvents ? 'page' : 'none';
}

export function anythingAbove(
  scrolledDown: boolean,
  edge: WindowEdge,
  hasOlderEvents: boolean,
): boolean {
  return scrolledDown || edgeHasMoreAbove(edge) || hasOlderEvents;
}

export function reseedOnReopen(stored: StoredWindowKind | undefined): boolean {
  return stored === 'claimed';
}

export function scrollToTopNeedsRenderAll(edge: WindowEdge, hasOlderEvents = false): boolean {
  return edgeHasMoreAbove(edge) || hasOlderEvents;
}
