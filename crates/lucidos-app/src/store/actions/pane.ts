import { mobileView, setMobileView, splitRatio, threadDrawerOpen, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, type MobileView } from '../store';
import { forceCloseDrawer } from '../../components/layout/Drawer';
import { setSplitRatio, DEFAULT_SPLIT_RATIO } from '../../components/layout/splitHelpers';
import { isMobile } from '../../utils/viewport';

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

/** Toggle the thread list visibility.
 *
 *  - **Mobile**: navigates to threads pane (pane 0) so that dots, header, and
 *    pane content all update atomically via the `mobileView` signal.
 *  - **Desktop**: toggles `threadDrawerOpen` (drawer overlay in split layout).
 *
 *  This is the ONLY correct way for UI elements to show/hide the thread list.
 *  Never toggle `threadDrawerOpen` directly on mobile — it bypasses mobileView
 *  and causes the dot indicator to desync from what the user sees. */
export function toggleThreads() {
  if (isMobile()) {
    navigateToPane('threads');
  } else {
    threadDrawerOpen.value = !threadDrawerOpen.value;
  }
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
