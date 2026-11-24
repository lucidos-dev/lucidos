import { describe, it, expect } from 'vitest';
import {
  wholeHistoryLoaded,
  showThreadSkeletonNow,
  threadIsLoadingNow,
  type ThreadLoadingState,
} from '../threadSkeletonGate';

const LOADING: ThreadLoadingState = {
  hasExchanges: false,
  animating: false,
  eventsLoadFailed: false,
  historyLoaded: false,
  disconnected: false,
};

describe('wholeHistoryLoaded', () => {
  it('needs the first page loaded', () => {
    expect(wholeHistoryLoaded(false, false)).toBe(false);
  });

  it('needs nothing older left on the server', () => {
    expect(wholeHistoryLoaded(true, true)).toBe(false);
  });

  it('holds once loaded with nothing older', () => {
    expect(wholeHistoryLoaded(true, false)).toBe(true);
  });
});

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
    ['a thread whose whole history is empty', { historyLoaded: true }],
    ['an unreachable workspace', { disconnected: true }],
  ] as const)('is false for %s', (_name, patch) => {
    expect(threadIsLoadingNow({ ...LOADING, ...patch })).toBe(false);
  });
});

describe('showThreadSkeletonNow', () => {
  it('shows nothing until the delay gate has elapsed', () => {
    expect(showThreadSkeletonNow(LOADING, false)).toBe(false);
  });

  it('shows once the gate elapses on a load still in flight', () => {
    expect(showThreadSkeletonNow(LOADING, true)).toBe(true);
  });

  it.each([
    ['content', { hasExchanges: true }],
    ['a thread whose whole history is empty', { historyLoaded: true }],
    ['a failed load', { eventsLoadFailed: true }],
    ['an unreachable workspace', { disconnected: true }],
  ] as const)('never covers %s', (_name, patch) => {
    expect(showThreadSkeletonNow({ ...LOADING, ...patch }, true)).toBe(false);
  });
});
