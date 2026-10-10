/**
 * Regression: the connection dot must NOT flash red on a page refresh while the
 * very first `/health` poll is in flight. The dot is driven by `connectionStatus`,
 * and the old initial value was `'disconnected'` — which renders the dot red AND
 * blinking (see the `.status-dot.disconnected` rule in styles/panels/shell.css)
 * for the whole network round-trip before the first check resolves. The pristine
 * state is genuinely unknown, so it must be `'connecting'`, which falls back to
 * the neutral grey base `.status-dot` (no red, no blink) until the poll answers.
 *
 * This reads the value from a FRESHLY re-imported store module so a sibling test
 * that mutates the singleton can't mask the initial value.
 */
import { describe, it, expect, vi } from 'vitest';

describe('connectionStatus initial state', () => {
  it('starts as "connecting" so a refresh never flashes the dot red', async () => {
    vi.resetModules();
    const { connectionStatus } = await import('../store');
    expect(connectionStatus.value).toBe('connecting');
  });
});
