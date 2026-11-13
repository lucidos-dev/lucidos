import { afterEach, describe, expect, it } from 'vitest';
import { installHealthCounters } from './pageHealthLog';
import { topListeners } from './pageHealthLog.report';

describe('page health counters', () => {
  let restore: (() => void) | null = null;
  afterEach(() => { restore?.(); restore = null; });

  function install() {
    const watched = new EventTarget();
    const host = {
      setInterval: ((() => 1) as unknown) as typeof setInterval,
      clearInterval: ((() => {}) as unknown) as typeof clearInterval,
      requestAnimationFrame: (() => 0) as typeof requestAnimationFrame,
    };
    const observerProto = { observe() {}, disconnect() {} };
    const installed = installHealthCounters({
      eventTarget: EventTarget,
      watched: (t) => t === watched,
      host,
      observerPrototypes: [observerProto],
    });
    restore = installed.restore;
    return { ...installed, watched, host, observerProto };
  }

  it('counts a live listener once and forgets it on removal', () => {
    const { counters, watched } = install();
    const fn = () => {};
    watched.addEventListener('touchend', fn, { capture: true });
    watched.addEventListener('touchend', fn, true);
    expect(counters.listeners.get('touchend')).toBe(1);
    watched.removeEventListener('touchend', fn, false);
    expect(counters.listeners.get('touchend')).toBe(1);
    watched.removeEventListener('touchend', fn, true);
    expect(counters.listeners.get('touchend')).toBe(0);
  });

  it('skips listeners the browser removes by itself, and unwatched targets', () => {
    const { counters, watched } = install();
    watched.addEventListener('click', () => {}, { once: true });
    watched.addEventListener('click', () => {}, { signal: new AbortController().signal });
    new EventTarget().addEventListener('click', () => {});
    expect(counters.listeners.get('click')).toBeUndefined();
  });

  it('counts live intervals, observers and frame requests', () => {
    const { counters, host, observerProto } = install();
    const id = host.setInterval(() => {}, 10);
    expect(counters.intervals).toBe(1);
    host.clearInterval(id);
    expect(counters.intervals).toBe(0);
    host.requestAnimationFrame(() => {});
    expect(counters.rafCalls).toBe(1);
    const observer = Object.create(observerProto) as typeof observerProto;
    observer.observe();
    observer.observe();
    expect(counters.observers).toBe(1);
    observer.disconnect();
    expect(counters.observers).toBe(0);
  });

  it('lists the largest listener counts first', () => {
    const counts = new Map([['a', 1], ['b', 5], ['c', 0], ['d', 3]]);
    expect(topListeners(counts, 2)).toEqual({ b: 5, d: 3 });
  });
});
