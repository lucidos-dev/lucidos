import { describe, it, expect } from 'vitest';
import { formatCount, formatDuration, formatUsd, formatUsdCentral } from './treeBackfillFormat';

describe('the Tree estimate figures', () => {
  it('reads counts compactly, and one figure when both bounds match', () => {
    expect(formatCount({ low: 210_000, high: 260_000 })).toBe('210K–260K');
    expect(formatCount({ low: 42, high: 42 })).toBe('42');
  });

  it('reads cents under a dollar and whole dollars above', () => {
    expect(formatUsd({ low: 0.04, high: 0.4 })).toBe('$0.04–$0.40');
    expect(formatUsd({ low: 1100.4, high: 2300 })).toBe('$1,100–$2,300');
    expect(formatUsd({ low: 0, high: 0 })).toBe('$0');
  });

  it('leads with the central figure, the range beside it', () => {
    expect(formatUsdCentral(1700, { low: 1100, high: 2300 })).toBe('$1,700 ($1,100–$2,300)');
  });

  it('picks the unit its high bound reads best in', () => {
    expect(formatDuration({ low: 60, high: 600 })).toBe('1–10 minutes');
    expect(formatDuration({ low: 6 * 3600, high: 19 * 3600 })).toBe('6–19 hours');
    expect(formatDuration({ low: 2 * 86400, high: 5 * 86400 })).toBe('2–5 days');
    expect(formatDuration({ low: 0, high: 0 })).toBe('under a minute');
    expect(formatDuration({ low: 30, high: 70 })).toBe('1 minute');
  });
});
