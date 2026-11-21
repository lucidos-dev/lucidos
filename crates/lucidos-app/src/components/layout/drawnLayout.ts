import { batch, computed, signal } from '@preact/signals';
import { splitRatio, threadDrawerOpen, threadDrawerWidth } from '../../store/store';
import { dashboardRailShown, dashboardRailWidth } from '../../store/dashboardRail';
import { dividerBandPx, paneFloors, type PaneFloors } from '../../store/paneMinimums';
import {
  DEFAULT_SPLIT_RATIO, computeDrawnLayout, computeDrawnSplitRatio, sidePanesFit, splitFitsBothPanes,
  splitIsSized, triggerPaneAnimate,
  type DrawnLayout, type HeldSideYields, type LayoutRoom, type SidePaneState,
} from './splitHelpers';

/** The window's measured width and every pane floor. Null until `SplitLayout`
 *  has measured it, and on the phone, which has no side panes. */
export interface LayoutGeometry {
  windowPx: number;
  floors: PaneFloors;
  dividerPx: number;
}

export const layoutGeometry = signal<LayoutGeometry | null>(null);

/** The yield each pane is pinned to for a divider drag in flight, true for
 *  yielded. A pin is null when nothing holds that pane. */
interface HeldYields extends HeldSideYields {
  canvas: boolean | null;
}

const UNHELD: HeldYields = { canvas: null, drawer: null, rail: null };

const heldYields = signal<HeldYields>(UNHELD);

/** The stored layout, which the user chose. A yield never writes any of it. */
interface StoredLayout {
  splitRatio: number;
  drawer: SidePaneState;
  rail: SidePaneState;
}

function readStored(): StoredLayout {
  return {
    splitRatio: splitRatio.value,
    drawer: { wanted: threadDrawerOpen.value, widthPx: threadDrawerWidth.value },
    rail: { wanted: dashboardRailShown.value, widthPx: dashboardRailWidth.value },
  };
}

function peekStored(): StoredLayout {
  return {
    splitRatio: splitRatio.peek(),
    drawer: { wanted: threadDrawerOpen.peek(), widthPx: threadDrawerWidth.peek() },
    rail: { wanted: dashboardRailShown.peek(), widthPx: dashboardRailWidth.peek() },
  };
}

interface Drawn {
  layout: DrawnLayout;
  splitRatio: number;
}

const UNMEASURED: LayoutGeometry = {
  windowPx: 0,
  floors: { drawerPx: 0, minThreadPx: 0, minContentPx: 0, railPx: 0 },
  dividerPx: 0,
};

function roomOf(geometry: LayoutGeometry, storedRatio: number): LayoutRoom {
  return { windowPx: geometry.windowPx, floors: geometry.floors, dividerPx: geometry.dividerPx, storedRatio };
}

/** What the layout draws for `stored` in this window. An unmeasured window
 *  draws the stored layout as it is. */
function drawnFor(geometry: LayoutGeometry | null, stored: StoredLayout, held: HeldYields): Drawn {
  const room = roomOf(geometry ?? UNMEASURED, stored.splitRatio);
  const layout = computeDrawnLayout(room, stored.drawer, stored.rail, held);
  return {
    layout,
    splitRatio: geometry
      ? computeDrawnSplitRatio(stored.splitRatio, layout.splitPx, geometry.floors, held.canvas)
      : stored.splitRatio,
  };
}

const drawn = computed(() => drawnFor(layoutGeometry.value, readStored(), heldYields.value));

/** What the split draws: the *drawn split ratio* (docs/glossary.md). Every
 *  reader that describes the screen reads this. Writers write `splitRatio`. */
export const drawnSplitRatio = computed(() => drawn.value.splitRatio);

/** What the drawer and the rail draw: the *drawn layout* (docs/glossary.md).
 *  Readers that describe the screen read these, never the stored signals.
 *  Primitives, so a reader re-runs only when its own pane changes. */
export const drawnDrawerShown = computed(() => drawn.value.layout.drawer.drawn);
export const drawnDrawerWidth = computed(() => drawn.value.layout.drawer.widthPx);
export const drawnRailShown = computed(() => drawn.value.layout.rail.drawn);
export const drawnRailWidth = computed(() => drawn.value.layout.rail.widthPx);

/** The fit questions below read `.value`, so a toggle that asks while it
 *  renders redraws when the answer changes. A handler that asks subscribes to
 *  nothing. */

/** The geometry, when it is measured and sized, so a fit question has an
 *  answer. An unmeasured layout keeps the ordinary toggles. */
function measured(): LayoutGeometry | null {
  const geometry = layoutGeometry.value;
  return geometry && splitIsSized(geometry.windowPx) ? geometry : null;
}

/** Whether the split holds both pane floors. True before it is measured. */
export function splitFitsBoth(): boolean {
  const geometry = measured();
  return !geometry || splitFitsBothPanes(drawn.value.layout.splitPx, geometry.floors);
}

/** Whether the window holds the Conversation's floor beside these side panes at
 *  their floors. True before it is measured. */
function sidePanesFitNow(drawer: boolean, rail: boolean, storedRatio: number): boolean {
  const geometry = measured();
  return !geometry || sidePanesFit(roomOf(geometry, storedRatio), drawer, rail);
}

/** Whether the drawer can show beside the Conversation with the rail yielded.
 *  A collapsed Conversation returns when the drawer shows, so it counts. */
export function threadListCanShow(): boolean {
  const stored = splitRatio.value;
  return sidePanesFitNow(true, false, stored > 0 ? stored : DEFAULT_SPLIT_RATIO);
}

/** Whether the rail can show with the drawer closed. */
export function dashboardRailCanShow(): boolean {
  return sidePanesFitNow(false, true, splitRatio.value);
}

/** Whether the rail shows beside the drawer as drawn now, with no swap. */
export function dashboardRailFitsBesideDrawer(): boolean {
  return sidePanesFitNow(drawnDrawerShown.value, true, splitRatio.value);
}

function yieldFlags(d: Drawn): boolean[] {
  return [d.splitRatio >= 1, !d.layout.drawer.drawn, !d.layout.rail.drawn];
}

/** Apply a change to what decides the yields. A pane that yields or returns for
 *  room animates like a toggle, so the class goes on before the write, as in
 *  `setSplitRatio`. The first measure draws without one. */
function redecideYields(geometry: LayoutGeometry | null, held: HeldYields): void {
  if (layoutGeometry.peek() && geometry) {
    const before = yieldFlags(drawn.peek());
    const after = yieldFlags(drawnFor(geometry, peekStored(), held));
    if (before.some((flag, i) => flag !== after[i])) triggerPaneAnimate();
  }
  batch(() => {
    layoutGeometry.value = geometry;
    heldYields.value = held;
  });
}

function sameGeometry(a: LayoutGeometry, b: LayoutGeometry): boolean {
  return a.windowPx === b.windowPx && a.dividerPx === b.dividerPx
    && (Object.keys(a.floors) as (keyof PaneFloors)[]).every((key) => a.floors[key] === b.floors[key]);
}

/** Re-read the window's width and the floors. The floors are DOM reads
 *  (paneMinimums), so they are measured here and never inside the computed. */
export function measureLayout(): void {
  const next: LayoutGeometry = { windowPx: window.innerWidth, floors: paneFloors(), dividerPx: dividerBandPx() };
  const previous = layoutGeometry.peek();
  if (previous && sameGeometry(previous, next)) return;
  redecideYields(next, heldYields.peek());
}

/** Set while a divider drag is live. CSS keys off it to turn the geometry
 *  transitions off, so the panes and header regions track the pointer 1:1. */
const RESIZING_ATTR = 'data-pane-resizing';

/** Call on divider pointerdown: switch the layout into 1:1 pointer-tracking
 *  mode, and pin every yield. A drag that changes the room (the drawer's, the
 *  rail's) must not flip a pane under the pointer (ADR 0056). */
export function beginPaneResize(): void {
  const stored = splitRatio.peek();
  const now = drawn.peek();
  heldYields.value = {
    canvas: stored > 0 && stored < 1 && now.splitRatio >= 1,
    drawer: !now.layout.drawer.drawn,
    rail: !now.layout.rail.drawn,
  };
  document.documentElement.setAttribute(RESIZING_ATTR, '');
}

/** Call on divider release: transitions come back on, and every yield is
 *  decided from the room the drag left. */
export function endPaneResize(): void {
  redecideYields(layoutGeometry.peek(), UNHELD);
  document.documentElement.removeAttribute(RESIZING_ATTR);
}
