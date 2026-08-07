import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Stub HTMLElement before importing modules that reference it
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}

import {
  awayFromBottom,
  clearPendingEventScroll,
  makeScrollObservers,
  preserveOnToggle,
  scrollToEventAndPulse,
  scrolledUp,
} from '../scrollState';

/** What a resize means depends on whether the reader is parked, and NOTHING
 *  else. `scroll-raf-observers.test.ts` owns the bottom-pin loop (what happens
 *  INSIDE the suppression window); this file owns the observer's resize rule,
 *  i.e. what happens once that window has closed and the transcript grows on
 *  its own clock.
 *
 *  The regression: `onResize` used to read "content grew and we are no longer
 *  within 80px of the bottom" as "the reader scrolled up" and set `scrolledUp`.
 *  Nothing the reader did produced that conclusion, and every auto-scroll path
 *  defers to `scrolledUp`, so one late-decoding image left the transcript stuck
 *  above the newest turn for the rest of the visit. */

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

describe('onResize follows the bottom instead of inferring a scroll-up', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    awayFromBottom.value = false;
    clearPendingEventScroll();
  });
  afterEach(() => {
    clearPendingEventScroll();
  });

  it('keeps a following reader on the bottom when content grows after the pin window', () => {
    // The reported bug, at the layer it happens: the thread opened and pinned,
    // the 500ms suppression window closed, and only THEN did a markdown image
    // decode and add 400px to the transcript.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 2400;
    onResize();

    expect(el.scrollTop).toBe(1900); // the new bottom, 2400 - 500
    expect(scrolledUp.value).toBe(false);
    expect(awayFromBottom.value).toBe(false);
  });

  it('never sets scrolledUp from a resize, however large the growth', () => {
    // Size is not evidence. A whole screenful arriving at once is still the app
    // growing its own content, not the reader choosing to read history.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    el.scrollHeight = 20000;
    onResize();

    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(19500);
  });

  it('leaves a parked reader where they are, and shows the chevron', () => {
    // The panel-expand contract: expanding a step row while at the bottom marks
    // the reader parked (preserveOnToggle) precisely so the growth does NOT
    // re-pin them, since they expanded it to read it.
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2000 });
    const { onResize } = makeScrollObservers(el);

    preserveOnToggle();
    el.scrollHeight = 2400;
    onResize();

    expect(el.scrollTop).toBe(1500);
    expect(scrolledUp.value).toBe(true);
    expect(awayFromBottom.value).toBe(true);
  });

  it('clears the chevron when a shrink leaves a parked reader visually at the bottom', () => {
    const el = makeEl({ scrollTop: 1500, scrollHeight: 2500 });
    const { onResize } = makeScrollObservers(el);
    scrolledUp.value = true;
    awayFromBottom.value = true;

    el.scrollHeight = 2000; // 1500 + 500 clientHeight == the new bottom
    onResize();

    expect(awayFromBottom.value).toBe(false);
  });

  it('defers to an in-flight notification deep-link claim', () => {
    // A deep-link owns the scroll target for the whole resolve window. Following
    // the bottom here would slam the freshly-landed event out of view a beat
    // after it landed.
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
