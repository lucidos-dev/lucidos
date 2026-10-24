/**
 * *Ghost focus*: a keyboard field holds focus while the iOS keyboard is down.
 * The iOS photo picker leaves the composer that way, and so does a focus()
 * made outside a gesture. Everything that reads focus as "the user is typing"
 * then holds: the header and title row stay away and pane swipes are refused.
 * A tap on the transcript does not end it, since iOS keeps focus on a tap that
 * lands on nothing focusable.
 *
 * The mobile shell blurs the field on the next touch elsewhere when this pure
 * decision says the focus is ghost.
 */
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
