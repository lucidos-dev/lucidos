import { describe, it, expect } from 'vitest';
import { formatAgoPhrase, formatDurationPhrase, formatElapsed } from './formatTime';

/** The status toast's build timer is redrawn once a second, so the boundaries
 *  matter: a counter that skips a value, or renders a negative one, is read as a
 *  broken build rather than a running one. */
describe('formatElapsed', () => {
  it('counts seconds under a minute', () => {
    expect(formatElapsed(0)).toBe('0s');
    expect(formatElapsed(999)).toBe('0s');
    expect(formatElapsed(1000)).toBe('1s');
    expect(formatElapsed(59_999)).toBe('59s');
  });

  it('keeps seconds visible through the minute range', () => {
    // Seconds stay, because this is the range a build actually lives in and a
    // number that only moved once a minute would read as frozen.
    expect(formatElapsed(60_000)).toBe('1m 0s');
    expect(formatElapsed(134_000)).toBe('2m 14s');
    expect(formatElapsed(3_599_000)).toBe('59m 59s');
  });

  it('drops to minutes past an hour', () => {
    // Past an hour the seconds are noise, and the string stops churning.
    expect(formatElapsed(3_600_000)).toBe('1h 00m');
    expect(formatElapsed(3_780_000)).toBe('1h 03m');
    expect(formatElapsed(90_000_000)).toBe('25h 00m');
  });

  /** The caller derives this from clock arithmetic, so a value that has gone
   *  backwards is possible. "-3s" is worse than a momentarily stalled counter. */
  it('clamps a negative or nonsense duration to zero', () => {
    expect(formatElapsed(-5000)).toBe('0s');
    expect(formatElapsed(Number.NaN)).toBe('0s');
    expect(formatElapsed(Number.POSITIVE_INFINITY)).toBe('0s');
  });
});

/** This one reads inside a clause, so the words and the singular matter more
 *  than the boundaries do. */
describe('formatAgoPhrase', () => {
  const now = new Date('2026-08-27T12:00:00Z');
  const before = (ms: number) => new Date(now.getTime() - ms);

  it('spells out each unit, and says "just now" under a minute', () => {
    expect(formatAgoPhrase(before(0), now)).toBe('just now');
    expect(formatAgoPhrase(before(59_000), now)).toBe('just now');
    expect(formatAgoPhrase(before(180_000), now)).toBe('3 minutes ago');
    expect(formatAgoPhrase(before(8 * 3_600_000), now)).toBe('8 hours ago');
    expect(formatAgoPhrase(before(5 * 86_400_000), now)).toBe('5 days ago');
  });

  it('drops the plural at one of anything', () => {
    expect(formatAgoPhrase(before(60_000), now)).toBe('1 minute ago');
    expect(formatAgoPhrase(before(3_600_000), now)).toBe('1 hour ago');
    expect(formatAgoPhrase(before(86_400_000), now)).toBe('1 day ago');
  });

  it('reads a stamp from the future as just now', () => {
    // Two clocks, so this is reachable. "-2 minutes ago" is worse than a stamp
    // that has simply rounded to the present.
    expect(formatAgoPhrase(new Date(now.getTime() + 120_000), now)).toBe('just now');
  });
});

/** A span rather than a point, and it takes seconds because a caller reads what
 *  the server measured. */
describe('formatDurationPhrase', () => {
  it('spells out each unit and drops the plural at one', () => {
    expect(formatDurationPhrase(180)).toBe('3 minutes');
    expect(formatDurationPhrase(60)).toBe('1 minute');
    expect(formatDurationPhrase(8 * 3600)).toBe('8 hours');
    expect(formatDurationPhrase(3600)).toBe('1 hour');
    expect(formatDurationPhrase(5 * 86_400)).toBe('5 days');
    expect(formatDurationPhrase(86_400)).toBe('1 day');
  });

  it('says "under a minute" rather than counting seconds', () => {
    // It is read once, inside a clause. A second count there is precision the
    // reader has no use for.
    expect(formatDurationPhrase(0)).toBe('under a minute');
    expect(formatDurationPhrase(59)).toBe('under a minute');
  });

  it('holds the same floor for a negative or nonsense span', () => {
    expect(formatDurationPhrase(-30)).toBe('under a minute');
    expect(formatDurationPhrase(Number.NaN)).toBe('under a minute');
  });
});
