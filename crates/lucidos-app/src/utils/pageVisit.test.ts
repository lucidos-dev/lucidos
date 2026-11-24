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
    const wake = vi.fn();
    onPageWake(wake);

    page.windowFocus();
    page.windowFocus();

    expect(wake).not.toHaveBeenCalled();
  });

  it('does not call the pageshow of a fresh load a wake', () => {
    const wake = vi.fn();
    onPageWake(wake);

    page.pageshow();

    expect(wake).not.toHaveBeenCalled();
  });

  it('gives a subscriber that arrives while already hidden its wake', () => {
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

    expect(wake).toHaveBeenCalledTimes(1);
  });

  it('detaches its listeners once the last subscriber leaves', () => {
    const offWake = onPageWake(vi.fn());
    const offHide = onPageHide(vi.fn());
    expect(page.listenerCount()).toBeGreaterThan(0);

    offWake();
    expect(page.listenerCount()).toBeGreaterThan(0);

    offHide();
    expect(page.listenerCount()).toBe(0);
  });

  it('removes its listeners from the page it BOUND to, not whatever is current', () => {
    const off = onPageWake(vi.fn());
    expect(page.listenerCount()).toBeGreaterThan(0);

    const second = installFakePage();
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
    onPageHide(vi.fn());

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
