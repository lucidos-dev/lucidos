// The macOS View menu's Show/Hide item per pane (view_menu.rs). The page owns
// the state. It derives each item from the drawn layout, pushes it to the shell
// on change, and answers the clicks the shell routes here.

import { effect } from '@preact/signals';
import { mobileView, type MobileView } from '../store';
import { dashboardPanelOpen } from '../dashboards';
import { homeThreadId } from './homeThread';
import { bindingFor } from './keybindings';
import {
  navigateToPane, showOrHideContentPane, showOrHideThreadPane, toggleDashboardRail, toggleThreads,
} from './pane';
import { setDashboardPanelOpen } from './thread-navigation';
import {
  dashboardRailCanShow, drawnDrawerShown, drawnRailShown, drawnSplitRatio, threadListCanShow,
} from '../../components/layout/drawnLayout';
import { viewportIsMobile } from '../../utils/viewport';
import { isTauri } from '../../utils/platform';
import { formatMenuAccelerator, type ShortcutId } from '../../utils/shortcuts';
import {
  VIEW_MENU_ITEM_CLICKED_EVENT, VIEW_MENU_PANES, listen, setViewMenuState,
  type ViewMenuItemState, type ViewMenuPane,
} from '../../utils/tauri';

const PANE_SHORTCUTS: Record<ViewMenuPane, ShortcutId | null> = {
  'thread-drawer': 'toggleThreadDrawer',
  'thread-pane': 'toggleThreadPane',
  'content-pane': 'toggleContentPane',
  dashboard: null,
};

/** The phone pane each item swipes to, and the one hiding it swipes to. */
const MOBILE_PANES: Record<Exclude<ViewMenuPane, 'dashboard'>, { pane: MobileView; hideTo: MobileView }> = {
  'thread-drawer': { pane: 'threads', hideTo: 'thread' },
  'thread-pane': { pane: 'thread', hideTo: 'content' },
  'content-pane': { pane: 'content', hideTo: 'thread' },
};

interface PaneVisibility {
  shown: boolean;
  enabled: boolean;
}

/** One pane at a time. The dashboard panel draws over the thread pane, so it
 *  counts as hidden off that pane. */
function mobileVisibility(pane: ViewMenuPane): PaneVisibility {
  const view = mobileView.value;
  const shown = pane === 'dashboard'
    ? dashboardPanelOpen.value && view === 'thread'
    : view === MOBILE_PANES[pane].pane;
  return { shown, enabled: true };
}

/** What the split layout draws. A hidden side pane the window has no room
 *  for is disabled, as its in-app toggle is. So is a hidden dashboard before
 *  Home exists, which is when its toggle appears. */
function splitVisibility(pane: ViewMenuPane): PaneVisibility {
  switch (pane) {
    case 'thread-drawer': {
      const shown = drawnDrawerShown.value;
      return { shown, enabled: shown || threadListCanShow() };
    }
    case 'thread-pane':
      return { shown: drawnSplitRatio.value > 0, enabled: true };
    case 'content-pane':
      return { shown: drawnSplitRatio.value < 1, enabled: true };
    case 'dashboard': {
      const shown = drawnRailShown.value;
      return { shown, enabled: shown || (homeThreadId.value !== null && dashboardRailCanShow()) };
    }
  }
}

/** Every item as this window draws it, with each pane's current binding. */
export function viewMenuItems(): ViewMenuItemState[] {
  const visibility = viewportIsMobile.value ? mobileVisibility : splitVisibility;
  return VIEW_MENU_PANES.map((pane) => {
    const shortcut = PANE_SHORTCUTS[pane];
    return {
      pane,
      ...visibility(pane),
      accelerator: shortcut ? formatMenuAccelerator(bindingFor(shortcut)) : null,
    };
  });
}

function clickOnPhone(pane: ViewMenuPane): void {
  const shown = mobileVisibility(pane).shown;
  if (pane === 'dashboard') {
    if (!shown && mobileView.value !== 'thread') navigateToPane('thread');
    setDashboardPanelOpen(!shown);
    return;
  }
  const { pane: view, hideTo } = MOBILE_PANES[pane];
  navigateToPane(shown ? hideTo : view);
}

/** Show a hidden pane or hide a shown one, with no focus stage. */
export function handleViewMenuClick(pane: ViewMenuPane): void {
  if (viewportIsMobile.value) {
    clickOnPhone(pane);
    return;
  }
  switch (pane) {
    case 'thread-drawer': toggleThreads(); break;
    case 'thread-pane': showOrHideThreadPane(); break;
    case 'content-pane': showOrHideContentPane(); break;
    case 'dashboard': toggleDashboardRail(); break;
  }
}

/** Keep the shell's View menu in step with this window, and answer its
 *  clicks. Tauri only. Returns a stop that ends both. */
export async function setupViewMenu(): Promise<() => void> {
  if (!isTauri()) return () => {};
  let pushed: string | null = null;
  const stopPushing = effect(() => {
    const items = viewMenuItems();
    const key = JSON.stringify(items);
    if (key === pushed) return;
    pushed = key;
    setViewMenuState(items).catch((err: unknown) => {
      // Telemetry carve-out (.claude/rules/frontend.md): no user intent rides
      // on a push. The menu keeps its last state, and the next layout change
      // pushes again, since the failed state is no longer the one recorded.
      pushed = null;
      console.warn('[ViewMenu] Failed to push the View menu state:', err);
    });
  });
  try {
    const stopListening = await listen<ViewMenuPane>(VIEW_MENU_ITEM_CLICKED_EVENT, (e) => {
      handleViewMenuClick(e.payload);
    });
    return () => {
      stopPushing();
      stopListening();
    };
  } catch (err) {
    stopPushing();
    throw err;
  }
}

/** {@link setupViewMenu} for `startShell`, whose installs return their stop at
 *  once. A stop that lands before the setup resolves ends it on arrival. */
export function installViewMenu(): () => void {
  let stop: (() => void) | null = null;
  let stopped = false;
  setupViewMenu()
    .then((s) => {
      if (stopped) s();
      else stop = s;
    })
    .catch(() => { /* best-effort: the in-app toggles and shortcuts still work */ });
  return () => {
    stopped = true;
    stop?.();
  };
}
