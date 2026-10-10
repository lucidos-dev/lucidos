/**
 * How one frame reaches the listeners an app registered with `lucidos.sse.on`.
 *
 * The direct transport is stubbed with a fake `EventSource`, so these cases
 * drive `handleFrame` exactly as a real frame does.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { sse } from './sse';

class FakeEventSource {
  static last: FakeEventSource | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public readonly url: string) {
    FakeEventSource.last = this;
  }
  close(): void {}
}

function deliver(data: string): void {
  FakeEventSource.last!.onmessage!({ data });
}

describe('lucidos.sse dispatch', () => {
  beforeEach(() => {
    vi.stubGlobal('EventSource', FakeEventSource);
    vi.stubGlobal('SharedWorker', undefined);
    vi.useFakeTimers();
    sse.connect();
  });

  afterEach(() => {
    sse.disconnect();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  // A listener that throws must not stop later listeners, and its error must
  // still reach the console.
  it('a listener that throws costs no other listener its frame', () => {
    const after = vi.fn();
    const wildcard = vi.fn();
    const offs = [
      sse.on('PreferencesChanged', () => { throw new Error('app bug'); }),
      sse.on('PreferencesChanged', after),
      sse.on('*', wildcard),
    ];
    deliver(JSON.stringify({ type: 'PreferencesChanged', data: { key: 'theme' } }));
    expect(after).toHaveBeenCalledWith({ key: 'theme' }, expect.anything());
    expect(wildcard).toHaveBeenCalledOnce();
    // Rethrown on its own task, so the console still reports it.
    expect(vi.getTimerCount()).toBe(1);
    for (const off of offs) off();
  });

  it('ignores a frame that is not an event', () => {
    const wildcard = vi.fn();
    const off = sse.on('*', wildcard);
    deliver('not json');
    deliver('null');
    deliver(JSON.stringify({ data: {} }));
    expect(wildcard).not.toHaveBeenCalled();
    off();
  });
});
