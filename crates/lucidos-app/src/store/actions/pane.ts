import {
  mobileView, setMobileView, splitRatio, threadDrawerOpen, threadDrawerWidth,
  DEFAULT_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY,
  MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, type MobileView,
  focusedPane, type FocusedPane,
} from '../store';
import { forceCloseDrawer } from '../../components/layout/Drawer';
import {
  setSplitRatio, DEFAULT_SPLIT_RATIO, cancelPendingSnap,
  computeStepRatio, computeDrawerStepWidth,
  toggleThreadPaneRatio, toggleContentPaneRatio,
  KEYBOARD_RESIZE_STEP_PX, MIN_THREAD_PANE_PX, MIN_CONTENT_PANE_PX,
} from '../../components/layout/splitHelpers';
import { isMobile } from '../../utils/viewport';
import { focusPaneMainControl } from '../../components/layout/paneFocus';

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
  forceCloseDrawer();
  // Drawer can only be open on the thread pane (per the consistency invariant
  // in checkPaneConsistency). Keep it open when navigating *to* 'thread' so the
  // user lands on the compose view with the drawer still visible.
  if (view !== 'thread') threadDrawerOpen.value = false;
  setMobileView(view);
}

/** Make the content pane visible after opening something into it.
 *  Mobile: swipe to the content pane. Desktop: expand the split if collapsed.
 *  Always call this after setting `panelOverlay.value` so a click on a content
 *  link is never silently absorbed when the pane is closed. */
export function revealContentPane() {
  if (isMobile()) {
    navigateToPane('content');
  } else if (splitRatio.value >= 1) {
    setSplitRatio(DEFAULT_SPLIT_RATIO);
  }
}

/** Move desktop pane focus. No-op on mobile, where panes are navigated, not
 *  focused. Called on a pointer-down inside a pane so the focus indicator always
 *  tracks where the user is actually working, and by the two-stage toggles. */
export function focusPane(pane: FocusedPane): void {
  if (isMobile()) return;
  focusedPane.value = pane;
}

/** Show or hide the thread list (drawer). A plain visibility toggle — unlike the
 *  pane toggles below it does NOT move pane focus: opening the drawer reveals it
 *  without stealing keyboard focus, so the icon is purely show/hide.
 *
 *  - **Mobile**: navigates to the threads pane (pane 0) so dots, header, and
 *    pane content all update atomically via the `mobileView` signal.
 *  - **Desktop**: flips `threadDrawerOpen`. When hiding a drawer that currently
 *    holds focus, focus falls back to the thread pane it was overlaying — a
 *    hidden pane must never stay the focused pane or the focus wash strands on an
 *    invisible region. That fix-up is signal-only; no DOM focus is moved, so the
 *    toggle never lands keyboard focus on a pane the way the old focus stage did.
 *
 *  This is the ONLY correct way for UI elements to show/hide the thread list.
 *  Never toggle `threadDrawerOpen` directly on mobile — it bypasses mobileView
 *  and causes the dot indicator to desync from what the user sees. */
export function toggleThreads() {
  if (isMobile()) {
    navigateToPane('threads');
    return;
  }
  threadDrawerOpen.value = !threadDrawerOpen.value;
  if (!threadDrawerOpen.value && focusedPane.value === 'drawer') {
    focusedPane.value = 'thread';
  }
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
  if (splitRatio.value === 0) {
    setSplitRatio(toggleThreadPaneRatio(splitRatio.value)); // expand
    focusPaneAndControl('thread');
  } else if (focusedPane.value !== 'thread') {
    focusPaneAndControl('thread');
  } else {
    setSplitRatio(toggleThreadPaneRatio(splitRatio.value)); // collapse
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
  if (splitRatio.value >= 1) {
    setSplitRatio(toggleContentPaneRatio(splitRatio.value)); // expand
    focusPaneAndControl('content');
  } else if (focusedPane.value !== 'content') {
    focusPaneAndControl('content');
  } else {
    setSplitRatio(toggleContentPaneRatio(splitRatio.value)); // collapse
    focusPaneAndControl('thread');
  }
}

/** Keyboard resize: move the split divider by one KEYBOARD_RESIZE_STEP_PX step.
 *  +1 widens the thread pane, -1 narrows it. Unlike a divider drag there is no
 *  deferred snap — the step clamps to the pane minimums immediately and never
 *  collapses a pane (collapse belongs to the toggles). Desktop-only. */
export function stepThreadPaneWidth(direction: 1 | -1): void {
  if (isMobile()) return;
  const layout = document.querySelector('.split-layout') as HTMLElement | null;
  const next = computeStepRatio(
    splitRatio.value,
    layout?.offsetWidth ?? 0,
    direction * KEYBOARD_RESIZE_STEP_PX,
  );
  if (next !== null) setSplitRatio(next);
}

/** Keyboard resize for the thread drawer: ±KEYBOARD_RESIZE_STEP_PX, clamped to
 *  [MIN_DRAWER_WIDTH, row width minus the visible split panes' minimums].
 *  No-op while the drawer is hidden (closed, or thread pane collapsed). */
export function stepThreadDrawerWidth(direction: 1 | -1): void {
  if (isMobile()) return;
  if (!threadDrawerOpen.value || splitRatio.value <= 0) return;
  const row = document.querySelector('.content-row') as HTMLElement | null;
  const reserved = MIN_THREAD_PANE_PX + (splitRatio.value >= 1 ? 0 : MIN_CONTENT_PANE_PX);
  const next = computeDrawerStepWidth(
    threadDrawerWidth.value,
    direction * KEYBOARD_RESIZE_STEP_PX,
    (row?.offsetWidth ?? 0) - reserved,
  );
  if (next === null) return;
  // A snap still pending from a drag release must not overwrite the explicit step.
  cancelPendingSnap();
  threadDrawerWidth.value = next;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(next));
}

/** Restore the default desktop layout: split at DEFAULT_SPLIT_RATIO, thread
 *  drawer at its default width. The drawer's open/closed state is the user's
 *  choice and stays untouched. */
export function resetPaneLayout(): void {
  if (isMobile()) return;
  setSplitRatio(DEFAULT_SPLIT_RATIO);
  threadDrawerWidth.value = DEFAULT_DRAWER_WIDTH;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(DEFAULT_DRAWER_WIDTH));
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
