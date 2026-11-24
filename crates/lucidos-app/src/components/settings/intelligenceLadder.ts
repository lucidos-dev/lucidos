import type { IntelligenceLadder, LadderSurface, ServedIntelligenceRung, IntelligenceRung } from '../../api/types';

export const LADDER_SURFACES: readonly { surface: LadderSurface; name: string }[] = [
  { surface: 'chat', name: 'Lucidos Agent' },
  { surface: 'claude-code', name: 'Claude Code' },
  { surface: 'codex', name: 'Codex' },
];

export function ladderAnchor(surface: LadderSurface): string {
  return `intelligence:ladder-${surface}`;
}

export const KEEP_PRICES_CURRENT_SLUG = 'keep-prices-current';

export const KEEP_PRICES_CURRENT_CRON = {
  daily: '0 0 9 * * *',
  weekly: '0 0 9 * * 1',
} as const;

export type PriceCheckCadence = keyof typeof KEEP_PRICES_CURRENT_CRON;

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

export function ladderOverride(rungs: readonly ServedIntelligenceRung[], shipped: readonly IntelligenceRung[]): string | null {
  const value = pairValue(rungs.filter((r) => r.on));
  return value === pairValue(shipped) ? null : value;
}

export function movedRung(rungs: readonly ServedIntelligenceRung[], index: number, step: -1 | 1): ServedIntelligenceRung[] | null {
  let target = index + step;
  while (target >= 0 && target < rungs.length && !rungs[target].on) target += step;
  if (!rungs[index].on || target < 0 || target >= rungs.length) return null;
  const next = [...rungs];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

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

export function toggledRung(rungs: readonly ServedIntelligenceRung[], index: number): ServedIntelligenceRung[] {
  return rungs.map((r, i) => (i === index && !r.is_default ? { ...r, on: !r.on } : r));
}

export function percent(share: number): number {
  return Math.round(share * 100);
}

export function starredLadder(ladder: IntelligenceLadder, pick: ServedIntelligenceRung): IntelligenceLadder {
  const { model, effort } = pick;
  return {
    ...ladder,
    default_rung: { model, effort },
    rungs: ladder.rungs.map((rung) => {
      const is_default = rung.model === model && rung.effort === effort;
      return { ...rung, is_default, on: rung.on || is_default };
    }),
  };
}
