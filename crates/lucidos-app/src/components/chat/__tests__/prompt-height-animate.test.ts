import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { animateTextareaHeightFrom } from '../promptResize';

/**
 * Unit-tests the draft→draft height FLIP. The compose view stays centered when
 * switching between two drafts, so the ThreadPane FLIP never fires — this helper
 * eases the textarea from the previous draft's height to the new one instead of
 * insta-resizing. Tests run in the node environment (no jsdom), so we drive a
 * mock textarea plus stubbed requestAnimationFrame + fake timers.
 */
describe('animateTextareaHeightFrom', () => {
  let rafCbs: FrameRequestCallback[];

  beforeEach(() => {
    rafCbs = [];
    // Fake timers first — useFakeTimers() also fakes requestAnimationFrame, so
    // stub rAF AFTER it to keep our own queue (we drive setTimeout via vitest).
    // Handles are 1-based indices into rafCbs; cancel nulls the slot.
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

  /** Run every queued rAF callback (the helper chains a second frame); skips
   *  slots cleared by cancelAnimationFrame. */
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
    const el = makeEl('200px'); // target — autoResize already applied it
    animate(el, '80px');
    // Inverted synchronously with no transition, so the box starts where the
    // previous draft left it.
    expect(el.style.transition).toBe('none');
    expect(el.style.height).toBe('80px');
    // Next frame: transition enabled and height driven to the target.
    flushRaf();
    expect(el.style.transition).toBe('height 0.3s ease');
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
    expect(el.style.transition).toBe('height 0.3s ease'); // still animating
  });

  it('falls back to the safety timeout if transitionend never fires', () => {
    const el = makeEl('200px');
    animate(el, '80px');
    flushRaf();
    expect(el.style.transition).toBe('height 0.3s ease');
    vi.advanceTimersByTime(400);
    expect(el.style.transition).toBe('');
    expect(el.style.height).toBe('200px');
  });

  it('a rapid second switch supersedes the first without a stale clobber', () => {
    const el = makeEl('200px');
    animate(el, '80px'); // switch 1: 80 → 200
    flushRaf();
    expect(el.style.height).toBe('200px');

    // Switch 2 begins mid-animation: the caller (autoResize) has set the new
    // target, and fromHeight is switch 1's target height.
    el.style.height = '120px';
    animate(el, '200px'); // switch 2: 200 → 120, cancels switch 1
    flushRaf();
    expect(el.style.height).toBe('120px');

    // Switch 1's stale timer/listener must NOT fire and snap back to 200.
    vi.advanceTimersByTime(400);
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('');
  });

  it('cancels the first switch\'s pending frames when re-switched in the same frame', () => {
    const el = makeEl('200px');
    animate(el, '80px'); // switch 1 — outer rAF queued, not yet fired
    // Switch 2 in the SAME frame (before any rAF flush): caller applied the new
    // target; switch 1's pending frames must be canceled so its finish listener
    // is never attached.
    el.style.height = '120px';
    animate(el, '200px'); // switch 2 — cancels switch 1's pending frames
    flushRaf();
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('height 0.3s ease');
    // Only switch 2 is live: its transitionend lands on switch 2's target, and
    // no stale switch-1 listener fires afterward.
    fireTransitionEnd(el, 'height');
    expect(el.style.height).toBe('120px');
    expect(el.style.transition).toBe('');
  });
});
