import { describe, it, expect } from 'vitest';
import { toastAutofocusTarget } from './toastFocus';

describe('toastAutofocusTarget', () => {
  it('returns null for a plain toast (no actions) — never steals focus', () => {
    expect(toastAutofocusTarget({})).toBeNull();
    expect(toastAutofocusTarget({ dismissable: true })).toBeNull();
  });

  it('focuses the primary action when it is non-destructive', () => {
    // The Restart toast: Restart (neutral) + Dismiss (danger).
    expect(toastAutofocusTarget({
      action: { label: 'Restart', onClick: () => {} },
      secondaryAction: { label: 'Dismiss', onClick: () => {}, variant: 'danger' },
    })).toBe('primary');
  });

  it('skips a destructive primary and focuses a non-destructive secondary', () => {
    expect(toastAutofocusTarget({
      action: { label: 'Delete', onClick: () => {}, variant: 'danger' },
      secondaryAction: { label: 'Keep', onClick: () => {} },
    })).toBe('secondary');
  });

  it('falls back to the close (X) when the only action is destructive', () => {
    // A dismissable toast whose single action is danger: focus the X (safe —
    // it only dismisses), not the destructive button.
    expect(toastAutofocusTarget({
      action: { label: 'Discard', onClick: () => {}, variant: 'danger' },
    })).toBe('close');
  });

  it('returns null when the only action is destructive AND there is no dismiss', () => {
    // The non-dismissable Apply-All progress toast: single danger Cancel, no X.
    // Pre-focusing Cancel would let a reflexive Enter abort the batch — so leave
    // focus put; the button is still reachable by Tab.
    expect(toastAutofocusTarget({
      action: { label: 'Cancel', onClick: () => {}, variant: 'danger' },
      dismissable: false,
    })).toBeNull();
  });

  it('focuses the primary even when a secondary is present (primary wins)', () => {
    // The navigation-offer toast: a single neutral Open (no secondary).
    expect(toastAutofocusTarget({
      action: { label: 'Open', onClick: () => {} },
    })).toBe('primary');
  });

  it('focuses a non-destructive secondary when there is no primary action', () => {
    expect(toastAutofocusTarget({
      secondaryAction: { label: 'Undo', onClick: () => {} },
    })).toBe('secondary');
  });
});
