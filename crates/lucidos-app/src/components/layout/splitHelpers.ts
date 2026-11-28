import { splitRatio, scaledDurationMs, SPLIT_RATIO_KEY } from '../../store/store';
import type { PaneFloors, SplitBounds } from '../../store/paneMinimums';

export const DEFAULT_SPLIT_RATIO = 0.4;


export const PANE_TRANSITION_MS = 300;

export function clampToRange(value: number, lo: number, hi: number): number {
  if (hi < lo) return lo;
  return Math.min(Math.max(value, lo), hi);
}

export function clampSplitRatio(pointerPx: number, totalPx: number, bounds: SplitBounds): number {
  if (!splitIsSized(totalPx)) return DEFAULT_SPLIT_RATIO;
  const threadPx = clampToRange(pointerPx, bounds.minThreadPx, totalPx - bounds.minContentPx);
  return clampToRange(threadPx, 1, totalPx - 1) / totalPx;
}

export function splitIsSized(totalPx: number): boolean {
  return totalPx > 2;
}

export function splitFitsBothPanes(totalPx: number, bounds: SplitBounds): boolean {
  return totalPx >= bounds.minThreadPx + bounds.minContentPx;
}

export function computeDrawnSplitRatio(
  stored: number, totalPx: number, bounds: SplitBounds, heldYield: boolean | null,
): number {
  if (stored <= 0) return 0;
  if (stored >= 1) return 1;
  if (!splitIsSized(totalPx)) return stored;
  const yields = heldYield ?? !splitFitsBothPanes(totalPx, bounds);
  return yields ? 1 : clampSplitRatio(stored * totalPx, totalPx, bounds);
}

export function contentShownRatio(fitsBoth: boolean): number {
  return fitsBoth ? DEFAULT_SPLIT_RATIO : 0;
}

export function toggleThreadPaneRatio(ratio: number): number {
  return ratio === 0 ? DEFAULT_SPLIT_RATIO : 0;
}

export function toggleContentPaneRatio(ratio: number, fitsBoth: boolean): number {
  return ratio >= 1 ? contentShownRatio(fitsBoth) : 1;
}

export const KEYBOARD_RESIZE_STEP_PX = 80;

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

export function computeDrawerStepWidth(
  width: number, deltaPx: number, minPx: number, maxPx: number,
): number | null {
  if (maxPx < minPx) return null;
  const next = Math.min(Math.max(width + deltaPx, minPx), maxPx);
  return next === width ? null : next;
}

export function dashboardRailCeiling(roomPx: number, threadPx: number, ratio: number, bounds: SplitBounds): number {
  if (ratio <= 0) return roomPx - bounds.minContentPx;
  if (ratio >= 1) return roomPx - bounds.minThreadPx;
  return roomPx - threadPx - bounds.minContentPx;
}

export interface LayoutRoom {
  windowPx: number;
  floors: PaneFloors;
  dividerPx: number;
  storedRatio: number;
}

export interface SidePaneState {
  wanted: boolean;
  widthPx: number;
}

export interface HeldSideYields {
  drawer: boolean | null;
  rail: boolean | null;
}

export interface DrawnSidePane {
  drawn: boolean;
  widthPx: number;
}

export interface DrawnLayout {
  drawer: DrawnSidePane;
  rail: DrawnSidePane;
  splitPx: number;
}

function keptFloorPx(room: LayoutRoom): number {
  return room.storedRatio <= 0 ? room.floors.minContentPx : room.floors.minThreadPx;
}

export function sidePanesFit(room: LayoutRoom, drawer: boolean, rail: boolean): boolean {
  const { floors, dividerPx } = room;
  const neededPx = keptFloorPx(room) + (drawer ? floors.drawerPx + dividerPx : 0) + (rail ? floors.railPx : 0);
  return neededPx <= room.windowPx;
}

function fittingSidePanes(room: LayoutRoom, wantDrawer: boolean, wantRail: boolean): [boolean, boolean] {
  const sets: [boolean, boolean][] = [[wantDrawer, wantRail], [wantDrawer, false], [false, wantRail]];
  return sets.find(([drawer, rail]) => sidePanesFit(room, drawer, rail)) ?? [false, false];
}

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

export function setSplitRatio(newRatio: number) {
  triggerPaneAnimate();
  splitRatio.value = newRatio;
  localStorage.setItem(SPLIT_RATIO_KEY, String(newRatio));
}
