/**
 * The connection dot's whole timing contract is two numbers, and they are only
 * meaningful against each other. `HEALTH_PROBE_TIMEOUT_MS` was 3s against a 5s
 * poll for no stated reason, which on a phone reaching a laptop over cellular
 * and a Tailscale tunnel is tight enough that a healthy engine reads as an
 * outage. Raising it has one hard ceiling: it must stay strictly below the poll
 * interval, or the timer starts a second probe before the first has given up and
 * they queue on the same HTTP/2 connection, manufacturing the outage the dot
 * exists to report.
 *
 * Pinned here rather than left to a comment because the two constants have
 * different consumers (`hooks/useStartup.ts` and `api/client/chat.ts`) and
 * neither can see the relation from where it sits.
 */
import { describe, it, expect } from 'vitest';
import { CONNECTION_POLL_INTERVAL_MS, HEALTH_PROBE_TIMEOUT_MS } from '../store';

describe('the health probe budget fits inside the poll interval', () => {
  it('keeps the deadline strictly below the interval', () => {
    expect(HEALTH_PROBE_TIMEOUT_MS).toBeLessThan(CONNECTION_POLL_INTERVAL_MS);
  });

  // The other direction: a deadline far under the interval wastes tolerance for
  // nothing, which is what 3s did. Not a precise claim about the right value,
  // just a floor that keeps a future "make it snappier" edit from silently
  // reintroducing the reported symptom.
  it('spends most of the interval on the probe rather than idling', () => {
    expect(HEALTH_PROBE_TIMEOUT_MS).toBeGreaterThanOrEqual(CONNECTION_POLL_INTERVAL_MS * 0.8);
  });
});
