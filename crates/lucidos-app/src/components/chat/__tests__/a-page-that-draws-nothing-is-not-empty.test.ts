/** A first page that draws nothing does not make the thread empty.
 *
 *  A terminal verdict needs the whole history loaded, and the skeleton covers
 *  the wait. See docs/glossary.md § Whole history loaded. */
import { describe, it, expect } from 'vitest';
import { emptyReason } from '../ThreadView';
import { wholeHistoryLoaded, threadIsLoadingNow } from '../threadSkeletonGate';

const ID = 'thread-1';

/** ThreadView's own wiring: one predicate feeds both the verdict and the gate. */
function verdict(opts: { hasOlderEvents: boolean; hasContent?: boolean; turnInFlight?: boolean; disconnected?: boolean }) {
  const historyLoaded = wholeHistoryLoaded(true, opts.hasOlderEvents);
  const disconnected = opts.disconnected ?? false;
  return {
    reason: emptyReason(false, historyLoaded, false, opts.hasContent ?? false, ID, disconnected, opts.turnInFlight ?? false),
    loading: threadIsLoadingNow({ hasExchanges: false, animating: false, eventsLoadFailed: false, historyLoaded, disconnected }),
  };
}

describe('a loaded page that draws nothing, with older pages on the server', () => {
  it('is loading, not empty', () => {
    expect(verdict({ hasOlderEvents: true })).toEqual({ reason: { kind: 'loading', threadId: ID }, loading: true });
  });

  it('is loading, not corrupt, when its content forms no exchange yet', () => {
    expect(verdict({ hasOlderEvents: true, hasContent: true }).reason.kind).toBe('loading');
  });

  it('is loading, not the working shimmer, mid-turn', () => {
    expect(verdict({ hasOlderEvents: true, turnInFlight: true }).reason.kind).toBe('loading');
  });

  it('says the workspace is unreachable when it is', () => {
    expect(verdict({ hasOlderEvents: true, disconnected: true })).toEqual({
      reason: { kind: 'disconnected', threadId: ID },
      loading: false,
    });
  });
});

describe('a loaded page with nothing older', () => {
  it.each([
    ['empty', {}],
    ['corrupt', { hasContent: true }],
    ['working', { turnInFlight: true }],
  ] as const)('reaches its terminal verdict, %s, with no skeleton over it', (kind, patch) => {
    const v = verdict({ hasOlderEvents: false, ...patch });
    expect(v.reason.kind).toBe(kind);
    expect(v.loading).toBe(false);
  });
});
