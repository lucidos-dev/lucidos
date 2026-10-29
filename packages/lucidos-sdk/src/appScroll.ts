/**
 * Reports the app's scroll to the host, so the mobile header can glide away
 * over an app (*dynamic bars*, ADR 0469).
 *
 * The frame's own document scrolls, so the host never sees it. This module
 * only watches: every listener is passive, nothing calls `preventDefault()`,
 * and nothing writes a scroll position. A version that consumed touches to
 * move the header first made the app double-scroll.
 *
 * Only the reader's scroll counts. An element's first scroll event sets its
 * baseline, so the scroll-memory restore on load moves nothing. A scroll right
 * after a viewport resize is the browser clamping, not the reader. The host
 * resizes this frame as the bars move, so that clamp must not feed back.
 *
 * It also reports whether a finger is on the app. The host moves the bars only
 * once it lifts. iOS tracks a drag in the frame's coordinates, so a frame
 * moving under a resting finger reads as a scroll back (ADR 0482).
 */

import { postToHost } from './_bridge';

/** Posted with the reader's scroll since the last report. */
export const APP_SCROLL_MESSAGE_TYPE = 'lucidos:app:scroll';

/** Posted with `touching` as a finger lands on the app or the last one lifts. */
export const APP_TOUCH_MESSAGE_TYPE = 'lucidos:app:touch';

/** One report: `delta` is the scroll since the last one, down positive.
 *  `top` and `max` are where the scrolled element now rests. */
export interface AppScrollReport {
  delta: number;
  top: number;
  max: number;
}

/** The part of a scrolling element the tracker reads. */
export interface ScrollBox {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** The box's furthest scroll position. */
function maxScroll(box: ScrollBox): number {
  return Math.max(0, box.scrollHeight - box.clientHeight);
}

/** The box's offset, with elastic bounce past either end clamped away. */
export function clampedTop(box: ScrollBox): number {
  return Math.min(Math.max(0, box.scrollTop), maxScroll(box));
}

/** Whether a move from `prev` to `now` is the browser clamping the scroll:
 *  up, onto the bottom, because the bottom itself came up. A clamp can land
 *  frames after the resize that caused it, so its shape is what tells it.
 *  "Onto" allows under a pixel: a fractional offset stops short of `max`. */
function isClamp(prev: { top: number; max: number }, now: { top: number; max: number }): boolean {
  return now.top < prev.top && now.max < prev.max && now.max - now.top < 1;
}

/** Measures each scroll event against the same element's last position. */
export function createScrollTracker() {
  const last = new WeakMap<ScrollBox, { top: number; max: number }>();
  return {
    /** The report for one scroll event on `box`. `relayout` says the event
     *  followed a viewport resize, so it re-bases without a delta. */
    scrolled(box: ScrollBox, relayout: boolean): AppScrollReport {
      const now = { top: clampedTop(box), max: maxScroll(box) };
      const prev = last.get(box);
      last.set(box, now);
      const delta = prev === undefined || relayout || isClamp(prev, now) ? 0 : now.top - prev.top;
      return { delta, ...now };
    },
  };
}

/** Fold a later report into a pending one: the deltas add up, and the later
 *  position wins. */
export function mergeReports(pending: AppScrollReport | null, next: AppScrollReport): AppScrollReport {
  return pending ? { ...next, delta: pending.delta + next.delta } : next;
}

/** The element a scroll event scrolled: the root scroller for the document. */
function scrolledBox(target: EventTarget | null): ScrollBox | null {
  const doc = target as Partial<Document> | null;
  if (doc?.documentElement) return doc.scrollingElement ?? doc.documentElement;
  return typeof (target as Partial<ScrollBox> | null)?.scrollTop === 'number' ? target as unknown as ScrollBox : null;
}

/**
 * Install the report in an app frame. Call once on SDK load. Posts at most one
 * scroll report per animation frame, and none whose delta is 0. Posts the
 * finger only when it lands or lifts.
 *
 * No-op when there is no parent window: the SDK loaded at the top level has no
 * header to move.
 */
export function installAppScrollReport(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const tracker = createScrollTracker();
  let pending: AppScrollReport | null = null;
  let frame: number | null = null;
  // The rendering steps run resize, then scroll, then animation frame
  // callbacks. So a scroll between a resize and the next callback is the clamp.
  let resized = false;

  const flush = () => {
    frame = null;
    resized = false;
    if (pending && pending.delta !== 0) {
      postToHost({ type: APP_SCROLL_MESSAGE_TYPE, ...pending });
    }
    pending = null;
  };
  const schedule = () => {
    if (frame === null) frame = requestAnimationFrame(flush);
  };
  const onResize = () => {
    resized = true;
    schedule();
  };
  const onScroll = (e: Event) => {
    const box = scrolledBox(e.target);
    if (!box) return;
    // A sideways scroll moves no `top`. Folded in, its position would replace
    // the vertical scroller's, and the host would read the top of the page.
    const report = tracker.scrolled(box, resized);
    if (report.delta === 0) return;
    pending = mergeReports(pending, report);
    schedule();
  };

  let touching = false;
  const postTouch = (next: boolean) => {
    if (next === touching) return;
    touching = next;
    postToHost({ type: APP_TOUCH_MESSAGE_TYPE, touching });
  };
  // Pointer events cannot serve: iOS cancels them once a native scroll starts.
  // Capture, so an app that stops a touch's propagation cannot hide the lift.
  const onTouch = (e: Event) => postTouch((e as TouchEvent).touches.length > 0);
  const onPageHide = () => postTouch(false);

  const scrollOptions: AddEventListenerOptions = { capture: true, passive: true };
  const passive: AddEventListenerOptions = { passive: true };
  const touchTypes = ['touchstart', 'touchend', 'touchcancel'] as const;
  window.addEventListener('resize', onResize, passive);
  document.addEventListener('scroll', onScroll, scrollOptions);
  for (const type of touchTypes) window.addEventListener(type, onTouch, scrollOptions);
  window.addEventListener('pagehide', onPageHide, passive);
  return () => {
    window.removeEventListener('resize', onResize);
    document.removeEventListener('scroll', onScroll, scrollOptions);
    for (const type of touchTypes) window.removeEventListener(type, onTouch, scrollOptions);
    window.removeEventListener('pagehide', onPageHide);
    if (frame !== null) cancelAnimationFrame(frame);
  };
}
