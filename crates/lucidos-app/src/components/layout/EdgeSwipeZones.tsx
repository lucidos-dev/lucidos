/** The pair of host-owned transparent strips at the left and right screen edges.
 *
 *  Absolutely positioned against the nearest positioned ancestor and sized only
 *  under the mobile media query (`.edge-swipe-zone` in `styles/mobile.css`), so
 *  mount them on the mobile layout only. Wherever they are mounted they do one
 *  low-level job: be the topmost thing at the screen edge, ABOVE any app iframe,
 *  so a touch there reaches the host document instead of the frame.
 *
 *  That job pays off twice:
 *
 *   - Inside a swipe pane, an iframe captures every touch it covers, so these
 *     strips are the only place a pane swipe over an app can begin.
 *   - Anywhere, they are what lets `MobileSwipeContainer`'s touchstart handler
 *     see an edge touch at all and `preventDefault()` it, which is the only way
 *     to suppress WebKit's native back/forward gesture in the standalone iOS PWA
 *     (no CSS opt-out for it exists). See `shouldSuppressEdgeNavigation`.
 *
 *  The second reason is why they are also mounted inside a pseudo-fullscreen app
 *  overlay, which covers the panes' own strips. Being mounted does NOT imply a
 *  pane swipe is available: `shouldStartPaneSwipe` turns that off while an app is
 *  fullscreen, and the strips stay purely for the suppression.
 *
 *  Their widths are mirrored by `EDGE_NAV_GUARD_LEFT_PX` /
 *  `EDGE_NAV_GUARD_RIGHT_PX`: change one side and change its constant, or a
 *  touch can land on a strip that the suppression decision does not cover. */
export function EdgeSwipeZones() {
  return (
    <>
      <div class="edge-swipe-zone edge-swipe-left" />
      <div class="edge-swipe-zone edge-swipe-right" />
    </>
  );
}
