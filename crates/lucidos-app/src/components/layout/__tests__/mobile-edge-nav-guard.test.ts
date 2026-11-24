import { describe, it, expect } from 'vitest';
import {
  shouldSuppressEdgeNavigation,
  EDGE_NAV_GUARD_LEFT_REM,
  EDGE_NAV_GUARD_RIGHT_REM,
} from '../MobileSwipeContainer';


const VW = 390;

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

  it('suppresses the 40-44px band the mobile root opens above a fixed 40px guard', () => {
    for (const clientX of [40.5, 42, 44, 45]) {
      expect(
        shouldSuppressEdgeNavigation({ ...base, remPx: MOBILE_REM, clientX }),
      ).toBe(true);
    }
  });

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

  it('does not suppress when the target is an interactive control, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, targetIsInteractive: true })).toBe(false);
  });

  it('does not suppress when a text input is focused, even at the edge', () => {
    expect(shouldSuppressEdgeNavigation({ ...base, clientX: 0, textInputFocused: true })).toBe(false);
  });

  it('guard widths are small, positive edge strips', () => {
    expect(EDGE_NAV_GUARD_LEFT_REM).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_RIGHT_REM).toBeGreaterThan(0);
    expect(EDGE_NAV_GUARD_LEFT_REM).toBeGreaterThanOrEqual(EDGE_NAV_GUARD_RIGHT_REM);
    expect(leftGuardAt(MOBILE_REM)).toBeLessThan(VW / 4);
  });
});
