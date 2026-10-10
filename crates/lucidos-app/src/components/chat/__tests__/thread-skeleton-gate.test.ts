/** The transcript's skeleton answers to ONE clock, the shared delay gate.
 *
 *  A second clock lived here for a day, a hold that raised the skeleton ahead
 *  of the gate. ADR 0081 records why it is gone: it could only ever show the
 *  skeleton instantly and then block the paint, which is the bug it was
 *  reported as. */
import { describe, it, expect } from 'vitest';
import {
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
  it('shows nothing until the delay gate has elapsed', () => {
    // The whole promise of the gate, and the one the hold used to break: a
    // load that finishes inside 300ms never puts a skeleton on screen.
    expect(showThreadSkeletonNow(LOADING, false)).toBe(false);
  });

  it('shows once the gate elapses on a load still in flight', () => {
    expect(showThreadSkeletonNow(LOADING, true)).toBe(true);
  });

  it.each([
    ['content', { hasExchanges: true }],
    ['a loaded but empty thread', { eventsLoaded: true }],
    ['a failed load', { eventsLoadFailed: true }],
    ['an unreachable workspace', { disconnected: true }],
  ] as const)('never covers %s', (_name, patch) => {
    expect(showThreadSkeletonNow({ ...LOADING, ...patch }, true)).toBe(false);
  });
});
