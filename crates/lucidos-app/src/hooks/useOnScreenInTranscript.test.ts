import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { watchOnScreen } from './useOnScreenInTranscript';
import { getActiveScrollElement, setActiveScrollElement } from '../components/chat/scrollState';

interface Armed {
  callback: () => void;
  root: Element | null;
  observed: unknown[];
  disconnects: number;
}

let armed: Armed[] = [];

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

function node(top: number, height = 20): HTMLElement {
  return {
    getBoundingClientRect: () => ({
      top, bottom: top + height, left: 0, right: 200, width: 200, height,
    }),
  } as unknown as HTMLElement;
}

function scroller(): HTMLElement {
  return node(0, 600);
}

describe('watchOnScreen', () => {
  beforeEach(() => {
    armed = [];
    installObserverFake();
    (globalThis as Record<string, unknown>).innerHeight = 800;
    (globalThis as Record<string, unknown>).innerWidth = 1024;
    setActiveScrollElement(null);
  });

  afterEach(() => {
    delete (globalThis as Record<string, unknown>).IntersectionObserver;
    setActiveScrollElement(null);
  });

  it('reports the verdict once up front, before anything crosses', () => {
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
