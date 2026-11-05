import { describe, expect, it } from 'vitest';
import type { ServedIntelligenceRung } from '../../api/types';
import {
  draggedRung, KEEP_PRICES_CURRENT_CRON, KEEP_PRICES_CURRENT_SLUG, keepPricesCurrentTrigger, ladderOverride, movedRung,
  percent, rowUnder, toggledRung,
} from './intelligenceLadder';

const rung = (model: string, effort: string, on = true): ServedIntelligenceRung => ({
  model,
  effort,
  label: model,
  on,
  estimated: true,
  intelligence: null,
  cost: null,
  time: null,
  is_default: false,
});

const shipped = [rung('a', 'low'), rung('b', 'low', false), rung('c', 'high')];
const shippedPairs = [
  { model: 'a', effort: 'low' },
  { model: 'c', effort: 'high' },
];

describe('ladderOverride', () => {
  it('stays unset while the rungs that are on match the shipped ladder', () => {
    expect(ladderOverride(shipped, shippedPairs)).toBeNull();
  });

  it('stores the rungs that are on, in order, once they differ', () => {
    expect(ladderOverride(toggledRung(shipped, 1), shippedPairs)).toBe('a=low, b=low, c=high');
    expect(ladderOverride(toggledRung(shipped, 0), shippedPairs)).toBe('c=high');
  });
});

describe('movedRung', () => {
  it('swaps past the off rungs to the next rung that is on', () => {
    const moved = movedRung(shipped, 2, -1);
    expect(moved?.map((r) => r.model)).toEqual(['c', 'b', 'a']);
    expect(ladderOverride(moved ?? [], shippedPairs)).toBe('c=high, a=low');
  });

  it('has nowhere to go at an end, and never moves an off rung', () => {
    expect(movedRung(shipped, 0, -1)).toBeNull();
    expect(movedRung(shipped, 2, 1)).toBeNull();
    expect(movedRung(shipped, 1, 1)).toBeNull();
  });
});

describe('percent', () => {
  it('rounds a share to a whole percentage', () => {
    expect(percent(0.7817)).toBe(78);
    expect(percent(0)).toBe(0);
  });
});

describe('draggedRung', () => {
  const four = [rung('a', 'low'), rung('b', 'low', false), rung('c', 'high'), rung('d', 'high')];
  const order = (rungs: ServedIntelligenceRung[] | null) => rungs?.map((r) => r.model).join('');

  it('carries a rung down to the rung it is dropped on, as the arrows would', () => {
    expect(order(draggedRung(four, 0, 3))).toBe('cbda');
  });

  it('carries a rung up over several rungs in one drop', () => {
    expect(order(draggedRung(four, 3, 0))).toBe('dbac');
  });

  it('moves nothing when dropped where it started, or when it is off', () => {
    expect(draggedRung(four, 2, 2)).toBeNull();
    expect(draggedRung(four, 1, 3)).toBeNull();
  });

  it('stops just past an off rung it is dropped on', () => {
    expect(order(draggedRung(four, 0, 1))).toBe('cbad');
  });
});

describe('rowUnder', () => {
  const rows = [{ index: 0, bottom: 40 }, { index: 2, bottom: 80 }, { index: 3, bottom: 120 }];

  it('names the row whose band holds the pointer, clamped at both ends', () => {
    expect(rowUnder(rows, -10)).toBe(0);
    expect(rowUnder(rows, 39)).toBe(0);
    expect(rowUnder(rows, 40)).toBe(2);
    expect(rowUnder(rows, 119)).toBe(3);
    expect(rowUnder(rows, 500)).toBe(3);
  });
});

describe('Keep prices current', () => {
  it('creates an intent trigger under its own slug, on the picked schedule', () => {
    for (const cadence of ['daily', 'weekly'] as const) {
      const trigger = keepPricesCurrentTrigger(cadence);
      expect(trigger.slug).toBe(KEEP_PRICES_CURRENT_SLUG);
      expect(trigger.run.type).toBe('intent');
      expect(trigger.cron_expressions).toEqual([KEEP_PRICES_CURRENT_CRON[cadence]]);
    }
  });

  /** Lucidos never creates the price trigger by itself (ADR 0525): the
   *  page's button is the one caller of the request that builds it. */
  it('is built only by the Intelligence page', () => {
    const sources = import.meta.glob<string>(
      ['../../**/*.{ts,tsx}', '!../../**/*.test.{ts,tsx}', '!./intelligenceLadder.ts'],
      { query: '?raw', import: 'default', eager: true },
    );
    const callers = Object.entries(sources)
      .filter(([, text]) => text.includes('keepPricesCurrentTrigger('))
      .map(([path]) => path);
    expect(callers).toEqual(['./IntelligencePage.tsx']);
  });
});
