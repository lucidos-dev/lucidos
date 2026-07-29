import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import { unreadNotifications, unreadCount } from '../store';
import type { Notification } from '../types';

// `isTauri` is the one context gate the sync consults at call time (the desktop
// app drives a native dock badge instead of the web Badging API). Default off.
vi.mock('../../utils/platform', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/platform')>()),
  isTauri: vi.fn(() => false),
}));

const { applyAppBadge, syncWorkspaceAppBadge } = await import('./app-badge');
const { isTauri } = await import('../../utils/platform');

let setAppBadge: ReturnType<typeof vi.fn>;
let clearAppBadge: ReturnType<typeof vi.fn>;

/** Install a fake Badging API on `navigator` (absent in the test runtime, as it
 *  is in every browser that doesn't implement it). */
function installBadgingApi(): void {
  setAppBadge = vi.fn(() => Promise.resolve());
  clearAppBadge = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, 'setAppBadge', { value: setAppBadge, configurable: true });
  Object.defineProperty(navigator, 'clearAppBadge', { value: clearAppBadge, configurable: true });
}

function removeBadgingApi(): void {
  Reflect.deleteProperty(navigator, 'setAppBadge');
  Reflect.deleteProperty(navigator, 'clearAppBadge');
}

function makeUnread(n: number): Notification[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `u${i}`,
    title: `Notification ${i}`,
    message: `Message ${i}`,
    read: false,
    created_at: new Date().toISOString(),
  }));
}

beforeEach(() => {
  installBadgingApi();
  (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(false);
  unreadNotifications.value = { status: 'not-loaded' };
});

afterEach(() => {
  removeBadgingApi();
});

describe('applyAppBadge', () => {
  it('sets a positive count and clears a non-positive one', () => {
    applyAppBadge(3);
    expect(setAppBadge).toHaveBeenCalledWith(3);
    expect(clearAppBadge).not.toHaveBeenCalled();

    applyAppBadge(0);
    expect(clearAppBadge).toHaveBeenCalledTimes(1);
    expect(setAppBadge).toHaveBeenCalledTimes(1);
  });

  it('no-ops when the Badging API is unavailable', () => {
    removeBadgingApi();
    expect(() => applyAppBadge(2)).not.toThrow();
  });
});

describe('syncWorkspaceAppBadge', () => {
  it('mirrors the unread set the bell projects', () => {
    unreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenCalledWith(2);
  });

  it('clears when the unread set is empty or has not loaded — the bell shows no count either', () => {
    unreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(1);

    unreadNotifications.value = { status: 'not-loaded' };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
    expect(setAppBadge).not.toHaveBeenCalled();
  });

  it('re-asserts unconditionally — the same count written twice writes the icon twice', () => {
    // Load-bearing, not redundant: the icon badge is an EXTERNALLY written
    // surface (iOS writes it from the push payload's `app_badge` in its parent
    // process; the SW `push` handler writes it on Chrome/Android), so the page
    // has to be able to overwrite a value it never observed changing. A sync
    // that skipped a write when its own count was unchanged would leave a
    // push-written icon reading 1 next to a bell reading 0.
    unreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
  });

  it('does not touch the web badge under Tauri (native dock badge owns it)', () => {
    (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: makeUnread(4) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).not.toHaveBeenCalled();
    expect(clearAppBadge).not.toHaveBeenCalled();
  });
});

describe('why the badge needs an explicit re-assert', () => {
  it('an effect on unreadCount does NOT re-run when a reload lands the same count', () => {
    // The signals cutoff: a computed whose recomputed value is equal does not
    // notify its subscribers. So the `unreadCount` effect in store/effects.ts
    // covers only the CHANGE case — a resume-time reload landing the same count
    // re-runs nothing, and an icon badge written behind the page's back (by a
    // push) would never be corrected. This test pins the mechanism so nobody
    // "simplifies" the explicit re-asserts back out.
    unreadNotifications.value = { status: 'loaded', data: [] };
    let runs = 0;
    const dispose = effect(() => {
      unreadCount.value;
      runs++;
    });
    expect(runs).toBe(1);

    // A fresh array with the same length — the signal changed, the count didn't.
    unreadNotifications.value = { status: 'loaded', data: [] };
    expect(runs).toBe(1);

    // A real change does re-run it.
    unreadNotifications.value = { status: 'loaded', data: makeUnread(1) };
    expect(runs).toBe(2);
    dispose();
  });
});
