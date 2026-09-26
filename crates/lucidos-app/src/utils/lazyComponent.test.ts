import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentType } from 'preact';

const reloadForStaleChunk = vi.fn(() => true);
vi.mock('../hooks/sw-update', () => ({ reloadForStaleChunk: () => reloadForStaleChunk() }));

import { lazyComponent, whenLoaded } from './lazyComponent';

/**
 * `preload()` is how a chunk loads before anything mounts it: the shell chunk
 * at boot, and the idle prefetch of popover bodies (ADR 0288). It must share
 * the one load a mount would start, and never reject into a caller that fired
 * it and moved on.
 */
describe('lazyComponent.preload', () => {
  beforeEach(() => reloadForStaleChunk.mockClear());

  it('resolves true once the component is in memory', async () => {
    const Lazy = lazyComponent(() => Promise.resolve(() => null));
    await expect(Lazy.preload()).resolves.toBe(true);
    await expect(Lazy.preload()).resolves.toBe(true);
  });

  it('loads once, however often it is asked', async () => {
    const Body = () => null;
    const loader = vi.fn(() => Promise.resolve(Body));
    const Lazy = lazyComponent(loader);
    await Promise.all([Lazy.preload(), Lazy.preload()]);
    await Lazy.preload();
    expect(loader).toHaveBeenCalledTimes(1);
  });

  it('renders the loaded component once preloaded, with no null frame', async () => {
    const Body = () => null;
    const Lazy = lazyComponent(() => Promise.resolve(Body));
    await Lazy.preload();
    const vnode = (Lazy as (props: object) => { type: unknown } | null)({});
    expect(vnode?.type).toBe(Body);
  });

  it('resolves on a failed load and takes the stale-chunk reload', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const Lazy = lazyComponent(() => Promise.reject(new Error('404')));
    await expect(Lazy.preload()).resolves.toBe(false);
    expect(reloadForStaleChunk).toHaveBeenCalledTimes(1);
    error.mockRestore();
  });

  it('retries after a failure, since the next ask may find the chunk', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const Body = () => null;
    const loader = vi.fn()
      .mockReturnValueOnce(Promise.reject(new Error('404')))
      .mockReturnValueOnce(Promise.resolve(Body));
    const Lazy = lazyComponent(loader);
    await Lazy.preload();
    await Lazy.preload();
    expect(loader).toHaveBeenCalledTimes(2);
    error.mockRestore();
  });
});

describe('whenLoaded', () => {
  const settle = () => new Promise((r) => setTimeout(r, 0));
  const pointerdown = (target: EventTarget) =>
    ({ type: 'pointerdown', target }) as unknown as PointerEvent;

  /** A document stand-in that records the one listener `whenLoaded` adds. */
  function fakeDocument() {
    let listener: ((e: PointerEvent) => void) | null = null;
    return {
      doc: {
        addEventListener: (_t: string, fn: (e: PointerEvent) => void) => { listener = fn; },
        removeEventListener: () => { listener = null; },
      } as unknown as Document,
      press: (target: EventTarget) => listener?.(pointerdown(target)),
    };
  }

  it('runs the open once the chunk is in memory', async () => {
    const { doc } = fakeDocument();
    const open = vi.fn();
    whenLoaded(lazyComponent(() => Promise.resolve(() => null)), open, {} as Element, doc);
    await settle();
    expect(open).toHaveBeenCalledTimes(1);
  });

  it('never opens once cancelled, so a second press can take the open back', async () => {
    const { doc } = fakeDocument();
    const open = vi.fn();
    const handle = whenLoaded(lazyComponent(() => Promise.resolve(() => null)), open, {} as Element, doc);
    handle.cancel();
    await settle();
    expect(open).not.toHaveBeenCalled();
  });

  it('is cancelled by a press anywhere but its own trigger', async () => {
    const { doc, press } = fakeDocument();
    const trigger = { contains: (n: unknown) => n === trigger } as unknown as Element;
    const open = vi.fn();
    const handle = whenLoaded(lazyComponent(() => Promise.resolve(() => null)), open, trigger, doc);
    press(trigger);
    expect(handle.pending, 'a press on the trigger is left to its own handler').toBe(true);
    press({} as EventTarget);
    await settle();
    expect(open).not.toHaveBeenCalled();
  });

  it('never opens a surface whose chunk failed to load', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { doc } = fakeDocument();
    const open = vi.fn();
    const handle = whenLoaded(lazyComponent(() => Promise.reject(new Error('404'))), open, {} as Element, doc);
    await settle();
    expect(open).not.toHaveBeenCalled();
    expect(handle.pending).toBe(false);
    error.mockRestore();
  });

  it('reports it is no longer pending, however it ended', async () => {
    // The caller keeps the handle, and an outside press may cancel it. The next
    // press on the trigger must then read "not pending" and open. It must not
    // spend itself cancelling an open that is already gone.
    const { doc, press } = fakeDocument();
    const trigger = { contains: () => false } as unknown as Element;
    const never = new Promise<ComponentType>(() => {});
    const cancelled = whenLoaded(lazyComponent(() => never), vi.fn(), trigger, doc);
    press({} as EventTarget);
    expect(cancelled.pending).toBe(false);
    const opened = whenLoaded(lazyComponent(() => Promise.resolve(() => null)), vi.fn(), trigger, doc);
    await settle();
    expect(opened.pending).toBe(false);
  });
});
