import { batch, computed, signal } from '@preact/signals';
import { splitRatio, threadDrawerOpen, threadDrawerWidth } from '../../store/store';
import { dashboardRailShown, dashboardRailWidth } from '../../store/dashboardRail';
import { dividerBandPx, paneFloors, type PaneFloors } from '../../store/paneMinimums';
import {
  DEFAULT_SPLIT_RATIO, computeDrawnLayout, computeDrawnSplitRatio, sidePanesFit, splitFitsBothPanes,
  splitIsSized, triggerPaneAnimate,
  type DrawnLayout, type HeldSideYields, type LayoutRoom, type SidePaneState,
} from './splitHelpers';

export interface LayoutGeometry {
  windowPx: number;
  floors: PaneFloors;
  dividerPx: number;
}

export const layoutGeometry = signal<LayoutGeometry | null>(null);

interface HeldYields extends HeldSideYields {
  canvas: boolean | null;
}

const UNHELD: HeldYields = { canvas: null, drawer: null, rail: null };

const heldYields = signal<HeldYields>(UNHELD);

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

export const drawnSplitRatio = computed(() => drawn.value.splitRatio);

export const drawnDrawerShown = computed(() => drawn.value.layout.drawer.drawn);
export const drawnDrawerWidth = computed(() => drawn.value.layout.drawer.widthPx);
export const drawnRailShown = computed(() => drawn.value.layout.rail.drawn);
export const drawnRailWidth = computed(() => drawn.value.layout.rail.widthPx);


function measured(): LayoutGeometry | null {
  const geometry = layoutGeometry.value;
  return geometry && splitIsSized(geometry.windowPx) ? geometry : null;
}

export function splitFitsBoth(): boolean {
  const geometry = measured();
  return !geometry || splitFitsBothPanes(drawn.value.layout.splitPx, geometry.floors);
}

function sidePanesFitNow(drawer: boolean, rail: boolean, storedRatio: number): boolean {
  const geometry = measured();
  return !geometry || sidePanesFit(roomOf(geometry, storedRatio), drawer, rail);
}

export function threadListCanShow(): boolean {
  const stored = splitRatio.value;
  return sidePanesFitNow(true, false, stored > 0 ? stored : DEFAULT_SPLIT_RATIO);
}

export function dashboardRailCanShow(): boolean {
  return sidePanesFitNow(false, true, splitRatio.value);
}

export function dashboardRailFitsBesideDrawer(): boolean {
  return sidePanesFitNow(drawnDrawerShown.value, true, splitRatio.value);
}

function yieldFlags(d: Drawn): boolean[] {
  return [d.splitRatio >= 1, !d.layout.drawer.drawn, !d.layout.rail.drawn];
}

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

export function measureLayout(): void {
  const next: LayoutGeometry = { windowPx: window.innerWidth, floors: paneFloors(), dividerPx: dividerBandPx() };
  const previous = layoutGeometry.peek();
  if (previous && sameGeometry(previous, next)) return;
  redecideYields(next, heldYields.peek());
}

const RESIZING_ATTR = 'data-pane-resizing';

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

export function endPaneResize(): void {
  redecideYields(layoutGeometry.peek(), UNHELD);
  document.documentElement.removeAttribute(RESIZING_ATTR);
}
