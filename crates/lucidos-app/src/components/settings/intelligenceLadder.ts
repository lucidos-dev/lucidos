import type { LadderSurface, ServedIntelligenceRung, IntelligenceRung } from '../../api/types';

/** The ladders Settings → Intelligence draws, in order. The chat surface is
 *  the Lucidos Agent's. */
export const LADDER_SURFACES: readonly { surface: LadderSurface; name: string }[] = [
  { surface: 'chat', name: 'Lucidos Agent' },
  { surface: 'claude-code', name: 'Claude Code' },
  { surface: 'codex', name: 'Codex' },
];

/** Where Search everywhere lands for `surface`'s ladder. */
export function ladderAnchor(surface: LadderSurface): string {
  return `intelligence:ladder-${surface}`;
}

/** The slug of the trigger Keep prices current creates. The page finds it by
 *  this, so a renamed trigger still counts. */
export const KEEP_PRICES_CURRENT_SLUG = 'keep-prices-current';

/** The schedules Keep prices current offers: 09:00 daily, or Mondays. */
export const KEEP_PRICES_CURRENT_CRON = {
  daily: '0 0 9 * * *',
  weekly: '0 0 9 * * 1',
} as const;

export type PriceCheckCadence = keyof typeof KEEP_PRICES_CURRENT_CRON;

/** The trigger Keep prices current creates for `cadence`. Its intent says
 *  what the user wants; the price-check knowhow says how. */
export function keepPricesCurrentTrigger(cadence: PriceCheckCadence) {
  return {
    name: 'Keep prices current',
    slug: KEEP_PRICES_CURRENT_SLUG,
    run: {
      type: 'intent' as const,
      intent: 'Keep the model prices in the price table current, and tell me when a price changes.',
    },
    cron_expressions: [KEEP_PRICES_CURRENT_CRON[cadence]],
  };
}

function pairValue(pairs: readonly IntelligenceRung[]): string {
  return pairs.map((p) => `${p.model}=${p.effort}`).join(', ');
}

/** What to store for `rungs`: the rungs that are on, in order. `null` when
 *  that restates the shipped ladder, so the preference stays unset and a
 *  later refresh still reaches this workspace. */
export function ladderOverride(rungs: readonly ServedIntelligenceRung[], shipped: readonly IntelligenceRung[]): string | null {
  const value = pairValue(rungs.filter((r) => r.on));
  return value === pairValue(shipped) ? null : value;
}

/** `rungs` with the rung at `index` swapped past the next rung that is on in
 *  `step`'s direction. Off rungs keep their shipped place, so moving one past
 *  them only would save nothing. `null` when there is nowhere to go. */
export function movedRung(rungs: readonly ServedIntelligenceRung[], index: number, step: -1 | 1): ServedIntelligenceRung[] | null {
  let target = index + step;
  while (target >= 0 && target < rungs.length && !rungs[target].on) target += step;
  if (!rungs[index].on || target < 0 || target >= rungs.length) return null;
  const next = [...rungs];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

/** `rungs` with the rung at `from` carried to the rung at `to`, one `movedRung`
 *  step at a time. A drag so ends where the arrows would have taken it. A `to`
 *  that is off leaves the rung just past it. `null` when nothing moves. */
export function draggedRung(rungs: readonly ServedIntelligenceRung[], from: number, to: number): ServedIntelligenceRung[] | null {
  const step = to > from ? 1 : -1;
  let current: ServedIntelligenceRung[] | null = null;
  let at = from;
  while (step > 0 ? at < to : at > to) {
    const next = movedRung(current ?? rungs, at, step);
    if (!next) break;
    current = next;
    at = next.findIndex((r) => r.model === rungs[from].model && r.effort === rungs[from].effort);
  }
  return current;
}

/** The row a drag at `y` is over: the first whose bottom edge is below `y`,
 *  else the last. `rows` run top to bottom. */
export function rowUnder(rows: readonly { index: number; bottom: number }[], y: number): number {
  return (rows.find((row) => y < row.bottom) ?? rows[rows.length - 1]).index;
}

/** `rungs` with the rung at `index` switched on or off. */
export function toggledRung(rungs: readonly ServedIntelligenceRung[], index: number): ServedIntelligenceRung[] {
  return rungs.map((r, i) => (i === index ? { ...r, on: !r.on } : r));
}

/** A share from 0 to 1 as a whole percentage. */
export function percent(share: number): number {
  return Math.round(share * 100);
}
