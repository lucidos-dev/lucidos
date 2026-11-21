// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type * as ScrollStateModule from './scrollState';

type Handle = typeof import('./scrollStateHandle');

/** A stand-in scrollState that records every call it receives, in order. */
function recordingScrollState(): { module: typeof ScrollStateModule; calls: string[] } {
  const calls: string[] = [];
  const module = {
    stopFollowingBottom: () => calls.push('stopFollowingBottom'),
    clearPendingEventScroll: () => calls.push('clearPendingEventScroll'),
    scrollToEventAndPulse: (id: string) => calls.push(`scrollToEventAndPulse:${id}`),
    scrollToChangeAndPulse: (id: string) => calls.push(`scrollToChangeAndPulse:${id}`),
    followSentMessage: () => calls.push('followSentMessage'),
    isEventInViewport: () => true,
    isReplyEndInViewport: () => true,
    lastExchangeOf: () => document.createElement('div'),
  } as unknown as typeof ScrollStateModule;
  return { module, calls };
}

describe('scrollStateHandle', () => {
  let handle: Handle;
  beforeEach(async () => {
    vi.resetModules();
    handle = await import('./scrollStateHandle');
  });

  it('queues calls made before the shell provides scrollState, and replays them in order', () => {
    handle.stopFollowingBottom();
    handle.scrollToEventAndPulse('ev-1');
    handle.clearPendingEventScroll();
    handle.scrollToChangeAndPulse('ch-1');
    handle.followSentMessage();
    const { module, calls } = recordingScrollState();
    expect(calls).toEqual([]);
    handle.provideScrollState(module);
    expect(calls).toEqual([
      'stopFollowingBottom',
      'scrollToEventAndPulse:ev-1',
      'clearPendingEventScroll',
      'scrollToChangeAndPulse:ch-1',
      'followSentMessage',
    ]);
  });

  it('replays a queued call once, never again on later calls', () => {
    handle.stopFollowingBottom();
    const { module, calls } = recordingScrollState();
    handle.provideScrollState(module);
    handle.followSentMessage();
    expect(calls).toEqual(['stopFollowingBottom', 'followSentMessage']);
  });

  it('forwards synchronously once scrollState is provided, inside the caller', () => {
    const { module, calls } = recordingScrollState();
    handle.provideScrollState(module);
    handle.stopFollowingBottom();
    // Read before any microtask: the focus retire must land before the focus moves.
    expect(calls).toEqual(['stopFollowingBottom']);
  });

  it('passes the deep link options through untouched', () => {
    const scrollToEventAndPulse = vi.fn();
    handle.provideScrollState({ scrollToEventAndPulse } as unknown as typeof ScrollStateModule);
    const opts = { onUnresolved: () => {} };
    handle.scrollToEventAndPulse('ev-2', opts);
    expect(scrollToEventAndPulse).toHaveBeenCalledWith('ev-2', opts);
  });

  it('answers the viewport questions as an empty document before the shell provides scrollState', () => {
    expect(handle.isEventInViewport('ev-1')).toBe(false);
    expect(handle.isReplyEndInViewport('t-1')).toBe(false);
    expect(handle.lastExchangeOf('t-1')).toBeNull();
  });

  it("answers the viewport questions with scrollState's answers once provided", () => {
    const { module } = recordingScrollState();
    handle.provideScrollState(module);
    expect(handle.isEventInViewport('ev-1')).toBe(true);
    expect(handle.isReplyEndInViewport('t-1')).toBe(true);
    expect(handle.lastExchangeOf('t-1')).toBeInstanceOf(HTMLElement);
  });
});
