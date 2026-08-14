import { describe, it, expect, beforeEach } from 'vitest';
import {
  SKELETON_HOLD_EVENT_COUNT,
  forcedSkeletonThreadId,
  releaseForcedSkeleton,
  shouldHoldForSkeleton,
  showThreadSkeletonNow,
  threadIsLoadingNow,
  type ThreadLoadingState,
} from '../threadSkeletonGate';

/** Mid-open: nothing on screen, nothing terminal to say. */
const LOADING: ThreadLoadingState = {
  hasExchanges: false,
  animating: false,
  eventsLoadFailed: false,
  eventsLoaded: false,
  disconnected: false,
};

describe('threadIsLoadingNow', () => {
  it('is true with nothing on screen and no terminal state', () => {
    expect(threadIsLoadingNow(LOADING)).toBe(true);
  });

  it('is false once exchanges render', () => {
    expect(threadIsLoadingNow({ ...LOADING, hasExchanges: true })).toBe(false);
  });

  it.each([
    ['the compose to thread FLIP', { animating: true }],
    ['a failed load', { eventsLoadFailed: true }],
    ['a loaded but empty thread', { eventsLoaded: true }],
    ['an unreachable workspace', { disconnected: true }],
  ] as const)('is false for %s', (_name, patch) => {
    expect(threadIsLoadingNow({ ...LOADING, ...patch })).toBe(false);
  });
});

describe('showThreadSkeletonNow', () => {
  it('waits for a clock: neither the delay nor a force means no skeleton', () => {
    expect(showThreadSkeletonNow(LOADING, false, false)).toBe(false);
  });

  it('shows on the delay gate alone', () => {
    expect(showThreadSkeletonNow(LOADING, true, false)).toBe(true);
  });

  it('shows on a forced raise alone, which is the point of the flag', () => {
    expect(showThreadSkeletonNow(LOADING, false, true)).toBe(true);
  });

  it.each([
    ['content', { hasExchanges: true }],
    ['a loaded but empty thread', { eventsLoaded: true }],
    ['a failed load', { eventsLoadFailed: true }],
    ['an unreachable workspace', { disconnected: true }],
  ] as const)('never covers %s, even when forced', (_name, patch) => {
    expect(showThreadSkeletonNow({ ...LOADING, ...patch }, true, true)).toBe(false);
  });
});

describe('shouldHoldForSkeleton', () => {
  const big = SKELETON_HOLD_EVENT_COUNT;

  it('holds for a big snapshot on the focused, empty thread', () => {
    expect(shouldHoldForSkeleton({ focused: true, snapshotEventCount: big, hasContent: false })).toBe(true);
  });

  it('lets a small snapshot render straight through', () => {
    expect(shouldHoldForSkeleton({ focused: true, snapshotEventCount: big - 1, hasContent: false })).toBe(false);
  });

  it('never holds a thread the user is not looking at', () => {
    // loadAllThreads fans this same load out over every active and saved
    // thread. None of those are on screen, so none may spend a frame.
    expect(shouldHoldForSkeleton({ focused: false, snapshotEventCount: big * 10, hasContent: false })).toBe(false);
  });

  it('never holds a thread that already shows content', () => {
    // SSE beat the snapshot: the turns are already painted, so there is
    // nothing for a skeleton to cover.
    expect(shouldHoldForSkeleton({ focused: true, snapshotEventCount: big * 10, hasContent: true })).toBe(false);
  });
});

describe('releaseForcedSkeleton', () => {
  beforeEach(() => { forcedSkeletonThreadId.value = null; });

  it('clears its own claim', () => {
    forcedSkeletonThreadId.value = 't1';
    releaseForcedSkeleton('t1');
    expect(forcedSkeletonThreadId.value).toBe(null);
  });

  it('leaves a newer load\'s claim alone', () => {
    forcedSkeletonThreadId.value = 't2';
    releaseForcedSkeleton('t1');
    expect(forcedSkeletonThreadId.value).toBe('t2');
  });
});
