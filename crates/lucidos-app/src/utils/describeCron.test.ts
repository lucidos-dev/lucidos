import { describe, it, expect, vi, afterEach } from 'vitest';
import { describeCron, validateCron } from './describeCron';

describe('describeCron', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  // -- Frequency patterns --

  it('every minute', () => {
    expect(describeCron('0 * * * * *')).toBe('Every minute');
  });

  it('every N minutes', () => {
    expect(describeCron('0 */5 * * * *')).toBe('Every 5 minutes');
  });

  it('*/1 minutes = every minute', () => {
    expect(describeCron('0 */1 * * * *')).toBe('Every minute');
  });

  it('every hour', () => {
    expect(describeCron('0 0 * * * *')).toBe('Every hour');
  });

  it('every hour at :MM', () => {
    expect(describeCron('0 30 * * * *')).toBe('Every hour at :30');
  });

  it('every N hours', () => {
    expect(describeCron('0 0 */3 * * *')).toBe('Every 3 hours');
  });

  it('*/1 hours = every hour', () => {
    expect(describeCron('0 0 */1 * * *')).toBe('Every hour');
  });

  // -- Daily --

  it('daily at time', () => {
    expect(describeCron('0 0 8 * * *')).toBe('Daily at 08:00');
  });

  it('daily at time with minutes', () => {
    expect(describeCron('0 30 14 * * *')).toBe('Daily at 14:30');
  });

  it('daily at midnight', () => {
    expect(describeCron('0 0 0 * * *')).toBe('Daily at 00:00');
  });

  it('daily at noon', () => {
    expect(describeCron('0 0 12 * * *')).toBe('Daily at 12:00');
  });

  // -- Weekday patterns --

  it('weekdays (1-5)', () => {
    expect(describeCron('0 0 8 * * 1-5')).toBe('Weekdays at 08:00');
  });

  it('weekdays (MON-FRI)', () => {
    expect(describeCron('0 0 8 * * MON-FRI')).toBe('Weekdays at 08:00');
  });

  it('weekends (0,6)', () => {
    expect(describeCron('0 0 10 * * 0,6')).toBe('Weekends at 10:00');
  });

  it('weekends (SAT,SUN)', () => {
    expect(describeCron('0 0 10 * * SAT,SUN')).toBe('Weekends at 10:00');
  });

  it('specific weekday', () => {
    expect(describeCron('0 0 9 * * 3')).toBe('Every Wednesday at 09:00');
  });

  it('multiple specific weekdays', () => {
    expect(describeCron('0 0 9 * * 1,3,5')).toBe('Every Monday, Wednesday, Friday at 09:00');
  });

  it('weekday range', () => {
    expect(describeCron('0 0 9 * * 1-3')).toBe('Every Monday–Wednesday at 09:00');
  });

  // -- Monthly on Nth day (month is *) --

  it('monthly on specific day', () => {
    expect(describeCron('0 0 12 4 * *')).toBe('4th of every month at 12:00');
  });

  it('monthly on 1st', () => {
    expect(describeCron('0 0 9 1 * *')).toBe('1st of every month at 09:00');
  });

  it('monthly on 2nd', () => {
    expect(describeCron('0 30 10 2 * *')).toBe('2nd of every month at 10:30');
  });

  it('monthly on 3rd', () => {
    expect(describeCron('0 0 8 3 * *')).toBe('3rd of every month at 08:00');
  });

  it('monthly on 15th', () => {
    expect(describeCron('0 0 17 15 * *')).toBe('15th of every month at 17:00');
  });

  it('monthly on 21st', () => {
    expect(describeCron('0 0 8 21 * *')).toBe('21st of every month at 08:00');
  });

  it('monthly on 22nd', () => {
    expect(describeCron('0 0 8 22 * *')).toBe('22nd of every month at 08:00');
  });

  it('monthly on 23rd', () => {
    expect(describeCron('0 0 8 23 * *')).toBe('23rd of every month at 08:00');
  });

  it('monthly on 11th (special ordinal)', () => {
    expect(describeCron('0 0 8 11 * *')).toBe('11th of every month at 08:00');
  });

  it('monthly on 12th (special ordinal)', () => {
    expect(describeCron('0 0 8 12 * *')).toBe('12th of every month at 08:00');
  });

  it('monthly on 13th (special ordinal)', () => {
    expect(describeCron('0 0 8 13 * *')).toBe('13th of every month at 08:00');
  });

  // -- Specific date (day + month both set) --

  it('specific date in the future shows current year', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 2, 3, 10, 0)); // Mar 3, 2026 10:00
    expect(describeCron('0 15 12 4 3 *')).toBe('Mar 4, 2026 at 12:15');
  });

  it('specific date already passed this year shows next year', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 2, 5, 10, 0)); // Mar 5, 2026 10:00
    expect(describeCron('0 15 12 4 3 *')).toBe('Mar 4, 2027 at 12:15');
  });

  it('specific date on the same day but time already passed shows next year', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 2, 4, 13, 0)); // Mar 4, 2026 13:00 (after 12:15)
    expect(describeCron('0 15 12 4 3 *')).toBe('Mar 4, 2027 at 12:15');
  });

  it('specific date on the same day before the time shows current year', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 2, 4, 11, 0)); // Mar 4, 2026 11:00 (before 12:15)
    expect(describeCron('0 15 12 4 3 *')).toBe('Mar 4, 2026 at 12:15');
  });

  it('specific date in a different month', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 0, 15, 10, 0)); // Jan 15, 2026
    expect(describeCron('0 0 9 25 12 *')).toBe('Dec 25, 2026 at 09:00');
  });

  it('specific date in a past month rolls to next year', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 5, 1, 10, 0)); // Jun 1, 2026
    expect(describeCron('0 0 9 25 1 *')).toBe('Jan 25, 2027 at 09:00');
  });

  it('specific date at midnight', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 0, 1, 0, 0)); // Jan 1, 2026
    expect(describeCron('0 0 0 14 2 *')).toBe('Feb 14, 2026 at 00:00');
  });

  // -- Edge cases --

  it('invalid field count returns raw cron', () => {
    expect(describeCron('* * * *')).toBe('* * * *');
  });

  it('dom + dow both set with month', () => {
    expect(describeCron('0 0 12 15 6 1')).toBe('15th & Monday in Jun at 12:00');
  });

  it('dom + dow both set without month', () => {
    expect(describeCron('0 0 12 15 * 1')).toBe('15th & Monday at 12:00');
  });

  // A day-of-month field naming more than one day. `parseInt` reads only the
  // leading token, so each of these used to state a schedule the expression
  // does not have. The raw cron is the honest answer.

  it('a stepped day of month falls back to the raw cron', () => {
    expect(describeCron('0 0 12 */2 * *')).toBe('0 0 12 */2 * *');
  });

  it('a day-of-month list falls back rather than dropping every day but the first', () => {
    expect(describeCron('0 0 12 1,15 * *')).toBe('0 0 12 1,15 * *');
  });

  it('a day-of-month range falls back', () => {
    expect(describeCron('0 0 12 1-5 * *')).toBe('0 0 12 1-5 * *');
  });

  it('a day-of-month list beside a weekday falls back', () => {
    expect(describeCron('0 0 12 1,15 * 1')).toBe('0 0 12 1,15 * 1');
  });

  it('a stepped day of month with a month falls back', () => {
    expect(describeCron('0 0 12 */2 6 *')).toBe('0 0 12 */2 6 *');
  });
});

describe('validateCron', () => {
  it('valid 6-field cron returns null', () => {
    expect(validateCron('0 0 8 * * *')).toBeNull();
  });

  it('empty string returns error', () => {
    expect(validateCron('')).toBe('Cron expression is required');
  });

  it('wrong field count returns error', () => {
    expect(validateCron('0 0 8 * *')).toContain('Expected 6 fields');
  });

  it('value out of range', () => {
    expect(validateCron('0 60 8 * * *')).toContain('out of range');
  });

  it('allows named weekdays', () => {
    expect(validateCron('0 0 8 * * MON-FRI')).toBeNull();
  });

  it('allows named months', () => {
    expect(validateCron('0 0 8 1 JAN *')).toBeNull();
  });

  it('allows step values', () => {
    expect(validateCron('0 */5 * * * *')).toBeNull();
  });

  it('rejects step of 0', () => {
    expect(validateCron('0 */0 * * * *')).toContain('Invalid step');
  });

  it('allows ranges', () => {
    expect(validateCron('0 0 8 * * 1-5')).toBeNull();
  });

  it('rejects range values out of bounds', () => {
    expect(validateCron('0 0 8 * * 1-8')).toContain('out of range');
  });

  it('rejects invalid syntax', () => {
    // Letters in a numeric-only field (day-of-month)
    expect(validateCron('0 0 8 abc * *')).toContain('Invalid syntax');
  });
});
