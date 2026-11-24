import { describe, it, expect } from 'vitest';
import { isGhostFocus, KEYBOARD_RISE_GRACE_MS } from '../MobileSwipeContainer';

const ghost = {
  fieldFocused: true,
  keyboardShrunk: false,
  focusHeldMs: 5000,
  hardwareKeyPressed: false,
};

describe('isGhostFocus', () => {
  it('is true for a field focused long ago with the keyboard down', () => {
    expect(isGhostFocus(ghost)).toBe(true);
  });

  it('is false while the keyboard is up, so typing is never interrupted', () => {
    expect(isGhostFocus({ ...ghost, keyboardShrunk: true })).toBe(false);
  });

  it('is false while the keyboard may still be rising after the focus', () => {
    expect(isGhostFocus({ ...ghost, focusHeldMs: KEYBOARD_RISE_GRACE_MS - 1 })).toBe(false);
  });

  it('is false with no field focused', () => {
    expect(isGhostFocus({ ...ghost, fieldFocused: false })).toBe(false);
  });

  it('is false once a hardware keyboard typed, which never shrinks the viewport', () => {
    expect(isGhostFocus({ ...ghost, hardwareKeyPressed: true })).toBe(false);
  });
});
