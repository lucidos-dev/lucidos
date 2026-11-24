import { describe, it, expect } from 'vitest';
import { formatElapsed } from './elapsed';

describe('formatElapsed', () => {
  it('counts seconds under a minute', () => {
    expect(formatElapsed(0)).toBe('0s');
    expect(formatElapsed(999)).toBe('0s');
    expect(formatElapsed(1000)).toBe('1s');
    expect(formatElapsed(59_999)).toBe('59s');
  });

  it('keeps seconds visible through the minute range', () => {
    expect(formatElapsed(60_000)).toBe('1m 0s');
    expect(formatElapsed(134_000)).toBe('2m 14s');
    expect(formatElapsed(3_599_000)).toBe('59m 59s');
  });

  it('drops to minutes past an hour', () => {
    expect(formatElapsed(3_600_000)).toBe('1h 00m');
    expect(formatElapsed(3_780_000)).toBe('1h 03m');
    expect(formatElapsed(90_000_000)).toBe('25h 00m');
  });

  it('clamps a negative or nonsense duration to zero', () => {
    expect(formatElapsed(-5000)).toBe('0s');
    expect(formatElapsed(Number.NaN)).toBe('0s');
    expect(formatElapsed(Number.POSITIVE_INFINITY)).toBe('0s');
  });
});
