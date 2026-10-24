import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { watchOnScreen } from './useOnScreenInTranscript';
import { getActiveScrollElement, setActiveScrollElement } from '../components/chat/scrollState';

/** What each constructed fake observer recorded. */
interface Armed {
  callback: () => void;
  root: Element | null;
  observed: unknown[];
  disconnects: number;
}

let armed: Armed[] = [];

/** Stand in for the browser observer, which this suite has no jsdom to supply.
 *  Faking it is also what makes the root and the teardown assertable. */
function installObserverFake(): void {
  (globalThis as Record<string, unknown>).IntersectionObserver = class {
    private entry: Armed;
    constructor(cb: () => void, opts?: { root?: Element | null }) {
      this.entry = { callback: cb, root: opts?.root ?? null, observed: [], disconnects: 0 };
      armed.push(this.entry);
    }
    observe(el: unknown) { this.entry.observed.push(el); }
    disconnect() { this.entry.disconnects++; }
  };
}

/** A node measurable at `top`, with no parent to clip it. `unmeasurableBox`
 *  walks `parentElement`, so leaving it undefined ends the walk at once. */
function node(top: number, height = 20): HTMLElement {
  return {
    getBoundingClientRect: () => ({
      top, bottom: top + height, left: 0, right: 200, width: 200, height,
    }),
  } as unknown as HTMLElement;
}

/** A transcript band from 0 to 600, inside a window that runs to 800. */
function scroller(): HTMLElement {
  return node(0, 600);
}

describe('watchOnScreen', () => {
  beforeEach(() => {
    armed = [];
    installObserverFake();
    // No layout engine, so the window band is supplied rather than measured.
    (globalThis as Record<string, unknown>).innerHeight = 800;
    (globalThis as Record<string, unknown>).innerWidth = 1024;
    setActiveScrollElement(null);
  });

  afterEach(() => {
    delete (globalThis as Record<string, unknown>).IntersectionObserver;
    setActiveScrollElement(null);
  });

  it('reports the verdict once up front, before anything crosses', () => {
    // A row that never crosses the root gets no further entry, so the caller
    // would sit on the optimistic default without this first sample.
    const reported: boolean[] = [];
    watchOnScreen(node(2000), (v: boolean) => reported.push(v));
    expect(reported).toEqual([false]);
  });

  it('reports true for a row inside the band', () => {
    const reported: boolean[] = [];
    watchOnScreen(node(100), (v: boolean) => reported.push(v));
    expect(reported).toEqual([true]);
  });

  it('re-samples whenever the observer fires', () => {
    const reported: boolean[] = [];
    let top = 100;
    const el = {
      getBoundingClientRect: () => ({ top, bottom: top + 20, left: 0, right: 200, width: 200, height: 20 }),
    } as unknown as HTMLElement;
    watchOnScreen(el, (v: boolean) => reported.push(v));
    expect(reported).toEqual([true]);
    top = 2000;
    armed[0].callback();
    expect(reported).toEqual([true, false]);
  });

  it('observes the element and disconnects on teardown', () => {
    const el = node(100);
    const stop = watchOnScreen(el, () => {});
    expect(armed).toHaveLength(1);
    expect(armed[0].observed).toEqual([el]);
    expect(armed[0].disconnects).toBe(0);
    stop();
    expect(armed[0].disconnects).toBe(1);
  });

  // The notifier and the verdict must measure the same box. `isElementOnScreen`
  // takes its band from the active scroll element, so the observer roots there
  // too. Root them differently and a crossing of the verdict's band fires no
  // callback, which latches the answer at whatever it last was.
  it('roots at the active scroll element, the box the verdict measures', () => {
    const el = node(100);
    setActiveScrollElement(scroller());
    watchOnScreen(el, () => {});
    expect(armed[0].root).toBe(getActiveScrollElement());
  });

  it('roots at the window when no transcript has registered', () => {
    watchOnScreen(node(100), () => {});
    expect(armed[0].root).toBeNull();
  });

  it('reports off screen for a row below the transcript but inside the window', () => {
    // The crossing a window-rooted observer would miss, and the reason the root
    // is the scroller. The window band here runs to 800.
    const reported: boolean[] = [];
    setActiveScrollElement(scroller());
    watchOnScreen(node(700), (v: boolean) => reported.push(v));
    expect(reported).toEqual([false]);
  });

  it('still reports once where the browser has no observer at all', () => {
    delete (globalThis as Record<string, unknown>).IntersectionObserver;
    const reported: boolean[] = [];
    const stop = watchOnScreen(node(100), (v: boolean) => reported.push(v));
    expect(reported).toEqual([true]);
    expect(() => stop()).not.toThrow();
  });
});
