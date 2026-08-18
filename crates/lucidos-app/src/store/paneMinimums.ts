/**
 * How narrow each desktop pane is allowed to get: the thread drawer, the
 * Conversation pane, and the Canvas pane.
 *
 * One module for all three because they are read TOGETHER. A divider drag is
 * clamped to what the panes on either side of it need, and a container too
 * narrow to hold every minimum at once has to degrade deterministically
 * (`clampSplitRatio` in components/layout/splitHelpers.ts), so a reader weighing
 * one floor is always weighing it against the others. They used to live in two
 * files, one of them a 2000-line store.
 *
 * All three are DERIVED from the root font size rather than written as px
 * constants, because everything they size is rem-authored: a constant is only
 * right at the one UI scale it was measured at. The drawer's floor learned that
 * first (its `260` was computed "at 16px/rem" for a header row that no longer
 * existed); the two pane floors follow, and are byte-identical to the `300` and
 * `360` they replace at a 16px root.
 *
 * The cost of deriving them is that the three no longer fit every window: they
 * sum to 1010px at 100% ui-scale, 1182 at 125%, 1355 at 150% and 1527 at 175%,
 * so from 150% on a 1280px screen `clampSplitRatio`'s empty-range branch is what
 * the user actually meets (137.5%, the step below, still fits at 1269). That
 * branch is load-bearing, not a corner. Those sums hold on every desktop client,
 * because neither derived floor varies by build (see `computeMinDrawerWidth`
 * and `computeMinThreadPaneWidth`).
 *
 * Deliberately holds no signal and imports nothing from `store.ts`: the store's
 * own module init reads `minDrawerWidth()` to clamp the persisted drawer width,
 * so an import back the other way would be a cycle resolved at boot.
 */
import { getRemPx } from '../utils/dom';

/** One END of the threads header's row, in rem, excluding whatever pads it: a
 *  button (Filter leading, Search trailing) and its gap. Mirrors
 *  `.threads-header` in styles/panels/shell.css (`--header-icon-box` and
 *  `--pane-header-gap`). */
const DRAWER_ROW_SIDE_REM = 2.25 + 0.25;
/** The title's own room, in rem: wide enough to stay a title rather than an
 *  ellipsis. */
const DRAWER_ROW_TITLE_REM = 4.5;
/** The row's own padding, in rem, per end. Half of `.threads-header`'s
 *  `0 0.5rem`. It is the floor UNDER the floor: whatever lead the row is sized
 *  around, an end can never be narrower than the padding every build has. */
const DRAWER_ROW_PAD_REM = 0.5;
/** Fallback for the traffic-lights reserve if the property cannot be read (it
 *  is declared inside the desktop media query, so a sub-769px viewport reports
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
/** The row's own leading padding, in rem. `--brand-lead-inset`'s web value, and
 *  the floor UNDER the floor for the same reason the drawer's is: whatever lead
 *  the row is sized around, an end is never narrower than the padding every
 *  build has. */
const THREAD_ROW_PAD_REM = 0.5;
/** The centred brand cluster at its natural width, in rem: two chevrons and the
 *  mark's tap target, touching. Mirrors `--desktop-nav-min-span`
 *  (`2 * --header-icon-box + --header-mark-tap`), which the clamp on
 *  `.pane-header-brand-label` floors the box at. */
const THREAD_ROW_CLUSTER_REM = 2 * 2.25 + 2.1;
/** The Canvas pane's floor. 360px at a 16px root. It is a constant where the
 *  other two are derived, and it can be. Its row needs
 *  `2 * --content-side-reserve + --desktop-nav-min-span`, which is 22.1rem, so
 *  this already covers it. The Canvas title cluster therefore never reaches its
 *  clamp's min-span arm at or above this width. Pinned in the suite. */
const MIN_CONTENT_PANE_REM = 22.5;

/** Floor for the thread drawer's width, from the root font size and the lead its
 *  header row is sized around. Pure, so the arithmetic is testable without a
 *  layout engine; the DOM read lives in `minDrawerWidth` below.
 *
 *  The row is sized around a title centred on the PANE (`.threads-header-title`
 *  in styles/panels/shell.css), not on the gap between the two buttons flanking
 *  it, so the title clears the WIDER of the row's two ends on BOTH sides and the
 *  floor is symmetric: `2 * side + title`. That is why the lead is counted twice
 *  rather than once.
 *
 *  ONE floor for every desktop client: its only caller passes the traffic-lights
 *  reserve as the lead, on the web build that has no lights too (ADR 0058). That
 *  is deliberate, not an oversight: a workspace opened in the browser and the
 *  same workspace in the packaged app must stop the drawer at the same width,
 *  and the packaged build's row is the wider of the two (its controls reach up
 *  into the reclaimed title-bar band, so it starts after the fixed reserve that
 *  clears the lights, where the web row starts after its own 0.5rem). Taking the
 *  wider one is what makes the floor a floor on both. The web row still LAYS OUT
 *  at 0.5rem, so what it gains is 144px of title room it is not obliged to use;
 *  nothing there paints against a light that is not there.
 *
 *  The `max` against the row's own padding keeps the floor honest at the other
 *  end of the scale: the reserve is PX (the lights are OS chrome and do not
 *  scale with our root font size) while the padding is rem, so at a large enough
 *  root the padding is the wider end and has to win. */
export function computeMinDrawerWidth(remPx: number, leadPx: number): number {
  const sidePx = Math.max(leadPx, DRAWER_ROW_PAD_REM * remPx) + DRAWER_ROW_SIDE_REM * remPx;
  return Math.ceil(2 * sidePx + DRAWER_ROW_TITLE_REM * remPx);
}

/** The lead both derived floors are sized around, read from CSS rather than
 *  restated, with the literal as the fallback. The property is declared inside
 *  the desktop media query, so a viewport under 769px (or a test harness with no
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
export function computeMinThreadPaneWidth(remPx: number, leadPx: number): number {
  const sidePx = Math.max(leadPx, THREAD_ROW_PAD_REM * remPx) + THREAD_ROW_SIDE_REM * remPx;
  return Math.ceil(2 * sidePx + THREAD_ROW_CLUSTER_REM * remPx);
}

/** Floor for the Conversation pane, at the CURRENT root font size. Reads the
 *  same lead the drawer's floor does, and by the same rule: the reserve decides
 *  how the row is LAID OUT, not how narrow the pane may get. */
export function minThreadPanePx(): number {
  return computeMinThreadPaneWidth(getRemPx(), titlebarLightsReservePx());
}

/** Floor for the Canvas pane. */
export function minContentPanePx(): number {
  return Math.ceil(MIN_CONTENT_PANE_REM * getRemPx());
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
