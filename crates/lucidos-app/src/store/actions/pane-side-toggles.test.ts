// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { focusedPane, splitRatio, threadDrawerOpen, threadDrawerWidth } from '../store';
import { dashboardRailShown, dashboardRailWidth } from '../dashboardRail';
import { dividerBandPx, paneFloors } from '../paneMinimums';
import { DEFAULT_SPLIT_RATIO } from '../../components/layout/splitHelpers';
import { drawnDrawerShown, drawnRailShown, layoutGeometry } from '../../components/layout/drawnLayout';
import {
  focusOrToggleThreadDrawer, showDashboardRail, showThreadList, startThreadSearch, toggleDashboardRail, toggleThreads,
} from './pane';
import { revealDashboard } from './dashboardNav';

// A side pane's toggle reads what is DRAWN, and a tap swaps, else is disabled.
// The drawer outranks the rail, so only showing the rail ever closes the drawer.

const FLOORS = paneFloors();
const DIVIDER = dividerBandPx();
const DRAWER_ONLY = FLOORS.minThreadPx + FLOORS.drawerPx + DIVIDER;
const BOTH_SIDES = DRAWER_ONLY + FLOORS.railPx;
const RAIL_ONLY = FLOORS.minThreadPx + FLOORS.railPx;
const ROOMY = BOTH_SIDES + FLOORS.minContentPx + 400;
const NO_ROOM_FOR_DRAWER = DRAWER_ONLY - 1;
const NO_ROOM_FOR_RAIL = RAIL_ONLY - 1;
// The rail yields to the drawn drawer, and fits alone.
const RAIL_YIELDS = BOTH_SIDES - 1;

function resize(windowPx: number) {
  layoutGeometry.value = { windowPx, floors: FLOORS, dividerPx: DIVIDER };
}

function store(drawerOpen: boolean, railShown: boolean) {
  threadDrawerOpen.value = drawerOpen;
  dashboardRailShown.value = railShown;
}

describe('side pane toggles read what is drawn', () => {
  const was = { drawerOpen: threadDrawerOpen.value, railShown: dashboardRailShown.value };

  beforeEach(() => {
    (globalThis as any).innerWidth = 1280;
    splitRatio.value = 0.4;
    threadDrawerWidth.value = FLOORS.drawerPx;
    dashboardRailWidth.value = FLOORS.railPx;
    focusedPane.value = 'thread';
    store(false, false);
    resize(ROOMY);
  });

  afterEach(() => {
    layoutGeometry.value = null;
    store(was.drawerOpen, was.railShown);
  });

  describe('the drawer', () => {
    it('hides a drawn drawer and leaves the rail alone', () => {
      store(true, true);
      toggleThreads();
      expect([threadDrawerOpen.value, dashboardRailShown.value]).toEqual([false, true]);
    });

    it('shows a hidden drawer, and a rail beside it yields drawn-only', () => {
      resize(DRAWER_ONLY);
      store(false, true);
      toggleThreads();
      expect(threadDrawerOpen.value).toBe(true);
      expect(dashboardRailShown.value).toBe(true);
      expect(drawnDrawerShown.value).toBe(true);
      expect(drawnRailShown.value).toBe(false);
    });

    it('re-expands a collapsed Conversation, since an open drawer rides it', () => {
      splitRatio.value = 0;
      store(true, false);
      toggleThreads();
      expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
      expect(drawnDrawerShown.value).toBe(true);
    });

    it('does nothing when the window cannot hold it even alone', () => {
      resize(NO_ROOM_FOR_DRAWER);
      store(false, false);
      toggleThreads();
      expect(threadDrawerOpen.value).toBe(false);
      expect(showThreadList()).toBe(false);
      expect(threadDrawerOpen.value).toBe(false);
    });

    it('does not close a drawer the layout yielded, which is not drawn', () => {
      resize(NO_ROOM_FOR_DRAWER);
      store(true, false);
      toggleThreads();
      expect(threadDrawerOpen.value).toBe(true);
    });

    it('moves no focus where it cannot show the drawer', () => {
      resize(NO_ROOM_FOR_DRAWER);
      expect(focusOrToggleThreadDrawer()).toBe(false);
      expect(focusedPane.value).toBe('thread');
      startThreadSearch();
      expect(focusedPane.value).toBe('thread');
    });
  });

  describe('the rail', () => {
    it('hides a drawn rail', () => {
      store(false, true);
      toggleDashboardRail();
      expect(dashboardRailShown.value).toBe(false);
    });

    it('shows a hidden rail beside the drawer without closing it, where both fit', () => {
      store(true, false);
      toggleDashboardRail();
      expect([threadDrawerOpen.value, dashboardRailShown.value]).toEqual([true, true]);
    });

    it('swaps a yielded rail for the drawer, closing the drawer for real', () => {
      resize(RAIL_YIELDS);
      store(true, true);
      expect(drawnRailShown.value).toBe(false);
      focusedPane.value = 'drawer';
      toggleDashboardRail();
      expect(threadDrawerOpen.value).toBe(false);
      expect(dashboardRailShown.value).toBe(true);
      expect(drawnRailShown.value).toBe(true);
      expect(drawnDrawerShown.value).toBe(false);
      expect(focusedPane.value).toBe('thread');
    });

    it('closes the drawer only when that is what makes room', () => {
      resize(RAIL_YIELDS);
      store(false, false);
      showDashboardRail();
      expect([threadDrawerOpen.value, dashboardRailShown.value]).toEqual([false, true]);
    });

    it('is disabled when the window cannot hold it even alone, and keeps the drawer', () => {
      resize(NO_ROOM_FOR_RAIL);
      store(true, false);
      toggleDashboardRail();
      expect([threadDrawerOpen.value, dashboardRailShown.value]).toEqual([true, false]);
    });

    it('reveals the dashboard by the same swap', () => {
      resize(RAIL_YIELDS);
      store(true, false);
      revealDashboard();
      expect([threadDrawerOpen.value, dashboardRailShown.value]).toEqual([false, true]);
      expect(drawnRailShown.value).toBe(true);
    });

    it('never yields again right after its toggle showed it', () => {
      for (const windowPx of [RAIL_YIELDS, BOTH_SIDES, ROOMY, RAIL_ONLY + 1]) {
        resize(windowPx);
        store(true, false);
        toggleDashboardRail();
        expect(drawnRailShown.value, `window ${windowPx}`).toBe(true);
      }
    });
  });
});
