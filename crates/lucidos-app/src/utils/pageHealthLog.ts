/** The page health log: one `[Client/page-health]` line every 30 s from a
 *  touch device, so a slow decay over a long phone session shows as a trend.
 *
 *  It chases a report that text editing on the iOS PWA degrades over a session
 *  and recovers on relaunch: the edit menu stops opening and caret moves lag.
 *  The emulator does not reproduce it. A temporary measure: see
 *  `docs/temporary-measures.md` § Page health log.
 *
 *  Two halves. `installHealthCounters` patches the globals at boot, before any
 *  other module adds a listener, and counts what is live. The reporter in
 *  `pageHealthLog.report.ts` samples those counts with the main-thread lag,
 *  frame rate and DOM size. It starts with the shell, so it stays out of the
 *  entry chunk. */

import { hasCoarsePointer } from './viewport';

export interface HealthCounters {
  /** Live listeners on `document`, `window` and `visualViewport`, by type. */
  listeners: Map<string, number>;
  intervals: number;
  observers: number;
  /** Every `requestAnimationFrame` call since install. */
  rafCalls: number;
}

interface Globals {
  eventTarget: { prototype: EventTarget };
  watched: (target: EventTarget) => boolean;
  host: {
    setInterval: typeof setInterval;
    clearInterval: typeof clearInterval;
    requestAnimationFrame: typeof requestAnimationFrame;
  };
  observerPrototypes: { observe: (...a: never[]) => void; disconnect: () => void }[];
}

/** The slice of `FinalizationRegistry` used here. The project's TS lib
 *  predates it. */
type FinalizationRegistryCtor = new (onCollect: () => void) => {
  register(target: object, held: undefined, token: object): void;
  unregister(token: object): void;
};

function captureOf(opts: unknown): boolean {
  return typeof opts === 'boolean' ? opts : !!(opts as { capture?: boolean } | undefined)?.capture;
}

/** A listener the browser removes by itself is never counted: `once`, or one
 *  bound to an `AbortSignal`. Neither passes through `removeEventListener`. */
function removesItself(opts: unknown): boolean {
  if (typeof opts !== 'object' || opts === null) return false;
  const o = opts as { once?: boolean; signal?: AbortSignal };
  return !!o.once || !!o.signal;
}

/** Patch the globals to count what is live. Returns a restore function. */
export function installHealthCounters(g: Globals): { counters: HealthCounters; restore: () => void } {
  const counters: HealthCounters = { listeners: new Map(), intervals: 0, observers: 0, rafCalls: 0 };
  const bump = (type: string, d: number) => counters.listeners.set(type, (counters.listeners.get(type) ?? 0) + d);

  const proto = g.eventTarget.prototype;
  const add = proto.addEventListener;
  const remove = proto.removeEventListener;
  const registered = new WeakMap<object, Set<string>>();
  // Keyed per target as well as type and phase, as the browser dedupes.
  const targetIds = new WeakMap<EventTarget, number>();
  let nextTargetId = 0;
  const keyOf = (target: EventTarget, type: string, opts: unknown) => {
    if (!targetIds.has(target)) targetIds.set(target, nextTargetId++);
    return `${targetIds.get(target)}|${type}|${captureOf(opts)}`;
  };
  proto.addEventListener = function (this: EventTarget, type: string, fn: EventListenerOrEventListenerObject | null, opts?: boolean | AddEventListenerOptions) {
    if (fn && g.watched(this) && !removesItself(opts)) {
      const key = keyOf(this, type, opts);
      let keys = registered.get(fn);
      if (!keys) { keys = new Set(); registered.set(fn, keys); }
      if (!keys.has(key)) { keys.add(key); bump(type, 1); }
    }
    return add.call(this, type, fn, opts);
  };
  proto.removeEventListener = function (this: EventTarget, type: string, fn: EventListenerOrEventListenerObject | null, opts?: boolean | EventListenerOptions) {
    if (fn && g.watched(this)) {
      const keys = registered.get(fn);
      if (keys?.delete(keyOf(this, type, opts))) bump(type, -1);
    }
    return remove.call(this, type, fn, opts);
  };

  const live = new Set<unknown>();
  const { setInterval: si, clearInterval: ci, requestAnimationFrame: raf } = g.host;
  g.host.setInterval = ((...a: Parameters<typeof setInterval>) => {
    const id = si(...a);
    live.add(id);
    counters.intervals = live.size;
    return id;
  }) as typeof setInterval;
  g.host.clearInterval = ((id?: Parameters<typeof clearInterval>[0]) => {
    live.delete(id);
    counters.intervals = live.size;
    return ci(id);
  }) as typeof clearInterval;
  g.host.requestAnimationFrame = (cb: FrameRequestCallback) => {
    counters.rafCalls += 1;
    return raf(cb);
  };

  const observing = new WeakSet<object>();
  // An observer collected without a disconnect still leaves the count.
  const Registry = (globalThis as { FinalizationRegistry?: FinalizationRegistryCtor }).FinalizationRegistry;
  const collected = Registry ? new Registry(() => { counters.observers -= 1; }) : null;
  const restores = g.observerPrototypes.map((p) => {
    const { observe, disconnect } = p;
    p.observe = function (this: object, ...a: never[]) {
      if (!observing.has(this)) {
        observing.add(this);
        counters.observers += 1;
        collected?.register(this, undefined, this);
      }
      return observe.apply(this, a);
    };
    p.disconnect = function (this: object) {
      if (observing.delete(this)) {
        counters.observers -= 1;
        collected?.unregister(this);
      }
      return disconnect.apply(this);
    };
    return () => { p.observe = observe; p.disconnect = disconnect; };
  });

  return {
    counters,
    restore: () => {
      proto.addEventListener = add;
      proto.removeEventListener = remove;
      g.host.setInterval = si;
      g.host.clearInterval = ci;
      g.host.requestAnimationFrame = raf;
      for (const r of restores) r();
    },
  };
}

let installed: HealthCounters | null = null;

/** The live counts, or null where nothing was installed (a desktop). */
export function pageHealthCounters(): HealthCounters | null {
  return installed;
}

/** Install on a touch device only. Idempotent. */
export function installPageHealthCounters(): void {
  if (installed || typeof window === 'undefined' || !hasCoarsePointer()) return;
  installed = installHealthCounters({
    eventTarget: EventTarget,
    watched: (t) => t === document || t === window || t === window.visualViewport,
    host: window,
    observerPrototypes: [
      typeof MutationObserver === 'function' ? MutationObserver.prototype : null,
      typeof ResizeObserver === 'function' ? ResizeObserver.prototype : null,
      typeof IntersectionObserver === 'function' ? IntersectionObserver.prototype : null,
    ].filter((p) => p !== null),
  }).counters;
}
