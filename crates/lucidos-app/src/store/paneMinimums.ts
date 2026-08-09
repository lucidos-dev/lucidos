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
 * sum to 828px at 100% ui-scale, 1035 at 125%, 1242 at 150% and 1449 at 175%,
 * so past roughly 150% on a 1280px screen `clampSplitRatio`'s empty-range branch
 * is what the user actually meets. That branch is load-bearing, not a corner.
 * Those are the web build's sums; the packaged macOS build adds twice the fixed
 * traffic-lights reserve on top (see `computeMinDrawerWidth`), so it meets the
 * same branch on a wider screen.
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
/** The row's own padding, in rem, at whichever end has no traffic lights to
 *  clear. Half of `.threads-header`'s `0 0.5rem`, so it is also the leading
 *  padding on a build with no lights at all. */
const DRAWER_ROW_PAD_REM = 0.5;
/** Fallback for the traffic-lights reserve if the property cannot be read (it
 *  is declared inside the desktop media query, so a sub-769px viewport reports
 *  nothing). Kept in step with `--titlebar-lights-reserve` in shell.css. */
const TITLEBAR_LIGHTS_RESERVE_PX = 80;

/** The Conversation pane's floor. 300px at a 16px root, which is the constant
 *  this replaces. */
const MIN_THREAD_PANE_REM = 18.75;
/** The Canvas pane's floor. 360px at a 16px root, likewise. It is the wider of
 *  the two because its header (hamburger, title, action icons) and its typical
 *  content need more room than a chat column. */
const MIN_CONTENT_PANE_REM = 22.5;

/** Floor for the thread drawer's width, from the root font size and the leading
 *  reserve. Pure, so the arithmetic is testable without a layout engine; the DOM
 *  read lives in `minDrawerWidth` below.
 *
 *  The row is sized around a title centred on the PANE (`.threads-header-title`
 *  in styles/panels/shell.css), not on the gap between the two buttons flanking
 *  it, so the title clears the WIDER of the row's two ends on BOTH sides and the
 *  floor is symmetric: `2 * side + title`. That is why the reserve below is
 *  counted twice rather than once, and it is the whole difference between the
 *  two builds.
 *
 *  Only the row's LEADING padding differs between them. The web build starts
 *  after its own 0.5rem, which makes both its ends the same width and leaves its
 *  floor byte-identical to the pre-centring one. The packaged macOS build starts
 *  after the fixed reserve that clears the traffic lights, since the row's
 *  controls reach up into the reclaimed title-bar band, so there the leading end
 *  is the wider one and sets both sides. The reserve is PX and stays outside the
 *  rem term on purpose: the lights are OS chrome and do not scale with our root
 *  font size. */
export function computeMinDrawerWidth(remPx: number, lightsReservePx: number | null): number {
  const leadingPx = lightsReservePx ?? DRAWER_ROW_PAD_REM * remPx;
  const trailingPx = DRAWER_ROW_PAD_REM * remPx;
  const sidePx = Math.max(leadingPx, trailingPx) + DRAWER_ROW_SIDE_REM * remPx;
  return Math.ceil(2 * sidePx + DRAWER_ROW_TITLE_REM * remPx);
}

/** Floor for the thread drawer's width: exactly what its header row needs, at
 *  the CURRENT root font size and on the current desktop build. */
export function minDrawerWidth(): number {
  const root = document.documentElement;
  const rem = getRemPx();
  if (!root.hasAttribute('data-titlebar-overlay')) return computeMinDrawerWidth(rem, null);
  // Read from CSS rather than restated, with the literal as the fallback: the
  // property is declared inside the desktop media query, so a viewport under
  // 769px (or a test harness with no layout engine) reports nothing.
  const reserve = parseFloat(
    getComputedStyle(root).getPropertyValue('--titlebar-lights-reserve'),
  );
  return computeMinDrawerWidth(
    rem,
    Number.isFinite(reserve) ? reserve : TITLEBAR_LIGHTS_RESERVE_PX,
  );
}

/** Floor for the Conversation pane. */
export function minThreadPanePx(): number {
  return Math.ceil(MIN_THREAD_PANE_REM * getRemPx());
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
