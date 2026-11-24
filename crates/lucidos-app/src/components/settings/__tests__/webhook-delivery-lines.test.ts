import { describe, it, expect } from 'vitest';
import { lastDeliveryLine, lastRefusalLine } from '../webhookDelivery';


const NOW = new Date('2026-08-27T12:00:00Z');

function ago(hours: number): string {
  return new Date(NOW.getTime() - hours * 3_600_000).toISOString();
}

describe('lastDeliveryLine', () => {
  it('says how long ago the last delivery verified', () => {
    expect(lastDeliveryLine({ last_accepted_at: ago(8) }, NOW)).toBe('Last delivery 8 hours ago');
    expect(lastDeliveryLine({ last_accepted_at: ago(1) }, NOW)).toBe('Last delivery 1 hour ago');
  });

  it('says so plainly when nothing has ever verified', () => {
    expect(lastDeliveryLine({ last_accepted_at: null }, NOW)).toBe('No delivery has verified yet');
  });

  it('drops the clause on a stamp it cannot read', () => {
    expect(lastDeliveryLine({ last_accepted_at: 'not a timestamp' }, NOW)).toBeNull();
  });
});

describe('lastRefusalLine', () => {
  it('names the time and the reason', () => {
    expect(
      lastRefusalLine(
        { last_refused_at: ago(0.05), last_refusal_reason: 'signature did not match' },
        NOW,
      ),
    ).toBe('Last refused 3 minutes ago: signature did not match');
  });

  it('still reports the refusal when no reason was recorded', () => {
    expect(lastRefusalLine({ last_refused_at: ago(2), last_refusal_reason: null }, NOW)).toBe(
      'Last refused 2 hours ago',
    );
  });

  it('says nothing when no delivery has ever been refused', () => {
    expect(lastRefusalLine({ last_refused_at: null, last_refusal_reason: null }, NOW)).toBeNull();
  });

  it('is independent of the accepted stamp', () => {
    const hook = {
      last_accepted_at: ago(1),
      last_refused_at: ago(0.05),
      last_refusal_reason: 'bearer token did not match',
    };
    expect(lastDeliveryLine(hook, NOW)).toBe('Last delivery 1 hour ago');
    expect(lastRefusalLine(hook, NOW)).toBe('Last refused 3 minutes ago: bearer token did not match');
  });
});
