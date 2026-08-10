/**
 * `data-keyboard-active` on `<html>` is what tells the mobile stylesheet the
 * on-screen keyboard is up, so a stray tap can't land on the header, the pane
 * dots, or the transcript's content while the user types.
 *
 * It used to be maintained purely by focusin/focusout, which describe focus by
 * its TRANSITIONS. One transition is never reported: removing the focused
 * element from the DOM moves focus to `<body>` without firing focusout (WebKit
 * and Chromium both, verified on mobile WebKit). The flag then survives with
 * nothing focused at all, and every surface it gates stays inert until a
 * reload. That is how the transcript could sit frozen with a live question card
 * on screen while the horizontal pane swipe kept working: the swipe's own gate
 * reads `document.activeElement`, so it correctly saw no text input, while the
 * stylesheet was still acting on a flag nothing was left to clear.
 *
 * `reconcileKeyboardActive` derives the flag from the LIVE focus instead, so the
 * state cannot outlive the thing it describes. The effect runs it on the user's
 * next touch and on a return to the foreground.
 *
 * Both helpers are duck-typed (the frontend test environment has no DOM), so the
 * stand-ins below carry just `tagName` and `closest`.
 */
import { describe, it, expect } from 'vitest';
import { isKeyboardActiveTarget, reconcileKeyboardActive } from '../MobileSwipeContainer';

const ATTR = 'data-keyboard-active';

/** An element stand-in. `inTitleRow` decides what `closest` answers. */
function el(tagName: string, inTitleRow = false): EventTarget {
  return {
    tagName,
    closest: (sel: string) => (inTitleRow && sel === '.mobile-thread-title-row' ? {} : null),
  } as unknown as EventTarget;
}

/** `<html>` stand-in that records the attribute the way the DOM would. */
function root() {
  let on = false;
  return {
    setAttribute: (name: string) => { if (name === ATTR) on = true; },
    removeAttribute: (name: string) => { if (name === ATTR) on = false; },
    get active() { return on; },
    force(value: boolean) { on = value; },
  };
}

describe('isKeyboardActiveTarget', () => {
  it('is true for a plain textarea (the composer)', () => {
    expect(isKeyboardActiveTarget(el('TEXTAREA'))).toBe(true);
  });

  it('is false for the thread-title editor, which the flag would lock out', () => {
    expect(isKeyboardActiveTarget(el('TEXTAREA', true))).toBe(false);
  });

  it('is false for an <input>, so the header search bar stays interactive', () => {
    expect(isKeyboardActiveTarget(el('INPUT'))).toBe(false);
  });

  it('is false for a non-element target and for nothing at all', () => {
    expect(isKeyboardActiveTarget(null)).toBe(false);
    expect(isKeyboardActiveTarget({ tagName: 'TEXTAREA' } as unknown as EventTarget)).toBe(false);
  });
});

describe('reconcileKeyboardActive', () => {
  it('sets the flag while a composer textarea holds focus', () => {
    const html = root();
    reconcileKeyboardActive(html, el('TEXTAREA'));
    expect(html.active).toBe(true);
  });

  it('clears a flag left behind when the focused textarea was removed', () => {
    const html = root();
    reconcileKeyboardActive(html, el('TEXTAREA'));
    expect(html.active).toBe(true);
    // The transition that fires no focusout: the node is gone and focus has
    // fallen back to <body>. Without this sweep the flag would stay set for the
    // life of the page, and the transcript with it.
    reconcileKeyboardActive(html, el('BODY'));
    expect(html.active).toBe(false);
  });

  it('clears a flag that survived a suspend with focus dropped', () => {
    const html = root();
    html.force(true);
    reconcileKeyboardActive(html, null);
    expect(html.active).toBe(false);
  });

  it('is idempotent, so running it on every touch changes nothing on its own', () => {
    const html = root();
    const composer = el('TEXTAREA');
    reconcileKeyboardActive(html, composer);
    reconcileKeyboardActive(html, composer);
    expect(html.active).toBe(true);
  });
});
