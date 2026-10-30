// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const isTauri = vi.fn(() => true);
vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  isTauri: () => isTauri(),
}));

const setViewMenuState = vi.fn((_items: unknown) => Promise.resolve());
let clickHandler: ((e: { payload: string }) => void) | null = null;
const stopListening = vi.fn();
const listen = vi.fn((_event: string, handler: (e: { payload: string }) => void) => {
  clickHandler = handler;
  return Promise.resolve(stopListening);
});
vi.mock('../../utils/tauri', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/tauri')>()),
  setViewMenuState: (items: unknown) => setViewMenuState(items),
  listen: (event: string, handler: (e: { payload: string }) => void) => listen(event, handler),
}));

import {
  focusedPane, mobileView, preferences, setMobileView, splitRatio, threadDrawerOpen, threadDrawerWidth, threadMap,
} from '../store';
import type { ThreadState } from '../thread-events';
import { dashboardRailShown, dashboardRailWidth } from '../dashboardRail';
import { dashboardPanelOpen } from '../dashboards';
import { dividerBandPx, paneFloors } from '../paneMinimums';
import { DEFAULT_SPLIT_RATIO } from '../../components/layout/splitHelpers';
import { drawnSplitRatio, layoutGeometry } from '../../components/layout/drawnLayout';
import { viewportIsMobile } from '../../utils/viewport';
import { KEYBINDINGS_PREF_KEY } from './keybindings';
import { VIEW_MENU_ITEM_CLICKED_EVENT, type ViewMenuItemState, type ViewMenuPane } from '../../utils/tauri';
import { handleViewMenuClick, setupViewMenu, viewMenuItems } from './view-menu';

const FLOORS = paneFloors();
const DIVIDER = dividerBandPx();
const DRAWER_ONLY = FLOORS.minThreadPx + FLOORS.drawerPx + DIVIDER;
const RAIL_ONLY = FLOORS.minThreadPx + FLOORS.railPx;
const ROOMY = DRAWER_ONLY + FLOORS.railPx + FLOORS.minContentPx + 400;
const TOO_NARROW_FOR_SIDES = Math.min(DRAWER_ONLY, RAIL_ONLY) - 1;

function resize(windowPx: number) {
  layoutGeometry.value = { windowPx, floors: FLOORS, dividerPx: DIVIDER };
}

function withHome() {
  const home = { meta: { id: 'home', home: true } } as unknown as ThreadState;
  threadMap.value = new Map([['home', home]]);
}

function item(pane: ViewMenuPane): ViewMenuItemState {
  return viewMenuItems().find((i) => i.pane === pane)!;
}

function shownFlags() {
  return viewMenuItems().map((i) => [i.pane, i.shown]);
}

describe('the View menu items', () => {
  beforeEach(() => {
    (globalThis as any).innerWidth = 1280;
    viewportIsMobile.value = false;
    splitRatio.value = DEFAULT_SPLIT_RATIO;
    threadDrawerWidth.value = FLOORS.drawerPx;
    dashboardRailWidth.value = FLOORS.railPx;
    threadDrawerOpen.value = true;
    dashboardRailShown.value = false;
    dashboardPanelOpen.value = false;
    focusedPane.value = 'thread';
    preferences.value = { status: 'loaded', data: {} };
    withHome();
    resize(ROOMY);
  });

  afterEach(() => {
    layoutGeometry.value = null;
    threadMap.value = new Map();
    preferences.value = { status: 'not-loaded' };
    viewportIsMobile.value = false;
  });

  describe('state in the split layout', () => {
    it('reads each pane as drawn, in menu order', () => {
      expect(shownFlags()).toEqual([
        ['thread-drawer', true], ['thread-pane', true], ['content-pane', true], ['dashboard', false],
      ]);
      expect(viewMenuItems().every((i) => i.enabled)).toBe(true);
    });

    it('a maximised Canvas hides the thread pane and the drawer that rides it', () => {
      splitRatio.value = 0;
      expect(shownFlags()).toEqual([
        ['thread-drawer', false], ['thread-pane', false], ['content-pane', true], ['dashboard', false],
      ]);
      expect(item('thread-drawer').enabled).toBe(true);
    });

    it('a Canvas that yields for room reads hidden', () => {
      resize(FLOORS.minThreadPx + FLOORS.minContentPx - 1);
      threadDrawerOpen.value = false;
      expect(drawnSplitRatio.value).toBe(1);
      expect(item('content-pane').shown).toBe(false);
      expect(item('thread-pane').shown).toBe(true);
    });

    it('disables a hidden side pane the window has no room for', () => {
      threadDrawerOpen.value = false;
      resize(TOO_NARROW_FOR_SIDES);
      expect(item('thread-drawer')).toMatchObject({ shown: false, enabled: false });
      expect(item('dashboard')).toMatchObject({ shown: false, enabled: false });
      expect(item('thread-pane').enabled).toBe(true);
      expect(item('content-pane').enabled).toBe(true);
    });

    it('disables a hidden dashboard until Home exists', () => {
      threadMap.value = new Map();
      expect(item('dashboard')).toMatchObject({ shown: false, enabled: false });
      dashboardRailShown.value = true;
      expect(item('dashboard')).toMatchObject({ shown: true, enabled: true });
    });
  });

  describe('state in the phone layout', () => {
    beforeEach(() => {
      viewportIsMobile.value = true;
    });

    afterEach(() => {
      setMobileView('thread');
    });

    it('shows only the pane on screen, and enables every item', () => {
      setMobileView('threads');
      expect(shownFlags()).toEqual([
        ['thread-drawer', true], ['thread-pane', false], ['content-pane', false], ['dashboard', false],
      ]);
      expect(viewMenuItems().every((i) => i.enabled)).toBe(true);
    });

    it('counts the dashboard panel as shown only over the thread pane', () => {
      dashboardPanelOpen.value = true;
      setMobileView('thread');
      expect(item('dashboard').shown).toBe(true);
      setMobileView('content');
      expect(item('dashboard').shown).toBe(false);
    });
  });

  describe('accelerators', () => {
    it('carries each pane binding in menu syntax, and none for the dashboard', () => {
      expect(viewMenuItems().map((i) => i.accelerator)).toEqual([
        'Ctrl+Shift+1', 'Ctrl+Shift+2', 'Ctrl+Shift+3', null,
      ]);
    });

    it('follows a rebind', () => {
      preferences.value = {
        status: 'loaded',
        data: { [KEYBINDINGS_PREF_KEY]: JSON.stringify({ toggleThreadDrawer: 'mod+alt+ArrowLeft' }) },
      };
      expect(item('thread-drawer').accelerator).toBe('Cmd+Alt+ArrowLeft');
    });

    it('sends none for a key the menu cannot draw', () => {
      preferences.value = {
        status: 'loaded',
        data: { [KEYBINDINGS_PREF_KEY]: JSON.stringify({ toggleThreadPane: 'mod+F13' }) },
      };
      expect(item('thread-pane').accelerator).toBeNull();
    });
  });

  describe('clicks in the split layout', () => {
    it('the drawer item hides a drawn drawer and shows it again', () => {
      handleViewMenuClick('thread-drawer');
      expect(threadDrawerOpen.value).toBe(false);
      handleViewMenuClick('thread-drawer');
      expect(item('thread-drawer').shown).toBe(true);
    });

    it('the drawer item brings back a drawer a maximised Canvas hid', () => {
      splitRatio.value = 0;
      handleViewMenuClick('thread-drawer');
      expect(item('thread-drawer').shown).toBe(true);
      expect(item('thread-pane').shown).toBe(true);
    });

    it('the thread pane item hides it with no focus stage, and the marker leaves it', () => {
      handleViewMenuClick('thread-pane');
      expect(splitRatio.value).toBe(0);
      expect(focusedPane.value).toBe('content');
      handleViewMenuClick('thread-pane');
      expect(item('thread-pane').shown).toBe(true);
    });

    it('the content pane item hides it with no focus stage, and the marker leaves it', () => {
      focusedPane.value = 'content';
      handleViewMenuClick('content-pane');
      expect(splitRatio.value).toBe(1);
      expect(focusedPane.value).toBe('thread');
      handleViewMenuClick('content-pane');
      expect(item('content-pane').shown).toBe(true);
    });

    it('showing a pane leaves the marker where it was', () => {
      focusedPane.value = 'content';
      splitRatio.value = 0;
      handleViewMenuClick('thread-pane');
      expect(focusedPane.value).toBe('content');
    });

    describe('in a window too narrow for both panes', () => {
      beforeEach(() => {
        threadDrawerOpen.value = false;
        resize(FLOORS.minThreadPx + FLOORS.minContentPx - 1);
      });

      it('showing the content pane hides the thread pane, and the marker leaves it', () => {
        expect(drawnSplitRatio.value).toBe(1);
        handleViewMenuClick('content-pane');
        expect(item('thread-pane').shown).toBe(false);
        expect(focusedPane.value).toBe('content');
      });

      it('showing the thread pane makes the Canvas yield, and the marker leaves it', () => {
        splitRatio.value = 0;
        focusedPane.value = 'content';
        handleViewMenuClick('thread-pane');
        expect(item('content-pane').shown).toBe(false);
        expect(focusedPane.value).toBe('thread');
      });
    });

    it('the dashboard item toggles the rail', () => {
      handleViewMenuClick('dashboard');
      expect(dashboardRailShown.value).toBe(true);
      handleViewMenuClick('dashboard');
      expect(dashboardRailShown.value).toBe(false);
    });
  });

  describe('clicks in the phone layout', () => {
    beforeEach(() => {
      viewportIsMobile.value = true;
      setMobileView('thread');
    });

    afterEach(() => {
      setMobileView('thread');
    });

    it('a hidden pane item swipes to its pane', () => {
      handleViewMenuClick('thread-drawer');
      expect(mobileView.value).toBe('threads');
      handleViewMenuClick('content-pane');
      expect(mobileView.value).toBe('content');
    });

    it('the shown pane item hides it, as the split layout does', () => {
      handleViewMenuClick('thread-pane');
      expect(mobileView.value).toBe('content');
      handleViewMenuClick('content-pane');
      expect(mobileView.value).toBe('thread');
    });

    it('the dashboard item opens the panel over the thread pane, then closes it', () => {
      setMobileView('content');
      handleViewMenuClick('dashboard');
      expect(mobileView.value).toBe('thread');
      expect(dashboardPanelOpen.value).toBe(true);
      handleViewMenuClick('dashboard');
      expect(dashboardPanelOpen.value).toBe(false);
    });
  });

  describe('setupViewMenu', () => {
    beforeEach(() => {
      setViewMenuState.mockClear();
      listen.mockClear();
      stopListening.mockClear();
      clickHandler = null;
    });

    it('does nothing off Tauri', async () => {
      isTauri.mockReturnValueOnce(false);
      const stop = await setupViewMenu();
      stop();
      expect(setViewMenuState).not.toHaveBeenCalled();
      expect(listen).not.toHaveBeenCalled();
    });

    it('pushes on change only, and routes a click to its pane', async () => {
      const stop = await setupViewMenu();
      expect(setViewMenuState).toHaveBeenCalledTimes(1);
      expect(setViewMenuState).toHaveBeenLastCalledWith(viewMenuItems());
      expect(listen).toHaveBeenCalledWith(VIEW_MENU_ITEM_CLICKED_EVENT, expect.any(Function));

      threadDrawerWidth.value = FLOORS.drawerPx + 10;
      expect(setViewMenuState).toHaveBeenCalledTimes(1);

      clickHandler!({ payload: 'dashboard' });
      expect(dashboardRailShown.value).toBe(true);
      expect(setViewMenuState).toHaveBeenCalledTimes(2);
      expect(item('dashboard').shown).toBe(true);

      stop();
      expect(stopListening).toHaveBeenCalled();
      dashboardRailShown.value = false;
      expect(setViewMenuState).toHaveBeenCalledTimes(2);
    });
  });
});
