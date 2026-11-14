import { describe, it, expect, vi } from 'vitest';
import { HEARTBEAT_INTERVAL_MS, startHeartbeat, type HeartbeatDeps } from './heartbeat';

function harness() {
  const listeners = new Map<string, () => void>();
  const doc = {
    visibilityState: 'visible' as DocumentVisibilityState,
    addEventListener: vi.fn((type: string, fn: () => void) => listeners.set(type, fn)),
    removeEventListener: vi.fn((type: string) => listeners.delete(type)),
  };
  const invoke = vi.fn((_cmd: string, _args: Record<string, unknown>) => Promise.resolve());
  const timers: Array<{ fn: () => void; ms: number }> = [];
  const clearInterval = vi.fn();
  const deps: HeartbeatDeps = {
    invoke,
    doc: doc as unknown as HeartbeatDeps['doc'],
    setInterval: (fn, ms) => timers.push({ fn, ms }),
    clearInterval,
    pageId: 'doc-1',
  };
  return { deps, doc, invoke, timers, listeners, clearInterval };
}

describe('startHeartbeat', () => {
  it('beats at once, naming the document and its visibility', () => {
    const { deps, invoke } = harness();
    startHeartbeat(deps);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('heartbeat', { page: 'doc-1', visible: true });
  });

  it('beats on the one interval and sets up no second timer', () => {
    const { deps, invoke, timers } = harness();
    startHeartbeat(deps);
    expect(timers.map((t) => t.ms)).toEqual([HEARTBEAT_INTERVAL_MS]);
    timers[0].fn();
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it('reports hidden the moment visibility changes, before WebKit suspends it', () => {
    const { deps, doc, invoke, listeners } = harness();
    startHeartbeat(deps);
    doc.visibilityState = 'hidden';
    listeners.get('visibilitychange')!();
    expect(invoke).toHaveBeenLastCalledWith('heartbeat', { page: 'doc-1', visible: false });
    doc.visibilityState = 'visible';
    listeners.get('visibilitychange')!();
    expect(invoke).toHaveBeenLastCalledWith('heartbeat', { page: 'doc-1', visible: true });
  });

  it('stops the interval and the listener', () => {
    const { deps, listeners, clearInterval } = harness();
    startHeartbeat(deps)();
    expect(clearInterval).toHaveBeenCalledTimes(1);
    expect(listeners.size).toBe(0);
  });

  it('survives a rejected invoke', async () => {
    const { deps, invoke } = harness();
    invoke.mockReturnValue(Promise.reject(new Error('denied')));
    expect(() => startHeartbeat(deps)).not.toThrow();
    await Promise.resolve();
  });
});
