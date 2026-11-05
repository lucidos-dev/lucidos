/**
 * How narrow each desktop pane is allowed to get: the thread drawer, the
 * Conversation pane, the Canvas pane, and the rail that holds the dashboard.
 *
 * One module for all four because they are read TOGETHER. A divider drag is
 * clamped to what the panes on either side of it need, and a container too
 * narrow to hold every minimum at once has to degrade deterministically
 * (`computeDrawnLayout` in components/layout/splitHelpers.ts), so a reader weighing
 * one floor is always weighing it against the others. They used to live in two
 * files, one of them a 2000-line store.
 *
 * All are DERIVED from the root font size rather than written as px
 * constants, because everything they size is rem-authored: a constant is only
 * right at the one UI scale it was measured at. The drawer's floor learned that
 * first (its `260` was computed "at 16px/rem" for a header row that no longer
 * existed); the two pane floors follow, and are byte-identical to the `300` and
 * `360` they replace at a 16px root.
 *
 * The cost of deriving them is that they no longer fit every window. New
 * thread and Home lead the Conversation row. With both, the three pane floors
 * sum to 1178px at 100% ui-scale, 1392 at 125%, 1607 at 150% and 1821 at 175%.
 * A shown rail adds 364, 455, 546 and 637.
 *
 * So even without the rail, from 112.5% on a 1280px screen the split cannot
 * hold both pane floors, and the Canvas yields (`computeDrawnSplitRatio` in
 * components/layout/splitHelpers.ts). That case is load-bearing, not a corner.
 * The rail then yields before the drawer, and the Conversation never yields
 * (`computeDrawnLayout`, ADR 0498). Those sums hold on every desktop client,
 * because neither derived floor varies by build (see `computeMinDrawerWidth`
 * and `computeMinThreadPaneWidth`).
 *
 * Deliberately holds no signal and imports nothing from `store.ts`: the store's
 * own module init reads `minDrawerWidth()` to clamp the persisted drawer width,
 * so an import back the other way would be a cycle resolved at boot.
 */
import { getRemPx } from '../utils/dom';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, DASHBOARD_MIN_COLUMNS, DASHBOARD_PAD_X_REM } from '../utils/dashboardGeometry';

/** One control at an end of the threads header's row, in rem: a button and its
 *  gap. Each end holds two. The leading end is the drawer toggle resting over
 *  the row, then Filter; the trailing end is Grouping and Search. Mirrors
 *  `.threads-header` in styles/panels/shell.css (`--header-icon-box` and
 *  `--pane-header-gap`). */
const DRAWER_ROW_BUTTON_REM = 2.25 + 0.25;
/** The row's own trailing padding, in rem: `.threads-header`'s `0.5rem`. */
const DRAWER_ROW_PAD_REM = 0.5;
/** Fallback for the traffic-lights reserve if the property cannot be read (it
 *  is declared inside the desktop media query, so the phone layout reports
 *  nothing). Kept in step with `--titlebar-lights-reserve` in shell.css, which
 *  no longer states it directly: it is `--titlebar-lights-x` (10px, stamped by
 *  the shell from the constant it places the cluster with) plus the cluster's
 *  measured 60px plus an equal 10px gap, the two halves of the slack that
 *  centre the lights in the room the row keeps clear. The SUM is what this
 *  restates, and it has never changed, so the floor this feeds is the same one
 *  it always was. */
const TITLEBAR_LIGHTS_RESERVE_PX = 80;

/** One END of the Conversation header's row, in rem, excluding whatever leads
 *  it: the drawer toggle's box. Mirrors `.thread-toggle-slot` in
 *  styles/panels/shell.css (`--header-icon-box`). */
const THREAD_ROW_SIDE_REM = 2.25;
/** What each control leading the row adds to that end: its box and one gap.
 *  New thread leads it, and Home beside it. Mirrors the `:has(...)` reserves
 *  on `.pane-header-brand`. */
const THREAD_ROW_LEAD_CONTROL_REM = 2.25 + 0.25;
/** The row's own leading padding, in rem. `--header-lead-inset`'s web value, and
 *  the floor UNDER the floor: whatever lead the row is sized around, an end is
 *  never narrower than the padding every build has. */
const THREAD_ROW_PAD_REM = 0.5;
/** The centred brand cluster at its natural width, in rem: two chevrons and the
 *  mark's tap target, touching. Mirrors `--desktop-nav-min-span`
 *  (`2 * --header-icon-box + --header-mark-tap`), which the clamp on
 *  `.pane-header-brand-label` floors the box at. */
const THREAD_ROW_CLUSTER_REM = 2 * 2.25 + 2.1;
/** The Canvas pane's floor. 360px at a 16px root. It is a constant where the
 *  other two are derived. Its row needs
 *  `2 * --content-side-reserve + --desktop-nav-min-span`, which is 22.1rem, so
 *  the title cluster rests on its clamp's reserve arm down to this width. Pinned
 *  in the suite. The docked dashboard rail toggle widens the reserve by a box, and
 *  then the cluster reaches its min-span arm near this floor. There
 *  `useHeaderActionCollapse` folds the actions into ⋯ to keep them clear. */
const MIN_CONTENT_PANE_REM = 22.5;

/** Floor for the thread drawer's width, from the root font size and the lead its
 *  header row is sized around. Pure, so the arithmetic is testable without a
 *  layout engine; the DOM read lives in `minDrawerWidth` below.
 *
 *  The row centres its title on the PANE (`.threads-header-title` in
 *  styles/panels/shell.css). So the title clears the WIDER of the row's two
 *  ends on BOTH sides, and the floor is symmetric: `2 * side`. The leading end
 *  is the lead plus the toggle and Filter; the trailing end is the padding plus
 *  Grouping and Search. The floor keeps no room for the title: a title that
 *  does not fit hides (`ThreadsPaneTitle`), so the controls are what must fit.
 *
 *  ONE floor for every desktop client: its only caller passes the traffic-lights
 *  reserve as the lead, on the web build that has no lights too (ADR 0058). That
 *  is deliberate, not an oversight: a workspace opened in the browser and the
 *  same workspace in the packaged app must stop the drawer at the same width,
 *  and the packaged build's row is the wider of the two (its controls reach up
 *  into the reclaimed title-bar band, so it starts after the fixed reserve that
 *  clears the lights, where the web row starts after its own 0.5rem). Taking the
 *  wider one is what makes the floor a floor on both. The web row still LAYS OUT
 *  at 0.5rem, so what it gains is title room it is not obliged to use; nothing
 *  there paints against a light that is not there.
 *
 *  The reserve is PX (the lights are OS chrome and do not scale with our root
 *  font size) while everything else here is rem. Both ends hold two buttons,
 *  and the reserve outweighs the trailing end's 0.5rem padding. So the leading
 *  end sets the floor at every scale a user can pick. */
export function computeMinDrawerWidth(remPx: number, leadPx: number): number {
  const leadEndPx = leadPx + 2 * DRAWER_ROW_BUTTON_REM * remPx;
  const trailEndPx = (DRAWER_ROW_PAD_REM + 2 * DRAWER_ROW_BUTTON_REM) * remPx;
  return Math.ceil(2 * Math.max(leadEndPx, trailEndPx));
}

/** The lead both derived floors are sized around, read from CSS rather than
 *  restated, with the literal as the fallback. The property is declared inside
 *  the desktop media query, so the phone layout (or a test harness with no
 *  layout engine) reports nothing. */
function titlebarLightsReservePx(): number {
  const reserve = parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue('--titlebar-lights-reserve'),
  );
  return Number.isFinite(reserve) ? reserve : TITLEBAR_LIGHTS_RESERVE_PX;
}

/** Floor for the thread drawer's width, at the CURRENT root font size. Reads no
 *  build attribute: `data-titlebar-overlay` decides how the row is LAID OUT, not
 *  how narrow the drawer may get. */
export function minDrawerWidth(): number {
  return computeMinDrawerWidth(getRemPx(), titlebarLightsReservePx());
}

/** Floor for the Conversation pane, from the root font size and the lead its
 *  header row is sized around. Pure, like `computeMinDrawerWidth` above, and the
 *  same shape for the same reason. The brand cluster is centred on the PANE
 *  (`.pane-header-brand-label`), so it clears the WIDER of the row's two ends on
 *  BOTH sides. That is why the lead is counted twice.
 *
 *  This is the width at which the cluster, held at its natural span, exactly
 *  meets the drawer toggle. It was a flat `18.75rem` before. On the packaged
 *  build that let a drag rest where the back chevron sat ON the toggle: the
 *  row's leading end there is the traffic-lights reserve plus a button, which no
 *  constant knew about.
 *
 *  ONE floor for every desktop client, exactly as the drawer's is: its caller
 *  passes the reserve on the web build too (ADR 0058). The trailing end is the
 *  narrower one on both builds, because the actions fold into the ⋯ menu long
 *  before this width (`useHeaderActionCollapse`). So the symmetric lead is what
 *  governs. */
export function computeMinThreadPaneWidth(remPx: number, leadPx: number, leadControls: number): number {
  const sideRem = THREAD_ROW_SIDE_REM + leadControls * THREAD_ROW_LEAD_CONTROL_REM;
  const sidePx = Math.max(leadPx, THREAD_ROW_PAD_REM * remPx) + sideRem * remPx;
  return Math.ceil(2 * sidePx + THREAD_ROW_CLUSTER_REM * remPx);
}

/** The Conversation header's row on desktop. */
export const THREAD_ROW = '.desktop-header .pane-header-brand';

/** The controls leading that row, each a box and a gap the floor pays at both
 *  ends. */
export const THREAD_ROW_LEAD_CONTROLS = `${THREAD_ROW} > :is(.brand-compose-btn, .home-thread-btn)`;

/** Floor for the Conversation pane, at the CURRENT root font size. Reads the
 *  same lead the drawer's floor does, and by the same rule: the reserve decides
 *  how the row is LAID OUT, not how narrow the pane may get. New thread leads
 *  the row. Home joins it once the thread list carries it. Each box counts
 *  while it shows. */
export function minThreadPanePx(): number {
  const leadControls = document.querySelectorAll(THREAD_ROW_LEAD_CONTROLS).length;
  return computeMinThreadPaneWidth(getRemPx(), titlebarLightsReservePx(), leadControls);
}

/** Floor for the Canvas pane, from the root font size. Pure, like the others. */
export function computeMinContentPaneWidth(remPx: number): number {
  return Math.ceil(MIN_CONTENT_PANE_REM * remPx);
}

/** Floor for the Canvas pane, at the CURRENT root font size. */
export function minContentPanePx(): number {
  return computeMinContentPaneWidth(getRemPx());
}

/** The divider band: `--divider-width` in styles/global/base.css. The rail pads
 *  its leading side by it, and the drawer's divider is one. */
export const DIVIDER_BAND_REM = 0.1875;

/** The divider band, at the CURRENT root font size. */
export function dividerBandPx(): number {
  return DIVIDER_BAND_REM * getRemPx();
}

/** Floor for the desktop rail that holds the dashboard: two of the dashboard's
 *  cells, the gap between them, the dashboard's side padding, the rail's divider
 *  band, and the scrollbar gutter `.dashboard-rail-body` reserves. Pure, so the
 *  arithmetic is testable without a layout engine. */
export function computeMinDashboardRailWidth(remPx: number, scrollbarGutterPx: number): number {
  const dashboard = DASHBOARD_MIN_COLUMNS * DASHBOARD_CELL_REM + (DASHBOARD_MIN_COLUMNS - 1) * DASHBOARD_GAP_REM + 2 * DASHBOARD_PAD_X_REM;
  const rem = dashboard + DIVIDER_BAND_REM;
  return Math.ceil(rem * remPx + scrollbarGutterPx);
}

/** The `--scrollbar-gutter-width` that `utils/scrollbarGutter.ts` publishes,
 *  in px. A layout-less environment reads none, which is no gutter. */
function publishedScrollbarGutterPx(): number {
  const value = getComputedStyle(document.documentElement).getPropertyValue('--scrollbar-gutter-width');
  return parseFloat(value) || 0;
}

/** Floor for the dashboard's rail, at the CURRENT root font size. */
export function minDashboardRailPx(): number {
  return computeMinDashboardRailWidth(getRemPx(), publishedScrollbarGutterPx());
}

/** The two split-pane floors, measured together and handed to the pure clamp
 *  helpers in `splitHelpers.ts`. One object so a caller cannot pass the pair in
 *  the wrong order, and so those helpers stay pure: the DOM read is the
 *  caller's, the arithmetic is theirs. */
export interface SplitBounds {
  minThreadPx: number;
  minContentPx: number;
}

export function splitBounds(): SplitBounds {
  return { minThreadPx: minThreadPanePx(), minContentPx: minContentPanePx() };
}

/** All four floors, for the *drawn layout* (`computeDrawnLayout` in
 *  components/layout/splitHelpers.ts): the window holds the drawer, the split
 *  and the rail side by side, and each is weighed against the others. */
export interface PaneFloors extends SplitBounds {
  drawerPx: number;
  railPx: number;
}

export function paneFloors(): PaneFloors {
  return { ...splitBounds(), drawerPx: minDrawerWidth(), railPx: minDashboardRailPx() };
}
