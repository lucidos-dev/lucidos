import { describe, it, expect } from 'vitest';
import { CONNECTION_POLL_INTERVAL_MS, HEALTH_PROBE_TIMEOUT_MS } from '../store';

describe('the health probe budget fits inside the poll interval', () => {
  it('keeps the deadline strictly below the interval', () => {
    expect(HEALTH_PROBE_TIMEOUT_MS).toBeLessThan(CONNECTION_POLL_INTERVAL_MS);
  });

  it('spends most of the interval on the probe rather than idling', () => {
    expect(HEALTH_PROBE_TIMEOUT_MS).toBeGreaterThanOrEqual(CONNECTION_POLL_INTERVAL_MS * 0.8);
  });
});
