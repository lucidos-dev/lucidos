import { splitRatio, scaledDurationMs, SPLIT_RATIO_KEY } from '../../store/store';
import type { PaneFloors, SplitBounds } from '../../store/paneMinimums';

export const DEFAULT_SPLIT_RATIO = 0.4;

/* The pane minimums themselves live in `store/paneMinimums.ts`: they are
   derived from the root font size, so reading them is a DOM read, and every
   helper in this module is pure by contract. Callers measure, these compute. */

/** Mirror of var(--duration-slow) in styles/global/base.css — the duration
 *  every pane/header geometry transition runs at, AT 1x. TS timers that must
 *  outlive those transitions derive from this; update the two together.
 *
 *  1x, because the token is that literal times var(--duration-scale) (the
 *  Animation speed slider). A timer mirroring it therefore passes this through
 *  `scaledDurationMs` rather than using it raw, or the two come apart the
 *  moment the slider leaves centre. */
export const PANE_TRANSITION_MS = 300;

/** Clamp a divider position to a range that may be EMPTY, which is the whole
 *  reason this is a named function rather than a `Math.min(Math.max(...))`.
 *
 *  `lo` is what the leading pane needs and `hi` what is left after the trailing
 *  pane takes its own minimum, so `hi < lo` means the container cannot hold both
 *  at once. A bare min-then-max would silently answer `hi` there, handing the
 *  space to the trailing pane; this keeps the LEADING pane whole instead and
 *  lets the trailing one take what remains. Deterministic either way, which a
 *  drag needs.
 *
 *  For the split, that remainder shows only while a drag holds the collapse
 *  state. At rest the Canvas yields instead (`computeDrawnSplitRatio`). */
export function clampToRange(value: number, lo: number, hi: number): number {
  if (hi < lo) return lo;
  return Math.min(Math.max(value, lo), hi);
}

/** Where a dragged split divider lands: the pointer, clamped so neither pane
 *  goes below its minimum. Returns a ratio strictly INSIDE (0, 1).
 *
 *  A clamp, NOT a free drop plus a release-time correction. The divider stops at
 *  the wall while the pointer keeps going, so the width the user releases is the
 *  width that persists.
 *
 *  Strictly inside is the load-bearing half, and it is guaranteed here rather
 *  than derived: ADR 0056's whole argument for allowing a mid-drag clamp is that
 *  a collapse-state flip is UNREACHABLE during a drag, and those attributes flip
 *  at a ratio of exactly 0 or 1. The pane-minimum clamp alone does not give that.
 *  When the container cannot hold both minimums, `clampToRange` hands back the
 *  leading pane's minimum, which can exceed the container: a drawer width
 *  persisted from a wider window can leave the split at 520px against a 525px
 *  thread minimum at 175% ui-scale, and 525/520 rounds down to exactly 1, which
 *  collapses the content pane under the pointer. The final 1px inset is what the
 *  drag handlers used to carry by hand and is the reason it existed. */
export function clampSplitRatio(pointerPx: number, totalPx: number, bounds: SplitBounds): number {
  if (!splitIsSized(totalPx)) return DEFAULT_SPLIT_RATIO;
  const threadPx = clampToRange(pointerPx, bounds.minThreadPx, totalPx - bounds.minContentPx);
  return clampToRange(threadPx, 1, totalPx - 1) / totalPx;
}

/** Whether the split, or the window, has been laid out. Its first frame can
 *  measure 0 while the shell sizes, which says nothing about the room it will
 *  have. */
export function splitIsSized(totalPx: number): boolean {
  return totalPx > 2;
}

/** Whether a split this wide holds both pane floors side by side. */
export function splitFitsBothPanes(totalPx: number, bounds: SplitBounds): boolean {
  return totalPx >= bounds.minThreadPx + bounds.minContentPx;
}

/** The *drawn split ratio* (docs/glossary.md): what the split draws for the
 *  stored ratio in a split this wide. The stored ratio is the user's choice and
 *  is never rewritten for lack of room, so a wider window brings it back.
 *
 *  With room for both panes, it is the stored ratio held to both floors. With
 *  room for one, an open Canvas yields: it is drawn collapsed (1) rather than
 *  squeezed under its floor, where its header controls overlap (ADR 0056 as
 *  amended). A collapsed pane is a state, so it draws as stored.
 *
 *  `heldYield` pins the collapse decision for a drag in flight, so no drag
 *  flips a collapse attribute. Null decides from the room. */
export function computeDrawnSplitRatio(
  stored: number, totalPx: number, bounds: SplitBounds, heldYield: boolean | null,
): number {
  if (stored <= 0) return 0;
  if (stored >= 1) return 1;
  if (!splitIsSized(totalPx)) return stored;
  const yields = heldYield ?? !splitFitsBothPanes(totalPx, bounds);
  return yields ? 1 : clampSplitRatio(stored * totalPx, totalPx, bounds);
}

/** The stored ratio that shows a hidden Canvas: the default split when both
 *  panes fit, else the Canvas takes the split from the Conversation. The
 *  default alone would yield again and leave the toggle doing nothing. */
export function contentShownRatio(fitsBoth: boolean): number {
  return fitsBoth ? DEFAULT_SPLIT_RATIO : 0;
}

/** Next ratio for a thread-pane visibility toggle: collapsed (0) restores the
 *  default split, anything else collapses. Shared by the toggleThreadPane
 *  intent and the content-side header double-click. `ratio` is the drawn one. */
export function toggleThreadPaneRatio(ratio: number): number {
  return ratio === 0 ? DEFAULT_SPLIT_RATIO : 0;
}

/** Mirror for the content pane: drawn collapsed (>= 1) shows it
 *  (`contentShownRatio`), else collapses. */
export function toggleContentPaneRatio(ratio: number, fitsBoth: boolean): number {
  return ratio >= 1 ? contentShownRatio(fitsBoth) : 1;
}

/** Pixels a keyboard resize (the Narrow/Widen pane shortcuts) moves a divider
 *  per press. */
export const KEYBOARD_RESIZE_STEP_PX = 80;

/** Where a keyboard step lands the split divider, or null for a no-op. Same
 *  clamp as a drag (`clampSplitRatio`), which is the point: one wall, whichever
 *  way the user moves the divider. It never collapses a pane, and stepping back
 *  into a fully-collapsed one reopens it at its minimum width.
 *
 *  Keeps its own no-op cases, which a drag has no equivalent of: a collapsed
 *  pane is a settled state, so a step pushing further out stays put rather than
 *  re-expanding, and a container too narrow for both minimums answers null
 *  instead of `clampToRange`'s pick, since there is no gesture in flight that
 *  has to land somewhere. */
export function computeStepRatio(
  ratio: number, totalPx: number, deltaPx: number, bounds: SplitBounds,
): number | null {
  if (totalPx <= 0) return null;
  if (deltaPx > 0 && ratio >= 1) return null;
  if (deltaPx < 0 && ratio <= 0) return null;
  if (totalPx - bounds.minContentPx < bounds.minThreadPx) return null;
  const nextRatio = clampSplitRatio(ratio * totalPx + deltaPx, totalPx, bounds);
  return nextRatio === ratio ? null : nextRatio;
}

/** Where a keyboard step lands the thread drawer width, or null for a no-op.
 *  `minPx` is the drawer's floor and `maxPx` the widest it may go: the caller
 *  measures both (the floor is derived from the root font size and the desktop
 *  build, the ceiling from the row less the visible split panes' minimums), so
 *  this stays pure like every other computation in this module. Clamps
 *  immediately and never closes the drawer (that's the toggle's job). */
export function computeDrawerStepWidth(
  width: number, deltaPx: number, minPx: number, maxPx: number,
): number | null {
  if (maxPx < minPx) return null;
  const next = Math.min(Math.max(width + deltaPx, minPx), maxPx);
  return next === width ? null : next;
}

/** The widest the dashboard's rail may go. `roomPx` is the split's width plus
 *  the rail's, the room the two share. The Conversation pane holds its width,
 *  so a wider rail comes out of the Canvas. With one pane collapsed, it comes
 *  out of the other. */
export function dashboardRailCeiling(roomPx: number, threadPx: number, ratio: number, bounds: SplitBounds): number {
  if (ratio <= 0) return roomPx - bounds.minContentPx;
  if (ratio >= 1) return roomPx - bounds.minThreadPx;
  return roomPx - threadPx - bounds.minContentPx;
}

/** The room the drawer, the split and the rail share: the window, every floor,
 *  the drawer divider's band, and the stored split ratio, which says which split
 *  panes the side panes must leave room for. */
export interface LayoutRoom {
  windowPx: number;
  floors: PaneFloors;
  dividerPx: number;
  storedRatio: number;
}

/** A side pane as stored: whether the user wants it, and the width they set. */
export interface SidePaneState {
  wanted: boolean;
  widthPx: number;
}

/** The yield each side pane is pinned to for a drag in flight: true is yielded,
 *  false is drawn. Null decides from the room. */
export interface HeldSideYields {
  drawer: boolean | null;
  rail: boolean | null;
}

export interface DrawnSidePane {
  drawn: boolean;
  /** What it draws at, or would draw at: a yielded pane keeps its width for the
   *  slide out. */
  widthPx: number;
}

export interface DrawnLayout {
  drawer: DrawnSidePane;
  rail: DrawnSidePane;
  /** What is left for the split, the Conversation and the Canvas together. */
  splitPx: number;
}

/** The pane the side panes must always leave whole: the Conversation, or the
 *  Canvas when the stored split shows only that. */
function keptFloorPx(room: LayoutRoom): number {
  return room.storedRatio <= 0 ? room.floors.minContentPx : room.floors.minThreadPx;
}

/** Whether the window holds the kept pane's floor beside these side panes at
 *  their floors. A drawn drawer takes its divider's band too. */
export function sidePanesFit(room: LayoutRoom, drawer: boolean, rail: boolean): boolean {
  const { floors, dividerPx } = room;
  const neededPx = keptFloorPx(room) + (drawer ? floors.drawerPx + dividerPx : 0) + (rail ? floors.railPx : 0);
  return neededPx <= room.windowPx;
}

/** The first set of wanted side panes that fits: both, the drawer alone, the
 *  rail alone, neither. So the rail yields before the drawer. */
function fittingSidePanes(room: LayoutRoom, wantDrawer: boolean, wantRail: boolean): [boolean, boolean] {
  const sets: [boolean, boolean][] = [[wantDrawer, wantRail], [wantDrawer, false], [false, wantRail]];
  return sets.find(([drawer, rail]) => sidePanesFit(room, drawer, rail)) ?? [false, false];
}

/** The *drawn layout* (docs/glossary.md): what the window draws for the stored
 *  drawer and rail. The stored state is the user's choice and is never rewritten
 *  for lack of room, so a wider window brings a yielded pane back.
 *
 *  A pane that does not fit beside the kept pane's floor yields, in the order
 *  Canvas, rail, drawer. The Canvas yields inside the split
 *  (`computeDrawnSplitRatio`, ADR 0495), from the `splitPx` returned here.
 *  A drawn drawer rides the Conversation, so none draws beside a collapsed one.
 *
 *  A drawn pane draws at its stored width, held so the panes beside it keep
 *  their floors, and never below its own. The drawer is held first, for the
 *  kept pane and a drawn rail's floor. The rail is held for the drawn drawer
 *  and every split pane the stored ratio shows.
 *
 *  `held` pins a yield for a drag in flight, so no drag flips a pane. */
export function computeDrawnLayout(
  room: LayoutRoom, drawer: SidePaneState, rail: SidePaneState, held: HeldSideYields,
): DrawnLayout {
  const { floors, dividerPx, windowPx } = room;
  const wantDrawer = drawer.wanted && room.storedRatio > 0;
  if (!splitIsSized(windowPx)) {
    return {
      drawer: { drawn: wantDrawer, widthPx: drawer.widthPx },
      rail: { drawn: rail.wanted, widthPx: rail.widthPx },
      splitPx: windowPx,
    };
  }
  const [fitsDrawer, fitsRail] = fittingSidePanes(room, wantDrawer, rail.wanted);
  const drawerDrawn = wantDrawer && !(held.drawer ?? !fitsDrawer);
  const railDrawn = rail.wanted && !(held.rail ?? !fitsRail);

  const drawerCeilingPx = windowPx - dividerPx - keptFloorPx(room) - (railDrawn ? floors.railPx : 0);
  const drawerPx = Math.max(floors.drawerPx, Math.min(drawer.widthPx, drawerCeilingPx));
  const drawerBlockPx = drawerDrawn ? drawerPx + dividerPx : 0;

  const splitFloorsPx = (room.storedRatio < 1 ? floors.minContentPx : 0) + (room.storedRatio > 0 ? floors.minThreadPx : 0);
  const railPx = Math.max(floors.railPx, Math.min(rail.widthPx, windowPx - drawerBlockPx - splitFloorsPx));

  return {
    drawer: { drawn: drawerDrawn, widthPx: drawerPx },
    rail: { drawn: railDrawn, widthPx: railPx },
    splitPx: windowPx - drawerBlockPx - (railDrawn ? railPx : 0),
  };
}

let paneAnimateTimer: ReturnType<typeof setTimeout> | null = null;

/** Add a brief CSS transition for an explicit ratio change: a pane toggle, a
 *  maximize, a keyboard step, a layout reset. NOT for a drag, which tracks the
 *  pointer 1:1 and turns these transitions off (`beginPaneResize`, drawnLayout.ts).
 *
 *  Named for the animation rather than for the deferred snap it used to serve:
 *  a drag is clamped now and corrects nothing on release, so "snap" described
 *  no caller left. The timeout outlives the PANE_TRANSITION_MS transition so the
 *  class never drops mid-flight, and rapid repeated calls (a held keyboard-resize
 *  chord) reset the timer instead of stacking removals, since an earlier call's
 *  removal must not fire mid-way through the latest call's transition.
 *
 *  The transition scales with the Animation speed slider, so the timer does too.
 *  The 100ms is slack, not animation, and stays OUTSIDE the scaled term: at 0.1x
 *  an unscaled timer would strip `pane-animate` a tenth of the way in and snap a
 *  maximizing pane the rest of the way to full width. */
export function triggerPaneAnimate(): void {
  const container = document.querySelector('.split-layout') as HTMLElement | null;
  if (!container) return;
  container.classList.add('pane-animate');
  if (paneAnimateTimer) clearTimeout(paneAnimateTimer);
  paneAnimateTimer = setTimeout(() => {
    paneAnimateTimer = null;
    container.classList.remove('pane-animate');
  }, scaledDurationMs(PANE_TRANSITION_MS) + 100);
}

/** Update splitRatio with animations and persist to localStorage */
export function setSplitRatio(newRatio: number) {
  triggerPaneAnimate();
  splitRatio.value = newRatio;
  localStorage.setItem(SPLIT_RATIO_KEY, String(newRatio));
}
