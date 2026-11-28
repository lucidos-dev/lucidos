import { describe, it, expect, afterEach, vi } from 'vitest';
import { afterPressSettles, notePressOwesClick, primaryPointerIsDown } from './pointerPress';

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

  it('runs after the click an already-lifted press still owes', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    notePressOwesClick();
    afterPressSettles(fn);
    dispatch('click');
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
