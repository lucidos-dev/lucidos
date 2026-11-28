import {
  mobileView, setMobileView, splitRatio, threadDrawerOpen, threadDrawerWidth,
  DEFAULT_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY,
  MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, type MobileView,
  focusedPane, type FocusedPane,
  activeMenuItem, panelOverlay, settingsSubview,
  drawerGrouping, setDrawerGrouping, showToast,
} from '../store';
import { setDashboardRailShown } from '../dashboardRail';
import { contentViewKey } from '../../components/layout/contentViewKey';
import { minDrawerWidth, minThreadPanePx, minContentPanePx, splitBounds } from '../paneMinimums';
import { forceCloseDrawer } from '../../components/layout/drawerState';
import {
  setSplitRatio, DEFAULT_SPLIT_RATIO,
  computeStepRatio, computeDrawerStepWidth,
  contentShownRatio, toggleThreadPaneRatio, toggleContentPaneRatio,
  KEYBOARD_RESIZE_STEP_PX,
} from '../../components/layout/splitHelpers';
import {
  drawnDrawerShown, drawnDrawerWidth, drawnRailShown, drawnSplitRatio, dashboardRailCanShow, dashboardRailFitsBesideDrawer, splitFitsBoth,
  threadListCanShow,
} from '../../components/layout/drawnLayout';
import { isMobile } from '../../utils/viewport';
import { focusPaneMainControl, paneHolding, reconcilePaneFocus } from '../../components/layout/paneFocus';
import { isTextInput } from '../../utils/dom';
import { markNavigationStart } from '../../utils/navigationMarks';
import { threadFilterPanelOpen, openThreadFilterPanel, toggleThreadFilterPanel } from '../threadFilterPanel';
import { openThreadSearch, closeThreadSearch, focusThreadSearchInput } from '../threadSearch';

function focusPaneAndControl(pane: FocusedPane): void {
  focusedPane.value = pane;
  focusPaneMainControl(pane);
}

export function resolveSwipePane(delta: number): MobileView | null {
  const currentIndex = PANE_INDEX[mobileView.value];
  const newIndex = Math.max(0, Math.min(PANE_COUNT - 1, currentIndex + delta));
  return newIndex !== currentIndex ? MOBILE_VIEWS[newIndex] : null;
}

export function swipeToNeighbourPane(paneDelta: -1 | 1): void {
  const target = resolveSwipePane(paneDelta);
  if (target) navigateToPane(target);
}

export function navigateToPane(view: MobileView) {
  markNavigationStart('pane', view, performance.now());
  forceCloseDrawer();
  if (view !== 'thread') threadDrawerOpen.value = false;
  setMobileView(view);
}

export function revealContentPane() {
  if (isMobile()) {
    navigateToPane('content');
    return;
  }
  markNavigationStart(
    'content',
    'content-pane',
    performance.now(),
    contentViewKey(activeMenuItem.value, panelOverlay.value, settingsSubview.value),
  );
  const typing = takeTypingHold();
  if (!typing) focusedPane.value = 'content';
  if (drawnSplitRatio.value >= 1 && (!typing || splitFitsBoth())) {
    setSplitRatio(contentShownRatio(splitFitsBoth()));
  }
  reconcilePaneFocus('content');
}

let typingHold: Element | null = null;

export function holdFocusedPaneWhileTyping(): void {
  const active = document.activeElement;
  const pane = active && isTextInput(active) ? paneHolding(active) : null;
  typingHold = pane !== null && pane !== 'content' ? active : null;
}

export function releaseTypingHold(): void {
  typingHold = null;
}

function takeTypingHold(): boolean {
  const hold = typingHold;
  typingHold = null;
  return !!hold && hold === document.activeElement;
}

export function revealThreadPane(): void {
  if (isMobile()) {
    navigateToPane('thread');
    return;
  }
  const wasCollapsed = drawnSplitRatio.value <= 0;
  if (wasCollapsed) setSplitRatio(DEFAULT_SPLIT_RATIO);
  if (wasCollapsed || focusedPane.value === 'content') {
    focusedPane.value = 'thread';
    reconcilePaneFocus('thread');
  }
}

export function focusPane(pane: FocusedPane): void {
  if (isMobile()) return;
  focusedPane.value = pane;
  reconcilePaneFocus(pane);
}

export const THREAD_LIST_TOO_NARROW = 'The window is too narrow for the thread list';
export const DASHBOARD_TOO_NARROW = 'The window is too narrow for the dashboard';

function closeThreadDrawer(): void {
  threadDrawerOpen.value = false;
  if (focusedPane.value === 'drawer') {
    focusedPane.value = 'thread';
    reconcilePaneFocus('thread');
  }
}

export function toggleThreads() {
  if (isMobile()) {
    navigateToPane('threads');
    return;
  }
  if (drawnDrawerShown.value) closeThreadDrawer();
  else showThreadList();
}

export type DrawerShortcutAction = 'open-focus' | 'focus' | 'close';

export function computeDrawerShortcutAction(
  visible: boolean, drawerFocused: boolean,
): DrawerShortcutAction {
  if (!visible) return 'open-focus';
  if (!drawerFocused) return 'focus';
  return 'close';
}

export function focusOrToggleThreadDrawer(): boolean {
  if (isMobile()) { navigateToPane('threads'); return false; }
  const action = computeDrawerShortcutAction(drawnDrawerShown.value, focusedPane.value === 'drawer');
  if (action === 'close') {
    toggleThreads();
    return false;
  }
  if (action === 'open-focus' && !showThreadList()) return false;
  focusPaneAndControl('drawer');
  return true;
}

function threadListHasRoom(): boolean {
  if (threadListCanShow()) return true;
  showToast(THREAD_LIST_TOO_NARROW, 'info');
  return false;
}

export function showThreadList(): boolean {
  if (isMobile()) { navigateToPane('threads'); return true; }
  if (!threadListHasRoom()) return false;
  threadDrawerOpen.value = true;
  if (drawnSplitRatio.value <= 0) setSplitRatio(DEFAULT_SPLIT_RATIO);
  return true;
}

export function threadListShown(): boolean {
  if (isMobile()) return mobileView.value === 'threads';
  return drawnDrawerShown.value;
}

export function showDashboardRail(): void {
  if (!dashboardRailCanShow()) {
    showToast(DASHBOARD_TOO_NARROW, 'info');
    return;
  }
  if (!dashboardRailFitsBesideDrawer()) closeThreadDrawer();
  setDashboardRailShown(true);
}

export function toggleDashboardRail(): void {
  if (drawnRailShown.value) setDashboardRailShown(false);
  else showDashboardRail();
}

export function toggleThreadFilter(): void {
  if (drawerGrouping.value === 'ongoing') {
    if (!threadListHasRoom()) return;
    setDrawerGrouping('folders');
    showThreadList();
    openThreadFilterPanel();
  } else if (threadListShown()) toggleThreadFilterPanel();
  else {
    if (!showThreadList()) return;
    openThreadFilterPanel();
  }
  if (threadFilterPanelOpen.value) closeThreadSearch();
  if (!isMobile()) focusPaneAndControl('drawer');
}

export function startThreadSearch(): void {
  if (!showThreadList()) return;
  openThreadSearch();
  if (!isMobile()) focusedPane.value = 'drawer';
  requestAnimationFrame(() => focusThreadSearchInput());
}

export function toggleSplitFromDivider(): void {
  const ratio = drawnSplitRatio.value;
  if (ratio === 0 || ratio >= 1) {
    setSplitRatio(ratio === 0 ? DEFAULT_SPLIT_RATIO : contentShownRatio(splitFitsBoth()));
    return;
  }
  setSplitRatio(0);
  if (focusedPane.value !== 'content') focusPaneAndControl('content');
}

export function showOrHideThreadPane(): void {
  setSplitRatio(toggleThreadPaneRatio(drawnSplitRatio.value));
  moveMarkerOffHiddenPane();
}

export function showOrHideContentPane(): void {
  setSplitRatio(toggleContentPaneRatio(drawnSplitRatio.value, splitFitsBoth()));
  moveMarkerOffHiddenPane();
}

function moveMarkerOffHiddenPane(): void {
  const ratio = drawnSplitRatio.value;
  if (ratio === 0 && focusedPane.value !== 'content') focusPaneAndControl('content');
  else if (ratio >= 1 && focusedPane.value === 'content') focusPaneAndControl('thread');
}

export function toggleThreadPane(): void {
  if (isMobile()) {
    navigateToPane('thread');
    return;
  }
  const ratio = drawnSplitRatio.value;
  if (ratio === 0) {
    setSplitRatio(toggleThreadPaneRatio(ratio));
    focusPaneAndControl('thread');
  } else if (focusedPane.value !== 'thread') {
    focusPaneAndControl('thread');
  } else {
    setSplitRatio(toggleThreadPaneRatio(ratio));
    focusPaneAndControl('content');
  }
}

export function toggleContentPane(): void {
  if (isMobile()) {
    navigateToPane('content');
    return;
  }
  const ratio = drawnSplitRatio.value;
  if (ratio >= 1) {
    setSplitRatio(toggleContentPaneRatio(ratio, splitFitsBoth()));
    focusPaneAndControl('content');
  } else if (focusedPane.value !== 'content') {
    focusPaneAndControl('content');
  } else {
    setSplitRatio(toggleContentPaneRatio(ratio, splitFitsBoth()));
    focusPaneAndControl('thread');
  }
}

export function computeMaximizeRatio(
  pane: FocusedPane, drawn: number, stored: number, prevRatio: number | null,
): { next: number; newPrev: number | null } | null {
  const target = pane === 'content' ? 0 : 1;
  if (drawn !== target) return { next: target, newPrev: stored };
  if (stored !== target) return null;
  return { next: prevRatio ?? DEFAULT_SPLIT_RATIO, newPrev: null };
}

let prevMaximizeRatio: number | null = null;

export function toggleMaximizeFocusedPaneGroup(): void {
  if (isMobile()) return;
  const step = computeMaximizeRatio(
    focusedPane.value, drawnSplitRatio.value, splitRatio.value, prevMaximizeRatio,
  );
  if (!step) return;
  prevMaximizeRatio = step.newPrev;
  setSplitRatio(step.next);
}

export function stepThreadPaneWidth(direction: 1 | -1): void {
  if (isMobile()) return;
  const layout = document.querySelector('.split-layout') as HTMLElement | null;
  const next = computeStepRatio(
    drawnSplitRatio.value,
    layout?.offsetWidth ?? 0,
    direction * KEYBOARD_RESIZE_STEP_PX,
    splitBounds(),
  );
  if (next !== null) setSplitRatio(next);
}

export function stepThreadDrawerWidth(direction: 1 | -1): void {
  if (isMobile()) return;
  const ratio = drawnSplitRatio.value;
  if (!drawnDrawerShown.value) return;
  const row = document.querySelector('.content-row') as HTMLElement | null;
  const reserved = minThreadPanePx() + (ratio >= 1 ? 0 : minContentPanePx());
  const next = computeDrawerStepWidth(
    drawnDrawerWidth.value,
    direction * KEYBOARD_RESIZE_STEP_PX,
    minDrawerWidth(),
    (row?.offsetWidth ?? 0) - reserved,
  );
  if (next === null) return;
  threadDrawerWidth.value = next;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(next));
}

export function resetPaneLayout(): void {
  if (isMobile()) return;
  setSplitRatio(DEFAULT_SPLIT_RATIO);
  const width = Math.max(DEFAULT_DRAWER_WIDTH, minDrawerWidth());
  threadDrawerWidth.value = width;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(width));
}

export function checkPaneConsistency(): string | null {
  const view = mobileView.value;

  if (!MOBILE_VIEWS.includes(view)) {
    return `mobileView '${view}' is not a valid MobileView`;
  }

  if (threadDrawerOpen.value && view !== 'thread') {
    return `threadDrawerOpen is true but mobileView is '${view}', not 'thread'`;
  }

  return null;
}
