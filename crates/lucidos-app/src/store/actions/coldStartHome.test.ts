/**
 * A cold start with no stored focus opens Home, and a returning user keeps
 * their last thread (ADR 0411).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./threads', () => ({ focusThread: vi.fn() }));

import { coldStartFocus, openHomeOnColdStart } from './coldStartHome';
import { focusThread } from './threads';
import { focusedThreadId, threadMap, threadsLoaded } from '../store';
import { makeThreadState } from './threads-test-helpers';

describe('coldStartFocus', () => {
  it('opens Home when this device stored no focus', () => {
    expect(coldStartFocus({ storedFocus: false, focusedNow: null, homeId: 'home' })).toBe('home');
  });

  it('leaves a returning user on their stored thread', () => {
    expect(coldStartFocus({ storedFocus: true, focusedNow: 'last', homeId: 'home' })).toBeNull();
    expect(coldStartFocus({ storedFocus: true, focusedNow: null, homeId: 'home' })).toBeNull();
  });

  it('never overrides a focus the user made while the list loaded', () => {
    expect(coldStartFocus({ storedFocus: false, focusedNow: 'draft', homeId: 'home' })).toBeNull();
  });

  it('does nothing when there is no Home', () => {
    expect(coldStartFocus({ storedFocus: false, focusedNow: null, homeId: null })).toBeNull();
  });
});

describe('openHomeOnColdStart', () => {
  let stop: (() => void) | null = null;

  beforeEach(() => {
    vi.mocked(focusThread).mockClear();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    threadMap.value = new Map([
      ['home', makeThreadState('home', { meta: { title: 'Home', home: true } })],
    ]);
  });

  afterEach(() => {
    stop?.();
    stop = null;
    threadsLoaded.value = false;
    threadMap.value = new Map();
  });

  it('opens Home once, as the thread list first lands, without moving the pane', () => {
    stop = openHomeOnColdStart(false);
    expect(focusThread).not.toHaveBeenCalled();
    threadsLoaded.value = true;
    expect(focusThread).toHaveBeenCalledTimes(1);
    expect(focusThread).toHaveBeenCalledWith('home', { revealPane: false });
    threadsLoaded.value = false;
    threadsLoaded.value = true;
    expect(focusThread).toHaveBeenCalledTimes(1);
  });

  it('leaves a stored focus alone', () => {
    stop = openHomeOnColdStart(true);
    threadsLoaded.value = true;
    expect(focusThread).not.toHaveBeenCalled();
  });
});
