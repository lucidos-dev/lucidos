import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Stub HTMLElement before importing modules that reference it
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}

import {
  awayFromBottom,
  clearPendingEventScroll,
  makeScrollObservers,
  scrollToEventAndPulse,
} from '../scrollState';

/** A resize NEVER moves the reader toward the bottom. Whatever grew, however
 *  much of it arrived, and wherever the reader happens to be sitting, the
 *  transcript stays where it is and only the chevron is reconciled.
 *
 *  This file used to assert the opposite half of a rule that had two: a reader
 *  within 80px of the bottom was re-pinned by the resize ("following"), and a
 *  reader outside it was left alone. Both are now the same answer, because the
 *  reader owns the scroll position (see the header of `scrollState.ts`). The
 *  cases are kept and inverted rather than deleted: each one still describes a
 *  real growth the app produces, and each now pins down that it moves nobody. */

/** A container that clamps `scrollTop` the way a browser does, so a
 *  `scrollTop = scrollHeight` write lands on the real maximum rather than
 *  parking an out-of-range number that hides an off-bottom state. */
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
    // A markdown image decodes and adds 400px under someone sitting at the live
    // edge. They stay at 1500, and the chevron comes up to offer them the ride
    // down. This case used to re-pin to 1900.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 2400;
    onResize();

    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('holds still however large the growth', () => {
    // Size is not evidence in either direction. A whole screenful arriving at
    // once is still the app growing its own content.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 20000;
    onResize();

    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('leaves a reader parked in history where they are, and shows the chevron', () => {
    // The panel-expand case. It needed an explicit `preserveOnToggle()` mark to
    // get this answer; now it is the only answer there is.
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

    el.scrollHeight = 2000; // 1500 + 500 clientHeight == the new bottom
    onResize();

    expect(awayFromBottom.value).toBe(false);
  });

  it('holds still while a notification deep-link is resolving', () => {
    // A deep-link owns the scroll target for the whole resolve window. This was
    // load-bearing when a resize could pin; it is asserted still because the
    // deep-link's landing must survive the growth of the thread it landed in.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    const origMO = (globalThis as any).MutationObserver;
    const origCSS = (globalThis as any).CSS;
    const origBody = (globalThis.document as any).body;
    (globalThis as any).MutationObserver = class { observe() {} disconnect() {} };
    (globalThis as any).CSS = { escape: (s: string) => s };
    (globalThis.document as any).body = { tagName: 'BODY' };
    try {
      // No match in the fake DOM, so the claim is held while it waits.
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
