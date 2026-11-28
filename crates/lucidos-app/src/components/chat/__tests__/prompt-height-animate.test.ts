import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { animateTextareaHeightFrom } from '../promptResize';

describe('animateTextareaHeightFrom', () => {
  let rafCbs: FrameRequestCallback[];

  beforeEach(() => {
    rafCbs = [];
    vi.useFakeTimers();
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => rafCbs.push(cb));
    vi.stubGlobal('cancelAnimationFrame', (handle: number) => {
      if (handle >= 1 && handle <= rafCbs.length) rafCbs[handle - 1] = null as unknown as FrameRequestCallback;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function flushRaf(): void {
    for (let guard = 0; guard < 10 && rafCbs.length; guard++) {
      const cbs = rafCbs;
      rafCbs = [];
      cbs.forEach((cb) => cb && cb(0));
    }
  }

  interface MockTextarea {
    style: { height: string; transition: string };
    offsetHeight: number;
    addEventListener: (type: string, fn: (e: unknown) => void) => void;
    removeEventListener: (type: string, fn: (e: unknown) => void) => void;
    dispatchEvent: (ev: { type: string; target: unknown; propertyName: string }) => void;
  }

  function makeEl(height: string): MockTextarea {
    const listeners: Record<string, Array<(e: unknown) => void>> = {};
    return {
      style: { height, transition: '' },
      offsetHeight: 0,
      addEventListener: (type, fn) => { (listeners[type] ??= []).push(fn); },
      removeEventListener: (type, fn) => { listeners[type] = (listeners[type] ?? []).filter((f) => f !== fn); },
      dispatchEvent: (ev) => { for (const fn of listeners[ev.type] ?? []) fn(ev); },
    };
  }

  function animate(el: MockTextarea, from: string): void {
    animateTextareaHeightFrom(el as unknown as HTMLTextAreaElement, from);
  }

  function fireTransitionEnd(el: MockTextarea, property: string): void {
    el.dispatchEvent({ type: 'transitionend', target: el, propertyName: property });
  }

  function hide(): void {
    const doc = document as unknown as {
      visibilityState: string;
      dispatchEvent: (e: { type: string }) => boolean;
    };
    doc.visibilityState = 'hidden';
    doc.dispatchEvent({ type: 'visibilitychange' });
    doc.visibilityState = 'visible';
  }

  it('no-ops when the height is unchanged', () => {
    const el = makeEl('120px');
    animate(el, '120px');
    expect(el.style.transition).toBe('');
    expect(el.style.height).toBe('120px');
    flushRaf();
    expect(el.style.transition).toBe('');
  });

  it('no-ops when there is no previous height to animate from', () => {
    const el = makeEl('120px');
    animate(el, '');
    expect(el.style.transition).toBe('');
    expect(el.style.height).toBe('120px');
  });

  it('inverts to the previous height first, then transitions to the target', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    expect(el.style.transition).toBe('none');
    expect(el.style.height).toBe('80px');
    flushRaf();
    expect(el.style.transition).toBe('height 300ms ease');
    expect(el.style.height).toBe('200px');
  });

  it('clears the transition and lands on the target on transitionend', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    fireTransitionEnd(el, 'height');
    expect(el.style.transition).toBe('');
    expect(el.style.height).toBe('200px');
  });

  it('ignores a transitionend for a different property', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    fireTransitionEnd(el, 'transform');
    expect(el.style.transition).toBe('height 300ms ease');
  });

  it('a stall past the fuse does not cancel the ease before it starts', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    expect(el.style.height).toBe('80px');

    vi.advanceTimersByTime(400);
    flushRaf();

    expect(el.style.transition).toBe('height 300ms ease');
    expect(el.style.height).toBe('200px');
  });

  it('lands the box when the tab hides before the ease starts', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    expect(el.style.height).toBe('80px');

    hide();

    expect(el.style.height).toBe('200px');
    expect(el.style.transition).toBe('');
    flushRaf();
    expect(el.style.transition).toBe('');
  });

  it('a hidden tab does NOT disturb an ease that already started', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    expect(el.style.transition).toBe('height 300ms ease');

    hide();

    expect(el.style.transition).toBe('height 300ms ease');
  });

  it('falls back to the safety timeout if transitionend never fires', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    expect(el.style.transition).toBe('height 300ms ease');
    vi.advanceTimersByTime(400);
    expect(el.style.transition).toBe('');
    expect(el.style.height).toBe('200px');
  });

  it('a rapid second switch supersedes the first without a stale clobber', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    expect(el.style.height).toBe('200px');

    el.style.height = '120px';
    animate(el, '200px');
    flushRaf();
    expect(el.style.height).toBe('120px');

    vi.advanceTimersByTime(400);
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('');
  });

  it('cancels the first switch\'s pending frames when re-switched in the same frame', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    el.style.height = '120px';
    animate(el, '200px');
    flushRaf();
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('height 300ms ease');
    fireTransitionEnd(el, 'height');
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('');
  });
});
