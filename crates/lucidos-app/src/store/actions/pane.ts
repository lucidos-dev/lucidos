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

/** Set the focused pane AND move real DOM focus into it. Used by the keyboard
 *  toggles/shortcuts so "focus pane" actually lands focus on the pane's main
 *  control (and Tab then cycles within it). The pointer-down path uses the
 *  signal-only `focusPane` instead — a click already places its own focus. */
function focusPaneAndControl(pane: FocusedPane): void {
  focusedPane.value = pane;
  focusPaneMainControl(pane);
}

/** Clamp a pane index + delta to valid bounds and return the target MobileView.
 *  Returns null if the result is the same as the current pane. */
export function resolveSwipePane(delta: number): MobileView | null {
  const currentIndex = PANE_INDEX[mobileView.value];
  const newIndex = Math.max(0, Math.min(PANE_COUNT - 1, currentIndex + delta));
  return newIndex !== currentIndex ? MOBILE_VIEWS[newIndex] : null;
}

/** Commit a pane swipe: move to the neighbouring pane, when there is one. */
export function swipeToNeighbourPane(paneDelta: -1 | 1): void {
  const target = resolveSwipePane(paneDelta);
  if (target) navigateToPane(target);
}

/** Single entry point for mobile pane switches.
 *  Atomically closes drawers and updates the signal, preventing
 *  intermediate states where a drawer is open on the wrong pane.
 *
 *  Does NOT push browser history entries. Pushing pushState entries
 *  enables iOS Safari's native back gesture which shows a blank/black
 *  snapshot during the swipe animation (the "previous page" is the same
 *  SPA so there's nothing meaningful to show). Instead, pane navigation
 *  is handled entirely in-app via MobileSwipeContainer's touch handler
 *  and the edge swipe zones that sit above iframes. */
export function navigateToPane(view: MobileView) {
  // Stamped BEFORE the signal write, so the span covers the render this call is
  // about to trigger. `MobileSwipeContainer` fires it on the next paint. A
  // re-tap of the current pane still stamps and fires nothing, so its mark is
  // replaced by the next navigation.
  markNavigationStart('pane', view, performance.now());
  forceCloseDrawer();
  // Drawer can only be open on the thread pane (per the consistency invariant
  // in checkPaneConsistency). Keep it open when navigating *to* 'thread' so the
  // user lands on the compose view with the drawer still visible.
  if (view !== 'thread') threadDrawerOpen.value = false;
  setMobileView(view);
}

/** Make the content pane visible AND the focused pane after opening something
 *  into it. Mobile: swipe to the content pane. Desktop: activate the Content
 *  pane group (`focusedPane = 'content'`) so the per-pane Tab trap routes
 *  keyboard tabbing into the freshly-navigated view, and expand the split if
 *  collapsed. Navigation that lands content (Search Everywhere, a menu switch, a
 *  deep link) never lands DOM focus in the content pane, so without setting the
 *  focused pane group `focusedPane` stays 'thread' and Tab cycles the wrong
 *  pane. `reconcilePaneFocus` then pulls DOM keyboard focus into the content pane
 *  (when it isn't already there) so the marker, native scroll keys, and shortcuts
 *  all point at the same pane — the focused-pane marker is kept in sync with real
 *  focus. Always call this after setting `panelOverlay.value` so a click on a content
 *  link is never silently absorbed when the pane is closed. The one exception
 *  to the marker move is an agent navigation while the reader types
 *  (`holdFocusedPaneWhileTyping`). That one also leaves a yielded Canvas
 *  collapsed. */
export function revealContentPane() {
  if (isMobile()) {
    // `navigateToPane` stamps the mark for this one. Mobile reveals ARE pane
    // swaps, and a second stamp would only overwrite it at the same instant.
    navigateToPane('content');
    return;
  }
  // `ContentPane`'s view-key effect fires this, and the key it must see is
  // already in the store: every caller sets the overlay or the menu item BEFORE
  // revealing. Naming it here is what makes a no-op reveal harmless. Expanding
  // a collapsed pane onto the view it already holds changes no key, so no
  // effect runs. An unrelated view change soon after then refuses the mark,
  // rather than reporting itself from this reveal's start.
  markNavigationStart(
    'content',
    'content-pane',
    performance.now(),
    contentViewKey(activeMenuItem.value, panelOverlay.value, settingsSubview.value),
  );
  const typing = takeTypingHold();
  if (!typing) focusedPane.value = 'content';
  // A held reveal never swaps the panes, which would hide the field being edited.
  if (drawnSplitRatio.value >= 1 && (!typing || splitFitsBoth())) {
    setSplitRatio(contentShownRatio(splitFitsBoth()));
  }
  reconcilePaneFocus('content');
}

let typingHold: Element | null = null;

/** Keep the focused-pane marker where the reader is typing, for the reveal
 *  this agent navigation is about to make. The reveal's reconcile never pulls
 *  focus off a field being edited. Moving the marker alone would split it from
 *  focus, and the reader's next Tab would leave the prompt. A reader's own
 *  navigation takes no hold, so their shortcut from the prompt still moves Tab. */
export function holdFocusedPaneWhileTyping(): void {
  const active = document.activeElement;
  const pane = active && isTextInput(active) ? paneHolding(active) : null;
  typingHold = pane !== null && pane !== 'content' ? active : null;
}

/** End the hold once its navigation settles, revealed or not. */
export function releaseTypingHold(): void {
  typingHold = null;
}

/** Spend the hold: true when the reader is still typing in the held field. */
function takeTypingHold(): boolean {
  const hold = typingHold;
  typingHold = null;
  return !!hold && hold === document.activeElement;
}

/** Mirror of `revealContentPane` for navigation that lands on a THREAD — an
 *  existing thread (focusThread), the empty compose view (unfocusThread), or a
 *  brand-new thread spawned from another pane (sendMessage's raw-new-thread
 *  path, e.g. the new-app form in the content pane). Mobile: swipe to the thread
 *  (conversation) pane so the focused thread/compose is visible. Desktop, in two
 *  halves:
 *
 *  - **Re-expand a collapsed split** (`splitRatio <= 0`, the Content pane group
 *    maximized), mirroring `revealContentPane`'s own collapsed branch. Without
 *    it the navigation lands behind a zero-width pane: the focused thread does
 *    change, nothing on screen does, and the click reads as a no-op. That is
 *    what a thread link clicked from the content pane hit (a change toast, a
 *    Search Everywhere result, a New-thread button); mobile never did, because
 *    panes there are navigated rather than collapsed.
 *  - **Re-activate the Threads pane group** (`focusedPane = 'thread'`), then let
 *    `reconcilePaneFocus` pull DOM keyboard focus into the pane (when it isn't
 *    already there) so the marker and real focus stay in sync. Normally ONLY
 *    from the cross-group case (arriving from the Content pane group), so an
 *    existing 'drawer'/'thread' focus is left alone and drawer ↑/↓ browsing
 *    isn't disturbed. A collapse we just undid is the exception: the pane was
 *    invisible (and a collapsed thread pane hides the drawer with it), so a
 *    marker pointing into it is stale by construction and the reveal owns it.
 *
 *  Both thread-side callers take a `revealPane: false` opt-out for a focus
 *  change that is bookkeeping rather than navigation (stale-pointer cleanup, the
 *  post-archive hand-off), which must leave the visible pane alone.
 *  Unlike a raw `navigateToPane('thread')` this keeps the desktop half (focused
 *  pane group) honest — the reason a bare mobile-gated navigate is wrong here, the
 *  same way it is for content (see frontend.md). */
export function revealThreadPane(): void {
  if (isMobile()) {
    // Stamped by `navigateToPane`, for the reason `revealContentPane` states.
    navigateToPane('thread');
    return;
  }
  // NO stamp on desktop, deliberately. Landing on a thread changes no content
  // view key and mounts no swipe container, so neither fire point would ever
  // take it: the mark would sit until some later content navigation read the
  // idle gap as its own span. `thread-render` already measures this edge, and
  // its `warm` flag is what marks thread-to-thread back and forth.
  const wasCollapsed = drawnSplitRatio.value <= 0;
  if (wasCollapsed) setSplitRatio(DEFAULT_SPLIT_RATIO);
  if (wasCollapsed || focusedPane.value === 'content') {
    focusedPane.value = 'thread';
    reconcilePaneFocus('thread');
  }
}

/** Move desktop pane focus. No-op on mobile, where panes are navigated, not
 *  focused. Called on a pointer-down inside a pane BODY (SplitLayout panes /
 *  thread drawer) and on CLICK inside a header region — the header doubles as the
 *  Tauri window-drag region, so firing focus on click (not pointerdown) keeps a
 *  window drag from changing the focused pane (a native drag suppresses the
 *  click). Either way the focused-pane marker tracks where the user is working, and
 *  `reconcilePaneFocus` pulls DOM keyboard focus into the pane when the click
 *  didn't land it there (e.g. a header-region click, or a click on non-focusable
 *  pane chrome) so native scroll keys act on the pane the marker points at. A
 *  click that lands focus on a control inside the pane is left untouched. */
export function focusPane(pane: FocusedPane): void {
  if (isMobile()) return;
  focusedPane.value = pane;
  reconcilePaneFocus(pane);
}

/** Why a side pane's toggle is disabled, and what a shortcut that cannot show
 *  it says. The window cannot hold the pane beside the Conversation's floor
 *  even with every other side pane yielded. */
export const THREAD_LIST_TOO_NARROW = 'The window is too narrow for the thread list';
export const DASHBOARD_TOO_NARROW = 'The window is too narrow for the dashboard';

/** Hide the thread drawer for real, and keep the focused pane off a hidden one:
 *  a hidden pane must never stay the focused pane or the marker strands on an
 *  invisible region. `reconcilePaneFocus` moves DOM keyboard focus (which was in
 *  the now-hidden drawer) into the thread pane so the two stay in sync. */
function closeThreadDrawer(): void {
  threadDrawerOpen.value = false;
  if (focusedPane.value === 'drawer') {
    focusedPane.value = 'thread';
    reconcilePaneFocus('thread');
  }
}

/** Show or hide the thread list (drawer). A plain visibility toggle — unlike the
 *  pane toggles below it does NOT move pane focus: opening the drawer reveals it
 *  without stealing keyboard focus, so the icon is purely show/hide.
 *
 *  - **Mobile**: navigates to the threads pane (pane 0) so dots, header, and
 *    pane content all update atomically via the `mobileView` signal.
 *  - **Desktop**: reads what is DRAWN. A drawn drawer closes (`closeThreadDrawer`).
 *    Anything else shows (`showThreadList`), which a window too narrow for the
 *    drawer cannot do, so the toggle is disabled there.
 *
 *  This is the ONLY correct way for UI elements to show/hide the thread list.
 *  Never toggle `threadDrawerOpen` directly on mobile — it bypasses mobileView
 *  and causes the dot indicator to desync from what the user sees. */
export function toggleThreads() {
  if (isMobile()) {
    navigateToPane('threads');
    return;
  }
  if (drawnDrawerShown.value) closeThreadDrawer();
  else showThreadList();
}

export type DrawerShortcutAction = 'open-focus' | 'focus' | 'close';

/** Pure decision for the focus-aware drawer shortcut (⌘⇧1). `visible` is what
 *  the layout draws, which a yield and a collapsed Conversation both clear.
 *  Hidden → open + focus; visible but unfocused → focus; visible + focused →
 *  close. Mirrors the three-stage toggleThreadPane/toggleContentPane shape. */
export function computeDrawerShortcutAction(
  visible: boolean, drawerFocused: boolean,
): DrawerShortcutAction {
  if (!visible) return 'open-focus';
  if (!drawerFocused) return 'focus';
  return 'close';
}

/** ⌘⇧1: the focus-aware three-stage drawer toggle (keyboard path only — the
 *  drawer icon stays a pure show/hide via `toggleThreads`). Moves real DOM focus
 *  into the drawer so its existing ↑/↓/Enter list-nav is reachable, re-expanding
 *  a collapsed Conversation side first. Returns true when the drawer is now
 *  focused (the caller seeds the keyboard highlight), false when it closed.
 *  Desktop-only; on mobile it navigates to the threads pane. */
export function focusOrToggleThreadDrawer(): boolean {
  if (isMobile()) { navigateToPane('threads'); return false; }
  const action = computeDrawerShortcutAction(drawnDrawerShown.value, focusedPane.value === 'drawer');
  if (action === 'close') {
    // The drawer is open + focused here, so toggleThreads closes it and drops
    // focus back to the thread pane — the single source of that close behavior.
    toggleThreads();
    return false;
  }
  if (action === 'open-focus' && !showThreadList()) return false;
  focusPaneAndControl('drawer');
  return true;
}

/** Whether the window can show the thread list, saying so when it cannot. */
function threadListHasRoom(): boolean {
  if (threadListCanShow()) return true;
  showToast(THREAD_LIST_TOO_NARROW, 'info');
  return false;
}

/** Make the thread list visible without moving focus. Mobile navigates to the
 *  threads pane. Desktop opens the drawer and re-expands a collapsed
 *  Conversation side, which would otherwise keep the open drawer hidden. The
 *  rail yields to it, drawn-only. A window too narrow for the drawer says so,
 *  and returns false. */
export function showThreadList(): boolean {
  if (isMobile()) { navigateToPane('threads'); return true; }
  if (!threadListHasRoom()) return false;
  threadDrawerOpen.value = true;
  if (drawnSplitRatio.value <= 0) setSplitRatio(DEFAULT_SPLIT_RATIO);
  return true;
}

/** Whether the thread list is on screen: the threads pane on mobile, and the
 *  drawer as the layout draws it on desktop. */
export function threadListShown(): boolean {
  if (isMobile()) return mobileView.value === 'threads';
  return drawnDrawerShown.value;
}

/** Show the dashboard's rail. The drawer outranks the rail, so a rail that
 *  does not fit beside the drawn drawer closes the drawer for real, as the one
 *  swap. A window too narrow for the rail even alone says so. */
export function showDashboardRail(): void {
  if (!dashboardRailCanShow()) {
    showToast(DASHBOARD_TOO_NARROW, 'info');
    return;
  }
  if (!dashboardRailFitsBesideDrawer()) closeThreadDrawer();
  setDashboardRailShown(true);
}

/** The rail's toggle reads what is DRAWN: a drawn rail hides, and anything else
 *  shows, which a window too narrow for the rail cannot do. */
export function toggleDashboardRail(): void {
  if (drawnRailShown.value) setDashboardRailShown(false);
  else showDashboardRail();
}

/** The filter shortcut. A hidden thread list comes back with its filters up,
 *  and a shown one toggles them. The filter shapes the Folders grouping only,
 *  so under Ongoing the shortcut switches to Folders and opens it. The drawer
 *  takes focus, so Tab walks what it now shows. Showing the filters puts
 *  thread search away, as the reverse does. */
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

/** The thread search shortcut: show the thread list, open its search bar and
 *  put the caret in it once the bar has rendered. */
export function startThreadSearch(): void {
  if (!showThreadList()) return;
  openThreadSearch();
  if (!isMobile()) focusedPane.value = 'drawer';
  requestAnimationFrame(() => focusThreadSearchInput());
}

/** The divider double-click: a collapsed split (either side) returns to the
 *  default ratio, and an open one collapses the Conversation side. The marker
 *  leaves a pane that collapses, as `toggleThreadPane` does. A collapsed pane
 *  is `visibility: hidden`, so a marker left on it turns every Tab dead. */
export function toggleSplitFromDivider(): void {
  const ratio = drawnSplitRatio.value;
  if (ratio === 0 || ratio >= 1) {
    setSplitRatio(ratio === 0 ? DEFAULT_SPLIT_RATIO : contentShownRatio(splitFitsBoth()));
    return;
  }
  setSplitRatio(0);
  if (focusedPane.value !== 'content') focusPaneAndControl('content');
}

/** Collapse/expand the thread pane with two-stage focus → hide. Desktop: a
 *  collapsed pane expands to DEFAULT_SPLIT_RATIO and takes focus; a
 *  visible-but-unfocused pane just takes focus; a focused, visible pane
 *  collapses to 0 (the drawer hides with it and restores on expand, see
 *  SplitLayout's drawerVisible) and focus falls to the content pane that fills
 *  the gap. Mobile: swipe to the thread pane instead — panes there are
 *  navigated, not collapsed. */
export function toggleThreadPane(): void {
  if (isMobile()) {
    navigateToPane('thread');
    return;
  }
  const ratio = drawnSplitRatio.value;
  if (ratio === 0) {
    setSplitRatio(toggleThreadPaneRatio(ratio)); // expand
    focusPaneAndControl('thread');
  } else if (focusedPane.value !== 'thread') {
    focusPaneAndControl('thread');
  } else {
    setSplitRatio(toggleThreadPaneRatio(ratio)); // collapse
    focusPaneAndControl('content');
  }
}

/** Collapse/expand the content pane. Desktop mirror of toggleThreadPane: a
 *  collapsed pane (ratio >= 1) expands and takes focus; a visible-but-unfocused
 *  pane just takes focus; a focused, visible pane collapses and focus falls to
 *  the thread pane that fills the gap. Mobile: swipe to the content pane. */
export function toggleContentPane(): void {
  if (isMobile()) {
    navigateToPane('content');
    return;
  }
  const ratio = drawnSplitRatio.value;
  if (ratio >= 1) {
    setSplitRatio(toggleContentPaneRatio(ratio, splitFitsBoth())); // expand
    focusPaneAndControl('content');
  } else if (focusedPane.value !== 'content') {
    focusPaneAndControl('content');
  } else {
    setSplitRatio(toggleContentPaneRatio(ratio, splitFitsBoth())); // collapse
    focusPaneAndControl('thread');
  }
}

/** Pure logic for ⌘⇧↵ (maximize the focused *pane group*, toggle). The Threads
 *  pane group (drawer/thread focused) fills via `splitRatio` 1 — the drawer rides
 *  `splitRatio > 0` so it stays open; the Content pane group (content focused)
 *  fills via 0. When already maximized for the focused group, restore the
 *  remembered pre-maximize ratio (or the default if none was remembered, e.g.
 *  after a reload). Returns the next ratio and the ratio to remember next (null
 *  once restored). Pure — exported for unit testing.
 *
 *  `drawn` decides what is on screen, `stored` is what a restore returns to. A
 *  Conversation drawn full only because the Canvas yielded is already
 *  maximized, and there is no split to restore: null, a no-op. */
export function computeMaximizeRatio(
  pane: FocusedPane, drawn: number, stored: number, prevRatio: number | null,
): { next: number; newPrev: number | null } | null {
  const target = pane === 'content' ? 0 : 1;
  if (drawn !== target) return { next: target, newPrev: stored };
  if (stored !== target) return null;
  return { next: prevRatio ?? DEFAULT_SPLIT_RATIO, newPrev: null };
}

/** Remembers the pre-maximize split ratio so the second ⌘⇧↵ restores it.
 *  In-memory only — a reload discards it (the maximize itself persists via
 *  `splitRatio`), so a post-reload restore falls back to DEFAULT_SPLIT_RATIO. */
let prevMaximizeRatio: number | null = null;

/** ⌘⇧↵: toggle the focused *pane group* full-width, restoring the previous split
 *  on the second press. Desktop-only — `focusedPane` doesn't exist on mobile,
 *  where panes are navigated, not focused. */
export function toggleMaximizeFocusedPaneGroup(): void {
  if (isMobile()) return;
  const step = computeMaximizeRatio(
    focusedPane.value, drawnSplitRatio.value, splitRatio.value, prevMaximizeRatio,
  );
  if (!step) return;
  prevMaximizeRatio = step.newPrev;
  setSplitRatio(step.next);
}

/** Keyboard resize: move the split divider by one KEYBOARD_RESIZE_STEP_PX step.
 *  +1 widens the thread pane, -1 narrows it. Clamps to the same pane minimums a
 *  drag does and never collapses a pane (collapse belongs to the toggles).
 *  Desktop-only. */
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

/** Keyboard resize for the thread drawer: ±KEYBOARD_RESIZE_STEP_PX, clamped to
 *  [minDrawerWidth(), row width minus the visible split panes' minimums].
 *  Steps from the width drawn, which a smaller window holds below the stored one.
 *  No-op while the drawer is hidden (closed, yielded, or thread pane collapsed). */
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

/** Restore the default desktop layout: split at DEFAULT_SPLIT_RATIO, thread
 *  drawer at its default width. The drawer's open/closed state is the user's
 *  choice and stays untouched.
 *
 *  The default is a constant while the drawer's floor is derived, and the floor
 *  wins from a 16px root up (312px against the default's 300), so a bare reset
 *  would put the drawer straight back under its own floor and overflow the
 *  header until something else clamped it. The constant still leads below 100%
 *  ui-scale, which is the only range where a reset lands on it. */
export function resetPaneLayout(): void {
  if (isMobile()) return;
  setSplitRatio(DEFAULT_SPLIT_RATIO);
  const width = Math.max(DEFAULT_DRAWER_WIDTH, minDrawerWidth());
  threadDrawerWidth.value = width;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(width));
}

/** Check whether the current pane state is consistent.
 *  Returns null if consistent, or a description of the inconsistency. */
export function checkPaneConsistency(): string | null {
  const view = mobileView.value;

  // mobileView must be a valid value
  if (!MOBILE_VIEWS.includes(view)) {
    return `mobileView '${view}' is not a valid MobileView`;
  }

  // Thread drawer can only be open on the thread pane
  if (threadDrawerOpen.value && view !== 'thread') {
    return `threadDrawerOpen is true but mobileView is '${view}', not 'thread'`;
  }

  return null;
}
