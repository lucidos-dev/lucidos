import { describe, it, expect } from 'vitest';
import { shouldSuppressEdgeNavigation, EDGE_NAV_GUARD_PX } from '../MobileSwipeContainer';

// ─────────────────────────────────────────────────────────────────────────────
// iOS standalone-PWA back/forward edge-swipe suppression.
//
// Bug: swiping out of an open thread on the installed iOS PWA fired WebKit's
// native back-navigation gesture, previewing a snapshot of a previous in-app
// state — two app shells overlapping mid-swipe, snapping back on release.
//
// Root cause: a standalone iOS PWA exposes NO CSS opt-out for this gesture
// (overscroll-behavior-x / touch-action do not disable it), and WebKit's edge
// recognizer commits before the in-app SwipeTouch 8px horizontal lock — so the
// existing onTouchMove preventDefault ran too late. The only reliable fix is to
// preventDefault on the touchstart itself when it begins at a screen edge.
//
// shouldSuppressEdgeNavigation is the pure decision so the edge math is tested
// without jsdom + synthetic TouchEvents (same pattern as computeAppHeight).
// ─────────────────────────────────────────────────────────────────────────────

const VW = 390; // iPhone logical width

describe('shouldSuppressEdgeNavigation', () => {
  const base = { viewportWidth: VW, targetIsInteractive: false, textInputFocused: false };

  it('suppresses at the very left edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0 })).toBe(true);
  });

  it('suppresses up to and including the left guard boundary', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: EDGE_NAV_GUARD_PX })).toBe(true);
  });

  it('does not suppress just inside the left guard', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: EDGE_NAV_GUARD_PX + 1 })).toBe(false);
  });

  it('suppresses at the very right edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW })).toBe(true);
  });

  it('suppresses down to and including the right guard boundary', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW - EDGE_NAV_GUARD_PX })).toBe(true);
  });

  it('does not suppress just inside the right guard', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW - EDGE_NAV_GUARD_PX - 1 })).toBe(false);
  });

  it('does not suppress mid-screen', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW / 2 })).toBe(false);
  });

  // Exemptions: edge controls (pin button, hamburger, content nav) must keep
  // their taps — preventDefault on touchstart would swallow the emulated click.
  it('does not suppress when the target is an interactive control, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, targetIsInteractive: true })).toBe(false);
  });

  // While typing, the keyboard is up and pane swipes are already disabled;
  // never eat edge touches out from under a focused input.
  it('does not suppress when a text input is focused, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, textInputFocused: true })).toBe(false);
  });

  it('guard width is a small, positive edge strip', () => {
    // Narrow enough that vertical scrolling and content taps (which carry their
    // own horizontal padding) outside the strip are unaffected.
    expect(EDGE_NAV_GUARD_PX).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_PX).toBeLessThan(VW / 4);
  });
});
