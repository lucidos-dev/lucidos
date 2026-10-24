/**
 * A launch never restores a thread the previous session died showing, and a
 * clean exit never trips the guard.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  RESTORE_GUARD_KEY,
  RESTORE_GUARD_SURVIVAL_MS,
  armRestoreGuard,
  installRestoreGuard,
  readGuardedFocus,
  restoredFocus,
  takeWithheldThread,
} from './restoreGuard';

const FOCUS_KEY = 'test-focused-thread';

function setVisibility(state: DocumentVisibilityState): void {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

describe('restoredFocus', () => {
  it('withholds a stored focus the marker names', () => {
    expect(restoredFocus('t1', 't1')).toEqual({ focus: null, withheld: 't1' });
  });

  it('restores the focus when the marker names another thread or is absent', () => {
    expect(restoredFocus('t1', 't2')).toEqual({ focus: 't1', withheld: null });
    expect(restoredFocus('t1', null)).toEqual({ focus: 't1', withheld: null });
  });

  it('has nothing to withhold without a stored focus', () => {
    expect(restoredFocus(null, 't1')).toEqual({ focus: null, withheld: null });
  });
});

describe('readGuardedFocus', () => {
  beforeEach(() => {
    localStorage.clear();
    takeWithheldThread();
  });

  it('drops a withheld focus from storage and offers it once', () => {
    localStorage.setItem(FOCUS_KEY, 't1');
    localStorage.setItem(RESTORE_GUARD_KEY, 't1');
    expect(readGuardedFocus(FOCUS_KEY)).toBeNull();
    expect(localStorage.getItem(FOCUS_KEY)).toBeNull();
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBeNull();
    expect(takeWithheldThread()).toBe('t1');
    expect(takeWithheldThread()).toBeNull();
  });

  it('restores an unguarded focus and withholds nothing', () => {
    localStorage.setItem(FOCUS_KEY, 't1');
    expect(readGuardedFocus(FOCUS_KEY)).toBe('t1');
    expect(localStorage.getItem(FOCUS_KEY)).toBe('t1');
    expect(takeWithheldThread()).toBeNull();
  });
});

describe('arming', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('names the focused thread until the page survives the window', () => {
    armRestoreGuard('t1');
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t1');
    vi.advanceTimersByTime(RESTORE_GUARD_SURVIVAL_MS - 1);
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t1');
    vi.advanceTimersByTime(1);
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBeNull();
  });

  it('restarts the window on a new focus', () => {
    armRestoreGuard('t1');
    vi.advanceTimersByTime(RESTORE_GUARD_SURVIVAL_MS - 1);
    armRestoreGuard('t2');
    vi.advanceTimersByTime(1);
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t2');
  });

  it('clears the marker when the focus goes', () => {
    armRestoreGuard('t1');
    armRestoreGuard(null);
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBeNull();
  });
});

describe('installRestoreGuard', () => {
  let stop: (() => void) | null = null;
  let focus: string | null = 't1';

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    focus = 't1';
    setVisibility('visible');
    stop = installRestoreGuard(() => focus);
  });
  afterEach(() => {
    stop?.();
    stop = null;
    vi.useRealTimers();
  });

  it('arms for the focus it boots with', () => {
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t1');
  });

  it('clears the marker on pagehide, so a reload restores the thread', () => {
    window.dispatchEvent(new Event('pagehide'));
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBeNull();
  });

  it('clears the marker when the page is hidden, and arms again on return', () => {
    setVisibility('hidden');
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBeNull();
    vi.advanceTimersByTime(RESTORE_GUARD_SURVIVAL_MS);
    focus = 't2';
    setVisibility('visible');
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t2');
  });

  it('stops listening after teardown', () => {
    stop?.();
    stop = null;
    window.dispatchEvent(new Event('pagehide'));
    expect(localStorage.getItem(RESTORE_GUARD_KEY)).toBe('t1');
  });
});
