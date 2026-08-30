import { describe, it, expect, afterEach } from 'vitest';
import { primaryPointerIsDown } from './pointerPress';

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
