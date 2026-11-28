
import { postToHost } from './_bridge';

export const APP_SCROLL_MESSAGE_TYPE = 'lucidos:app:scroll';

export const APP_TOUCH_MESSAGE_TYPE = 'lucidos:app:touch';

export interface AppScrollReport {
  delta: number;
  top: number;
  max: number;
}

export interface ScrollBox {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

function maxScroll(box: ScrollBox): number {
  return Math.max(0, box.scrollHeight - box.clientHeight);
}

export function clampedTop(box: ScrollBox): number {
  return Math.min(Math.max(0, box.scrollTop), maxScroll(box));
}

function isClamp(prev: { top: number; max: number }, now: { top: number; max: number }): boolean {
  return now.top < prev.top && now.max < prev.max && now.max - now.top < 1;
}

export function createScrollTracker() {
  const last = new WeakMap<ScrollBox, { top: number; max: number }>();
  return {
    scrolled(box: ScrollBox, relayout: boolean): AppScrollReport {
      const now = { top: clampedTop(box), max: maxScroll(box) };
      const prev = last.get(box);
      last.set(box, now);
      const delta = prev === undefined || relayout || isClamp(prev, now) ? 0 : now.top - prev.top;
      return { delta, ...now };
    },
  };
}

export function mergeReports(pending: AppScrollReport | null, next: AppScrollReport): AppScrollReport {
  return pending ? { ...next, delta: pending.delta + next.delta } : next;
}

function scrolledBox(target: EventTarget | null): ScrollBox | null {
  const doc = target as Partial<Document> | null;
  if (doc?.documentElement) return doc.scrollingElement ?? doc.documentElement;
  return typeof (target as Partial<ScrollBox> | null)?.scrollTop === 'number' ? target as unknown as ScrollBox : null;
}

export function installAppScrollReport(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const tracker = createScrollTracker();
  let pending: AppScrollReport | null = null;
  let frame: number | null = null;
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
