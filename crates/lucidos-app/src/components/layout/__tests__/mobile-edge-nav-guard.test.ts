import { describe, it, expect } from 'vitest';
import {
  shouldSuppressEdgeNavigation,
  EDGE_NAV_GUARD_LEFT_REM,
  EDGE_NAV_GUARD_RIGHT_REM,
} from '../MobileSwipeContainer';

// ─────────────────────────────────────────────────────────────────────────────
// iOS standalone-PWA back/forward edge-swipe suppression.
//
// A standalone iOS PWA exposes NO CSS opt-out for this gesture. WebKit's edge
// recognizer also commits before the in-app SwipeTouch 8px horizontal lock, so
// an onTouchMove preventDefault runs too late. The only reliable fix is to
// preventDefault on the touchstart itself when it begins at a screen edge.
//
// Bug 1: swiping out of an open thread fired the native back gesture, showing
// two app shells overlapping mid-swipe and snapping back on release.
//
// Bug 2: swiping from the app pane to the thread pane took the PWA out to the
// workspace gateway picker. An app iframe captures every touch except those in
// the `.edge-swipe-left` zone. So a back-swipe over an app is FORCED to start
// there. A guard narrower than the zone then leaves a band that reaches the
// in-app handler unsuppressed.
//
// Bug 3: the guards were px literals (40 and 24) over rem zones (2.5rem and
// 1.25rem). Every root but one therefore mismatched. Both guards are rem now,
// and the caller measures the root per touch.
//
// shouldSuppressEdgeNavigation is the pure decision, so the edge math is tested
// without jsdom and synthetic TouchEvents (same pattern as computeAppHeight).
// ─────────────────────────────────────────────────────────────────────────────

const VW = 390; // iPhone logical width

/** The three roots that matter: the desktop default, the mobile breakpoint's
 *  112.5%, and the bottom of the UI-scale range. */
const DESKTOP_REM = 16;
const MOBILE_REM = 18;
const SMALL_SCALE_REM = 12;

const leftGuardAt = (remPx: number) => EDGE_NAV_GUARD_LEFT_REM * remPx;
const rightGuardAt = (remPx: number) => EDGE_NAV_GUARD_RIGHT_REM * remPx;

describe('shouldSuppressEdgeNavigation', () => {
  const base = {
    viewportWidth: VW,
    remPx: MOBILE_REM,
    targetIsInteractive: false,
    textInputFocused: false,
  };

  it('suppresses at the very left edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0 })).toBe(true);
  });

  it('suppresses up to and including the left guard boundary', () => {
    const clientX = leftGuardAt(MOBILE_REM);
    expect(shouldSuppressEdgeNavigation({ ...base, clientX })).toBe(true);
  });

  it('does not suppress just inside the left guard', () => {
    const clientX = leftGuardAt(MOBILE_REM) + 1;
    expect(shouldSuppressEdgeNavigation({ ...base, clientX })).toBe(false);
  });

  // Bug 2 regression pin. The whole zone must suppress, at whatever root the
  // page happens to be rendering at.
  it('suppresses across the full edge-swipe-left zone (was the leak to the gateway)', () => {
    for (const remPx of [DESKTOP_REM, MOBILE_REM, SMALL_SCALE_REM]) {
      const zone = leftGuardAt(remPx);
      for (const clientX of [1, zone / 2, zone - 0.5, zone]) {
        expect(
          shouldSuppressEdgeNavigation({ ...base, remPx, clientX }),
          `root ${remPx}px, touch at ${clientX}px inside a ${zone}px zone`,
        ).toBe(true);
      }
    }
  });

  // Bug 3, the band the px literal reopened. `.edge-swipe-left` is 45px at the
  // mobile root, and the old fixed 40 covered only 0 to 40. A touchstart in the
  // 40 to 44px band landed ON the strip unsuppressed. That is the gesture the
  // widening from 24 to 40 was meant to close.
  it('suppresses the 40-44px band the mobile root opens above a fixed 40px guard', () => {
    for (const clientX of [40.5, 42, 44, 45]) {
      expect(
        shouldSuppressEdgeNavigation({ ...base, remPx: MOBILE_REM, clientX }),
      ).toBe(true);
    }
  });

  // Bug 3 from the other side. At 75% UI scale the strip is 30px, so a fixed
  // 40px guard preventDefaulted touchstarts on ordinary transcript content and
  // blocked vertical scrolling there.
  it('leaves content beyond the shrunken zone alone at a small UI scale', () => {
    for (const clientX of [31, 35, 39, 40]) {
      expect(
        shouldSuppressEdgeNavigation({ ...base, remPx: SMALL_SCALE_REM, clientX }),
      ).toBe(false);
    }
  });

  it('suppresses at the very right edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW })).toBe(true);
  });

  it('suppresses down to and including the right guard boundary', () => {
    const clientX = VW - rightGuardAt(MOBILE_REM);
    expect(shouldSuppressEdgeNavigation({ ...base, clientX })).toBe(true);
  });

  it('does not suppress just inside the right guard', () => {
    const clientX = VW - rightGuardAt(MOBILE_REM) - 1;
    expect(shouldSuppressEdgeNavigation({ ...base, clientX })).toBe(false);
  });

  // The right zone scales too. It is 22.5px at the mobile root, so the old
  // fixed 24 over-reached it, and over-reached much further below that.
  it('tracks the right zone across roots', () => {
    for (const remPx of [DESKTOP_REM, MOBILE_REM, SMALL_SCALE_REM]) {
      const zone = rightGuardAt(remPx);
      expect(shouldSuppressEdgeNavigation({ ...base, remPx, clientX: VW - zone })).toBe(true);
      expect(shouldSuppressEdgeNavigation({ ...base, remPx, clientX: VW - zone - 1 })).toBe(false);
    }
  });

  it('does not suppress mid-screen', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: VW / 2 })).toBe(false);
  });

  // Exemptions: edge controls (pin button, hamburger, content nav) must keep
  // their taps. preventDefault on touchstart would swallow the emulated click.
  it('does not suppress when the target is an interactive control, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, targetIsInteractive: true })).toBe(false);
  });

  // While typing, the keyboard is up and pane swipes are already disabled;
  // never eat edge touches out from under a focused input.
  it('does not suppress when a text input is focused, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, textInputFocused: true })).toBe(false);
  });

  it('guard widths are small, positive edge strips', () => {
    // Each strip matches its `.edge-swipe-*` zone in mobile.css, so a swipe over
    // an app iframe can never leak to native back-nav. Both stay narrow enough
    // to leave vertical scrolling and content taps outside them alone, at every
    // root the UI scale can produce.
    expect(EDGE_NAV_GUARD_LEFT_REM).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_RIGHT_REM).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_LEFT_REM).toBeGreaterThanOrEqual(EDGE_NAV_GUARD_RIGHT_REM);
    expect(leftGuardAt(MOBILE_REM)).toBeLessThan(VW / 4);
  });
});
