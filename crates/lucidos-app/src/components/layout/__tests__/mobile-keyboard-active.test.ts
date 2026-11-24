import { describe, it, expect } from 'vitest';
import { isKeyboardActiveTarget, reconcileKeyboardActive } from '../MobileSwipeContainer';

const ATTR = 'data-keyboard-active';

function el(tagName: string): EventTarget {
  return { tagName } as unknown as EventTarget;
}

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

  it('is false for an <input>, so the header search bar stays interactive', () => {
    expect(isKeyboardActiveTarget(el('INPUT'))).toBe(false);
  });

  it('is false for nothing at all', () => {
    expect(isKeyboardActiveTarget(null)).toBe(false);
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
