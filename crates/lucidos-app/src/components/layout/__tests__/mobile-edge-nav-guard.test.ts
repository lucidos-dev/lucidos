import { describe, it, expect } from 'vitest';
import {
  shouldSuppressEdgeNavigation,
  EDGE_NAV_GUARD_LEFT_PX,
  EDGE_NAV_GUARD_RIGHT_PX,
} from '../MobileSwipeContainer';

// ─────────────────────────────────────────────────────────────────────────────
// iOS standalone-PWA back/forward edge-swipe suppression.
//
// Bug 1: swiping out of an open thread on the installed iOS PWA fired WebKit's
// native back-navigation gesture, previewing a snapshot of a previous in-app
// state — two app shells overlapping mid-swipe, snapping back on release.
//
// Bug 2: swiping from the app (content) pane to the thread pane navigated the
// PWA out to the workspace gateway picker. The content pane renders the app as
// an iframe, which captures every touch except those in the `.edge-swipe-left`
// zone — so a back-swipe over the app is FORCED to start in that 40px zone. The
// old 24px guard left a 24–40px band where the in-app swipe ran but the native
// back-gesture was NOT suppressed, so WebKit popped history to the gateway. The
// left guard now matches the full `.edge-swipe-left` zone width (40px).
//
// Root cause (both): a standalone iOS PWA exposes NO CSS opt-out for this
// gesture (overscroll-behavior-x / touch-action do not disable it), and
// WebKit's edge recognizer commits before the in-app SwipeTouch 8px horizontal
// lock — so the existing onTouchMove preventDefault ran too late. The only
// reliable fix is to preventDefault on the touchstart itself when it begins at
// a screen edge.
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
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: EDGE_NAV_GUARD_LEFT_PX })).toBe(true);
  });

  it('does not suppress just inside the left guard', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: EDGE_NAV_GUARD_LEFT_PX + 1 })).toBe(false);
  });

  // Bug 2 regression pin: a back-swipe over the app iframe starts in the
  // `.edge-swipe-left` zone (40px). The old 24px guard let 24–40px leak to
  // WebKit's native back-nav → the gateway picker. The whole zone must suppress.
  it('suppresses across the full edge-swipe-left zone (was the 24-40px leak to the gateway)', () => {
    for (const clientX of [25, 30, 35, 40]) {
      expect(shouldSuppressEdgeNavigation({ ...base, clientX })).toBe(true);
    }
  });

  it('suppresses at the very right edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW })).toBe(true);
  });

  it('suppresses down to and including the right guard boundary', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW - EDGE_NAV_GUARD_RIGHT_PX })).toBe(true);
  });

  it('does not suppress just inside the right guard', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW - EDGE_NAV_GUARD_RIGHT_PX - 1 })).toBe(false);
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

  it('guard widths are small, positive edge strips', () => {
    // The left strip matches the `.edge-swipe-left` zone (2.5rem ≈ 40px) so a
    // swipe over the app iframe can never leak to native back-nav; the right
    // strip matches `.edge-swipe-right` (1.25rem ≈ 20px). Both stay narrow
    // enough that vertical scrolling and content taps outside them are
    // unaffected.
    expect(EDGE_NAV_GUARD_LEFT_PX).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_RIGHT_PX).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_LEFT_PX).toBeGreaterThanOrEqual(EDGE_NAV_GUARD_RIGHT_PX);
    expect(EDGE_NAV_GUARD_LEFT_PX).toBeLessThan(VW / 4);
  });
});
