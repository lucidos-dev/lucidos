import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { onPageHide, onPageWake, _resetPageVisitForTesting } from './pageVisit';
import { installFakePage } from './__tests__/fakePage';

describe('pageVisit', () => {
  let page: ReturnType<typeof installFakePage>;

  beforeEach(() => {
    _resetPageVisitForTesting();
    page = installFakePage();
  });
  afterEach(() => {
    _resetPageVisitForTesting();
    page.restore();
  });

  it('fires each side once per real transition, whatever the burst', () => {
    // The premise of the whole module: one background is several events and one
    // wake is several more, and a consumer that repositions the reader or
    // commits a write must see exactly one of each.
    const wake = vi.fn();
    const hide = vi.fn();
    onPageWake(wake);
    onPageHide(hide);

    page.background();
    expect(hide).toHaveBeenCalledTimes(1);
    expect(wake).toHaveBeenCalledTimes(0);

    page.foreground();
    expect(hide).toHaveBeenCalledTimes(1);
    expect(wake).toHaveBeenCalledTimes(1);
  });

  it('pairs rather than throttles, so a fast background-return-background is three signals', () => {
    // A time window would swallow the second background, and for the
    // save-on-hide consumer that is exactly the write it exists to protect.
    const wake = vi.fn();
    const hide = vi.fn();
    onPageWake(wake);
    onPageHide(hide);

    page.background();
    page.foreground();
    page.background();

    expect(hide).toHaveBeenCalledTimes(2);
    expect(wake).toHaveBeenCalledTimes(1);
  });

  it('does not call a plain window focus a wake', () => {
    // Clicking back into a window that never went away. A consumer that
    // repositions the reader would otherwise move them on every window focus.
    const wake = vi.fn();
    onPageWake(wake);

    page.windowFocus();
    page.windowFocus();

    expect(wake).not.toHaveBeenCalled();
  });

  it('does not call the pageshow of a fresh load a wake', () => {
    // `pageshow` fires on every page load, not only on a bfcache restore. A
    // fresh load is positioned by the attach, not by a wake.
    const wake = vi.fn();
    onPageWake(wake);

    page.pageshow();

    expect(wake).not.toHaveBeenCalled();
  });

  it('gives a subscriber that arrives while already hidden its wake', () => {
    // The away bit is seeded from the real state on install, so subscribing from
    // a background tab is not a permanently missed transition.
    page.doc.visibilityState = 'hidden';
    const wake = vi.fn();
    onPageWake(wake);

    page.foreground();

    expect(wake).toHaveBeenCalledTimes(1);
  });

  it('keeps the two sides independent', () => {
    const wake = vi.fn();
    onPageWake(wake);

    page.background();
    page.foreground();

    expect(wake).toHaveBeenCalledTimes(1); // a hide subscriber is not required
  });

  it('detaches its listeners once the last subscriber leaves', () => {
    // The module is a singleton over real document / window listeners, so a
    // leaked one outlives whatever mounted it.
    const offWake = onPageWake(vi.fn());
    const offHide = onPageHide(vi.fn());
    expect(page.listenerCount()).toBeGreaterThan(0);

    offWake();
    expect(page.listenerCount()).toBeGreaterThan(0); // the other side still holds them

    offHide();
    expect(page.listenerCount()).toBe(0);
  });

  it('removes its listeners from the page it BOUND to, not whatever is current', () => {
    // The module is a singleton over real listeners, and swapping the page is
    // the only way to drive these transitions at all, so the two targets do come
    // apart. Removing from the current global would leave the first page still
    // wired and this module still listening to something nobody can see.
    const off = onPageWake(vi.fn());
    expect(page.listenerCount()).toBeGreaterThan(0);

    const second = installFakePage(); // the globals now point somewhere else
    try {
      off();
      expect(page.listenerCount()).toBe(0);
      expect(second.listenerCount()).toBe(0);
    } finally {
      second.restore();
    }
  });

  it('stops calling an unsubscribed listener', () => {
    const wake = vi.fn();
    const off = onPageWake(wake);
    onPageHide(vi.fn()); // keep the module installed

    off();
    page.background();
    page.foreground();

    expect(wake).not.toHaveBeenCalled();
  });

  it('survives a listener that unsubscribes itself mid-fire', () => {
    const second = vi.fn();
    let off: (() => void) | null = null;
    off = onPageHide(() => off?.());
    onPageHide(second);

    expect(() => page.background()).not.toThrow();
    expect(second).toHaveBeenCalledTimes(1);
  });
});
