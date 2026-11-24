import { describe, it, expect, vi } from 'vitest';
import type { BootAction } from './bootSplash';
import { startStartupStatusPolling, STARTUP_STATUS_POLL_MS, type StartupCommand } from './startupStatus';

function harness(invoke: (cmd: StartupCommand) => Promise<unknown>) {
  const setStatus = vi.fn();
  const setAction = vi.fn<(action: BootAction | null) => void>();
  let tick: (() => void) | null = null;
  const setInterval = vi.fn((fn: () => void) => {
    tick = fn;
    return 7;
  });
  const id = startStartupStatusPolling({ invoke, setStatus, setAction, setInterval });
  return { setStatus, setAction, setInterval, id, tick: () => tick?.() };
}

const report = (label: string, trustButton: string | null = null) => ({ label, trustButton });

const settle = () => new Promise((r) => setTimeout(r, 0));

describe('startStartupStatusPolling', () => {
  it('asks immediately and pushes the label onto the splash', async () => {
    const invoke = vi.fn().mockResolvedValue(report('Waiting for the background service… (42s)'));
    const h = harness(invoke);
    await settle();

    expect(invoke).toHaveBeenCalledWith('startup_status');
    expect(h.setStatus).toHaveBeenCalledWith('Waiting for the background service… (42s)');
    expect(h.setAction).toHaveBeenCalledWith(null);
    expect(h.setInterval).toHaveBeenCalledWith(expect.any(Function), STARTUP_STATUS_POLL_MS);
    expect(h.id).toBe(7);
  });

  it('keeps pushing each new label as the wait runs', async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce(report('Starting Lucidos…'))
      .mockResolvedValueOnce(report('Waiting for the background service… (9s)'))
      .mockResolvedValueOnce(report('Waiting for the background service… (10s)'));
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

  it('offers the Trust button while the Trust step waits, and its press asks again', async () => {
    const invoke = vi.fn(async (cmd: StartupCommand): Promise<unknown> =>
      cmd === 'startup_status' ? report('Trust the certificate once.', 'Trust certificate') : undefined);
    const h = harness(invoke);
    await settle();

    const action = h.setAction.mock.lastCall?.[0];
    expect(action?.label).toBe('Trust certificate');
    action?.onPress();
    expect(h.setAction).toHaveBeenLastCalledWith(null);
    expect(invoke).toHaveBeenCalledWith('trust_loopback_ca');
    await settle();
    await settle();
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['startup_status', 'trust_loopback_ca', 'startup_status']);
  });

  it('asks again after a press that failed, so the reason reaches the screen', async () => {
    const invoke = vi.fn(async (cmd: StartupCommand): Promise<unknown> => {
      if (cmd === 'trust_loopback_ca') throw new Error('User canceled');
      return report('User canceled.\nLucidos opens once its local certificate is trusted.', 'Try again');
    });
    const h = harness(invoke);
    await settle();
    h.setAction.mock.lastCall?.[0]?.onPress();
    await settle();
    await settle();
    expect(h.setStatus).toHaveBeenCalledTimes(2);
    expect(h.setAction.mock.lastCall?.[0]?.label).toBe('Try again');
  });

  it('leaves the splash alone when the bridge fails, and keeps polling', async () => {
    const invoke = vi.fn().mockRejectedValue(new Error('Command not allowed by ACL'));
    const h = harness(invoke);
    await settle();

    expect(h.setStatus).not.toHaveBeenCalled();
    expect(h.setInterval).toHaveBeenCalledTimes(1);
    h.tick();
    await settle();
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it('ignores an answer that is not a usable report', async () => {
    for (const answer of [undefined, null, '', 42, 'Starting Lucidos…', { label: '' }, { label: 7 }]) {
      const h = harness(vi.fn().mockResolvedValue(answer));
      await settle();
      expect(h.setStatus, `answer ${JSON.stringify(answer)}`).not.toHaveBeenCalled();
      expect(h.setAction, `answer ${JSON.stringify(answer)}`).not.toHaveBeenCalled();
    }
  });
});
