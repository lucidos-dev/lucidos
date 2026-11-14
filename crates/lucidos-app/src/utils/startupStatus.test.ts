import { describe, it, expect, vi } from 'vitest';
import { startStartupStatusPolling, STARTUP_STATUS_POLL_MS } from './startupStatus';

/** A harness that captures the interval callback instead of scheduling it, so a
 *  test can advance the poll by hand. */
function harness(invoke: (cmd: 'startup_status') => Promise<unknown>) {
  const setStatus = vi.fn();
  let tick: (() => void) | null = null;
  const setInterval = vi.fn((fn: () => void) => {
    tick = fn;
    return 7;
  });
  const id = startStartupStatusPolling({ invoke, setStatus, setInterval });
  return { setStatus, setInterval, id, tick: () => tick?.() };
}

/** Let the poller's promise chain settle. */
const settle = () => new Promise((r) => setTimeout(r, 0));

describe('startStartupStatusPolling', () => {
  it('asks immediately and pushes the label onto the splash', async () => {
    // The immediate ask matters: a window that opens after the service has
    // already been struggling must say so on its first frame, not a second in.
    const invoke = vi.fn().mockResolvedValue('Waiting for the background service… (42s)');
    const h = harness(invoke);
    await settle();

    expect(invoke).toHaveBeenCalledWith('startup_status');
    expect(h.setStatus).toHaveBeenCalledWith('Waiting for the background service… (42s)');
    expect(h.setInterval).toHaveBeenCalledWith(expect.any(Function), STARTUP_STATUS_POLL_MS);
    expect(h.id).toBe(7);
  });

  it('keeps pushing each new label as the wait runs', async () => {
    // The whole point of the poll: a line that moves is what distinguishes a
    // start that is working from one that is wedged.
    const invoke = vi
      .fn()
      .mockResolvedValueOnce('Starting Lucidos…')
      .mockResolvedValueOnce('Waiting for the background service… (9s)')
      .mockResolvedValueOnce('Waiting for the background service… (10s)');
    const h = harness(invoke);
    await settle();
    h.tick();
    await settle();
    h.tick();
    await settle();

    expect(h.setStatus.mock.calls.map((c) => c[0])).toEqual([
      'Starting Lucidos…',
      'Waiting for the background service… (9s)',
      'Waiting for the background service… (10s)',
    ]);
  });

  it('leaves the splash alone when the bridge fails, and keeps polling', async () => {
    // A dead IPC bridge must not blank the splash or print an error onto it:
    // there is no app mounted here, so no toast can render, and `invoke` already
    // reports the bridge to the engine log itself.
    const invoke = vi.fn().mockRejectedValue(new Error('Command not allowed by ACL'));
    const h = harness(invoke);
    await settle();

    expect(h.setStatus).not.toHaveBeenCalled();
    // Still scheduled: the next tick retries rather than giving up.
    expect(h.setInterval).toHaveBeenCalledTimes(1);
    h.tick();
    await settle();
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it('ignores an answer that is not a usable label', async () => {
    // Mid-update the desktop binary can be older than this bundle. Keeping the
    // previous line beats blanking the splash or rendering "undefined".
    for (const answer of [undefined, null, '', 42, { label: 'x' }]) {
      const h = harness(vi.fn().mockResolvedValue(answer));
      await settle();
      expect(h.setStatus, `answer ${JSON.stringify(answer)}`).not.toHaveBeenCalled();
    }
  });
});
