// @vitest-environment jsdom
/**
 * The thread-drawer toggle reads what is drawn. A window too narrow for the
 * drawer even alone disables it, and the tooltip says why. The toggle is
 * aria-disabled rather than disabled, which keeps the pointer for its tooltip.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ThreadToggleButton } from '../ThreadToggleButton';
import { THREAD_LIST_TOO_NARROW } from '../../../store/actions/pane';
import { threadDrawerOpen, splitRatio } from '../../../store/store';
import { homeRailShown } from '../../../store/homeRail';
import { dividerBandPx, paneFloors } from '../../../store/paneMinimums';
import { layoutGeometry } from '../../layout/drawnLayout';
import { viewportIsMobile } from '../../../utils/viewport';
import type { AnyVNode } from '../../layout/__tests__/vnodeWalk';

const FLOORS = paneFloors();
const DRAWER_ONLY = FLOORS.minThreadPx + FLOORS.drawerPx + dividerBandPx();

function resize(windowPx: number) {
  layoutGeometry.value = { windowPx, floors: FLOORS, dividerPx: dividerBandPx() };
}

const toggle = () => (ThreadToggleButton({}) as AnyVNode).props;

describe('ThreadToggleButton in a window too narrow for the drawer', () => {
  const railWasShown = homeRailShown.value;

  beforeEach(() => {
    viewportIsMobile.value = false;
    splitRatio.value = 0.4;
    threadDrawerOpen.value = false;
    homeRailShown.value = false;
  });

  afterEach(() => {
    layoutGeometry.value = null;
    homeRailShown.value = railWasShown;
  });

  it('is enabled where the drawer fits', () => {
    resize(DRAWER_ONLY);
    expect(toggle()['aria-disabled']).toBeUndefined();
    expect(String(toggle()['data-tooltip'])).not.toContain(THREAD_LIST_TOO_NARROW);
  });

  it('is disabled one pixel short, and says the window is too narrow', () => {
    resize(DRAWER_ONLY - 1);
    expect(toggle()['aria-disabled']).toBe(true);
    expect(toggle()['data-tooltip']).toBe(THREAD_LIST_TOO_NARROW);
  });

  it('stays enabled while the drawer is drawn, so it can hide', () => {
    threadDrawerOpen.value = true;
    resize(DRAWER_ONLY);
    expect(toggle()['aria-disabled']).toBeUndefined();
  });

  it('is enabled before the window is measured', () => {
    layoutGeometry.value = null;
    expect(toggle()['aria-disabled']).toBeUndefined();
  });

  it('is never disabled on a phone, which has no drawer', () => {
    viewportIsMobile.value = true;
    resize(DRAWER_ONLY - 1);
    expect(toggle()['aria-disabled']).toBeUndefined();
  });
});
