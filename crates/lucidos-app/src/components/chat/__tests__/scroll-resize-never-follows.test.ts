import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}

import {
  awayFromBottom,
  clearPendingEventScroll,
  makeScrollObservers,
  scrollToEventAndPulse,
} from '../scrollState';


function makeEl(opts: { scrollTop: number; scrollHeight: number; clientHeight?: number }) {
  return {
    parentElement: null,
    children: [],
    clientWidth: 800,
    clientHeight: opts.clientHeight ?? 500,
    scrollHeight: opts.scrollHeight,
    _scrollTop: opts.scrollTop,
    get scrollTop() { return this._scrollTop; },
    set scrollTop(v: number) {
      this._scrollTop = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
    },
    getBoundingClientRect: () => ({ width: 400, height: 500, top: 0, bottom: 500, left: 0, right: 400 }),
  } as any;
}

describe('onResize never moves the reader toward the bottom', () => {
  beforeEach(() => {
    awayFromBottom.value = false;
    clearPendingEventScroll();
  });
  afterEach(() => {
    clearPendingEventScroll();
  });

  it('leaves a reader who is exactly at the bottom where they are when content grows', () => {
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 2400;
    onResize();

    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('holds still however large the growth', () => {
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 20000;
    onResize();

    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('leaves a reader parked in history where they are, and shows the chevron', () => {
    const el = makeEl({ scrollTop: 400, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 2400;
    onResize();

    expect(el.scrollTop).toBe(400);
    expect(awayFromBottom.value).toBe(true);
  });

  it('clears the chevron when a shrink leaves the reader visually at the bottom', () => {
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2500 });
    const { onResize } = makeScrollObservers(el);
    awayFromBottom.value = true;

    el.scrollHeight = 2000;
    onResize();

    expect(awayFromBottom.value).toBe(false);
  });

  it('holds still while a notification deep-link is resolving', () => {
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    const origMO = (globalThis as any).MutationObserver;
    const origCSS = (globalThis as any).CSS;
    const origBody = (globalThis.document as any).body;
    (globalThis as any).MutationObserver = class { observe() {} disconnect() {} };
    (globalThis as any).CSS = { escape: (s: string) => s };
    (globalThis.document as any).body = { tagName: 'BODY' };
    try {
      scrollToEventAndPulse('never-renders');
      el.scrollHeight = 2400;
      onResize();
      expect(el.scrollTop).toBe(1500);
    } finally {
      clearPendingEventScroll();
      (globalThis as any).MutationObserver = origMO;
      (globalThis as any).CSS = origCSS;
      (globalThis.document as any).body = origBody;
      vi.clearAllTimers();
    }
  });
});
