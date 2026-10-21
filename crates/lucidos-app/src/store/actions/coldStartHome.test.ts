/**
 * A cold start with no stored focus opens Home, and a returning user keeps
 * their last thread (ADR 0411).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./threads', () => ({ focusThread: vi.fn(), focusThreadOrBootstrap: vi.fn() }));

import { coldStartFocus, offerWithheldThread, openHomeOnColdStart } from './coldStartHome';
import { focusThread, focusThreadOrBootstrap } from './threads';
import { focusedThreadId, threadMap, threadsLoaded, toasts } from '../store';
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

describe('offerWithheldThread', () => {
  let stop: (() => void) | null = null;

  beforeEach(() => {
    vi.mocked(focusThread).mockClear();
    vi.mocked(focusThreadOrBootstrap).mockClear();
    toasts.value = [];
    threadsLoaded.value = false;
    threadMap.value = new Map([
      ['t1', makeThreadState('t1', { meta: { title: 'Widget layout' } })],
    ]);
  });

  afterEach(() => {
    stop?.();
    stop = null;
    toasts.value = [];
    threadsLoaded.value = false;
    threadMap.value = new Map();
  });

  it('offers the withheld thread once, by title, as the list lands', () => {
    stop = offerWithheldThread('t1');
    expect(toasts.value).toHaveLength(0);
    threadsLoaded.value = true;
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('“Widget layout”');
    threadsLoaded.value = false;
    threadsLoaded.value = true;
    expect(toasts.value).toHaveLength(1);
  });

  it('opens the thread from the offer, revealing its pane', () => {
    stop = offerWithheldThread('t1');
    threadsLoaded.value = true;
    toasts.value[0].action?.onClick();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('t1');
  });

  it('offers nothing when the launch withheld nothing', () => {
    stop = offerWithheldThread(null);
    threadsLoaded.value = true;
    expect(toasts.value).toHaveLength(0);
  });
});
