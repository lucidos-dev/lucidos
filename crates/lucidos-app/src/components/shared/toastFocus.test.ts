import { describe, it, expect } from 'vitest';
import { toastAutofocusTarget, toastTabTarget } from './toastFocus';

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

  it('returns null when noAutofocus is set, even with a non-destructive primary', () => {
    // Notification toasts pop unsolicited — a mid-typing [OK] steal would be
    // hostile (and a reflexive Enter would mark-read). Buttons stay Tab-reachable.
    expect(toastAutofocusTarget({
      action: { label: 'OK', onClick: () => {} },
      secondaryAction: { label: 'Open', onClick: () => {} },
      noAutofocus: true,
    })).toBeNull();
  });
});

describe('toastTabTarget', () => {
  it('falls through (null) when the toast has no focusable controls', () => {
    expect(toastTabTarget(0, -1, false, false)).toBeNull();
    expect(toastTabTarget(0, 0, true, false)).toBeNull();
  });

  it('forward Tab steps to the next control', () => {
    // 3 controls, on the first → second, second → third.
    expect(toastTabTarget(3, 0, false, false)).toBe(1);
    expect(toastTabTarget(3, 1, false, false)).toBe(2);
  });

  it('forward Tab wraps off the last control back to the first (cycle)', () => {
    expect(toastTabTarget(3, 2, false, false)).toBe(0);
    expect(toastTabTarget(1, 0, false, false)).toBe(0); // a lone control cycles to itself
  });

  it('forward Tab from outside the toast lands on the first control', () => {
    expect(toastTabTarget(3, -1, false, false)).toBe(0);
  });

  it('Shift+Tab exits to the focused pane from any position (no overlay)', () => {
    expect(toastTabTarget(3, 0, true, false)).toBe('exit');  // first control
    expect(toastTabTarget(3, 1, true, false)).toBe('exit');  // middle control
    expect(toastTabTarget(3, 2, true, false)).toBe('exit');  // last control
    expect(toastTabTarget(1, 0, true, false)).toBe('exit');  // lone control
  });

  it('Shift+Tab wraps backward within the toast when an overlay is open (never exits behind it)', () => {
    // An overlay owns the app: the pane is behind it, so keep focus contained in
    // the toast above the overlay by wrapping backward instead of exiting.
    expect(toastTabTarget(3, 2, true, true)).toBe(1);  // last → middle
    expect(toastTabTarget(3, 1, true, true)).toBe(0);  // middle → first
    expect(toastTabTarget(3, 0, true, true)).toBe(2);  // first wraps → last
    expect(toastTabTarget(1, 0, true, true)).toBe(0);  // lone control wraps to itself
  });

  it('forward Tab still cycles within the toast when an overlay is open (harmless — stays above it)', () => {
    expect(toastTabTarget(3, 0, false, true)).toBe(1);
    expect(toastTabTarget(3, 2, false, true)).toBe(0);
  });
});
