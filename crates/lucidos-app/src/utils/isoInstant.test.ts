import { describe, it, expect } from 'vitest';
import { instantMicros } from './isoInstant';

describe('instantMicros', () => {
  it('orders a whole second against its own sub-second neighbours', () => {
    // The exact wire payload that misordered a thread's events. A lexical sort
    // puts `...:21Z` last, because `.` sorts before `Z`.
    const wire = [
      '2026-08-28T07:19:20.980Z',
      '2026-08-28T07:19:20.990Z',
      '2026-08-28T07:19:21Z',
      '2026-08-28T07:19:21.010Z',
      '2026-08-28T07:19:21.020Z',
    ];
    expect([...wire].sort()).not.toEqual(wire);
    const byInstant = [...wire].sort((a, b) => instantMicros(a)! - instantMicros(b)!);
    expect(byInstant).toEqual(wire);
  });

  it('orders across producers writing different sub-second widths', () => {
    // Postgres keeps microseconds, so the engine can emit 6 digits; the
    // frontend's own `toISOString` always emits 3.
    expect(instantMicros('2026-08-28T07:19:21.010500Z')!).toBeGreaterThan(
      instantMicros('2026-08-28T07:19:21.010Z')!,
    );
    expect(instantMicros('2026-08-28T07:19:21.010500Z')!).toBeLessThan(
      instantMicros('2026-08-28T07:19:21.011Z')!,
    );
  });

  it('reads equal instants written at different widths as equal', () => {
    expect(instantMicros('2026-08-28T07:19:21Z')).toBe(instantMicros('2026-08-28T07:19:21.000Z'));
    expect(instantMicros('2026-08-28T07:19:21.010Z')).toBe(
      instantMicros('2026-08-28T07:19:21.010000Z'),
    );
  });

  it('keeps the fraction on a zoned stamp and before the epoch', () => {
    expect(instantMicros('2026-08-28T09:19:21.010+02:00')).toBe(
      instantMicros('2026-08-28T07:19:21.010Z'),
    );
    expect(instantMicros('1969-12-31T23:59:59.500Z')).toBe(-500_000);
  });

  it('counts microseconds exactly, with no float drift', () => {
    const base = instantMicros('2026-08-28T07:19:21Z')!;
    expect(Number.isSafeInteger(base)).toBe(true);
    expect(instantMicros('2026-08-28T07:19:21.000001Z')).toBe(base + 1);
  });

  it('returns null for an absent or unparsable value', () => {
    expect(instantMicros(undefined)).toBeNull();
    expect(instantMicros(null)).toBeNull();
    expect(instantMicros('')).toBeNull();
    expect(instantMicros('not a timestamp')).toBeNull();
  });
});
