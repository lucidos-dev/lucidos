import { describe, it, expect, afterEach, vi } from 'vitest';
import { afterPressSettles, primaryPointerIsDown } from './pointerPress';

/** Drives the document stub from `test-setup.ts`, which dispatches to every
 *  listener of a type regardless of phase. That is enough here: the module
 *  installs one listener per type. */
function dispatch(type: string, fields: Record<string, unknown> = {}): void {
  const e = { type, isTrusted: true, isPrimary: true, button: 0, ...fields };
  (document as unknown as { dispatchEvent: (e: unknown) => boolean }).dispatchEvent(e);
}

describe('primaryPointerIsDown', () => {
  afterEach(() => dispatch('pointerup'));

  it('is false before anything is pressed', () => {
    expect(primaryPointerIsDown()).toBe(false);
  });

  it('follows a trusted primary press down and up', () => {
    dispatch('pointerdown');
    expect(primaryPointerIsDown()).toBe(true);
    dispatch('pointerup');
    expect(primaryPointerIsDown()).toBe(false);
  });

  it('is cleared by a cancel, which dispatches no click either', () => {
    dispatch('pointerdown');
    dispatch('pointercancel');
    expect(primaryPointerIsDown()).toBe(false);
  });

  /** The whole reason the trust check exists. A dispatched PointerEvent gets no
   *  paired click, so an overlay opened by one must keep dismissing on the next
   *  synthetic click. `e2e/overlay-dismiss-swallow.spec.ts` drives exactly
   *  that. */
  it('ignores an untrusted press', () => {
    dispatch('pointerdown', { isTrusted: false });
    expect(primaryPointerIsDown()).toBe(false);
  });

  it('ignores a secondary button, which pairs with no click', () => {
    dispatch('pointerdown', { button: 2 });
    expect(primaryPointerIsDown()).toBe(false);
  });

  it('is not cleared by a second finger lifting', () => {
    dispatch('pointerdown');
    dispatch('pointerup', { isPrimary: false });
    expect(primaryPointerIsDown()).toBe(true);
  });
});

/** A relayout on focus loss waits for the press that moved the focus, so the
 *  release is hit-tested against the layout the press landed on. */
describe('afterPressSettles', () => {
  afterEach(() => {
    dispatch('pointerup');
    vi.useRealTimers();
  });

  it('runs in the next task when no press is held, after a tap has clicked', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    afterPressSettles(fn);
    expect(fn).not.toHaveBeenCalled();
    vi.runAllTimers();
    expect(fn).toHaveBeenCalledOnce();
  });

  it('waits for a held press to lift, then runs in the task after its click', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    dispatch('pointerdown');
    afterPressSettles(fn);
    vi.runAllTimers();
    expect(fn).not.toHaveBeenCalled();
    dispatch('pointerup');
    expect(fn).not.toHaveBeenCalled();
    vi.runAllTimers();
    expect(fn).toHaveBeenCalledOnce();
  });

  it('runs once a cancelled press ends, and only once', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    dispatch('pointerdown');
    afterPressSettles(fn);
    dispatch('pointercancel');
    dispatch('pointerup');
    vi.runAllTimers();
    expect(fn).toHaveBeenCalledOnce();
  });
});
