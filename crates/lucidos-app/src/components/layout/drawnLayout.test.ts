// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  splitRatio, SPLIT_RATIO_KEY, threadDrawerOpen, threadDrawerWidth, THREAD_DRAWER_WIDTH_KEY,
} from '../../store/store';
import { homeRailShown, homeRailWidth } from '../../store/homeRail';
import { HOME_RAIL_SHOWN_KEY, HOME_RAIL_WIDTH_KEY } from '../../store/homeRailKeys';
import { paneFloors, dividerBandPx } from '../../store/paneMinimums';
import {
  beginPaneResize, drawnDrawerShown, drawnDrawerWidth, drawnRailShown, drawnRailWidth, drawnSplitRatio,
  endPaneResize, homeRailCanShow, layoutGeometry, splitFitsBoth, threadListCanShow,
} from './drawnLayout';

const FLOORS = paneFloors();
const DIVIDER = dividerBandPx();
const BOTH = FLOORS.minThreadPx + FLOORS.minContentPx;
const DRAWER_ONLY = FLOORS.minThreadPx + FLOORS.drawerPx + DIVIDER;
const BOTH_SIDES = DRAWER_ONLY + FLOORS.railPx;
const ROOMY = BOTH_SIDES + BOTH + 400;

function resize(windowPx: number) {
  layoutGeometry.value = { windowPx, floors: FLOORS, dividerPx: DIVIDER };
}

function store(drawerOpen: boolean, railShown: boolean) {
  threadDrawerOpen.value = drawerOpen;
  homeRailShown.value = railShown;
}

const STORED_KEYS = [SPLIT_RATIO_KEY, THREAD_DRAWER_WIDTH_KEY, HOME_RAIL_SHOWN_KEY, HOME_RAIL_WIDTH_KEY];

describe('the drawn layout signals', () => {
  beforeEach(() => {
    splitRatio.value = 0.5;
    threadDrawerWidth.value = FLOORS.drawerPx + 40;
    homeRailWidth.value = FLOORS.railPx + 40;
    localStorage.setItem(SPLIT_RATIO_KEY, '0.5');
    store(false, false);
    resize(ROOMY);
  });

  afterEach(() => {
    endPaneResize();
    layoutGeometry.value = null;
    store(false, false);
  });

  it('draws the stored layout until the window is measured', () => {
    store(true, true);
    layoutGeometry.value = null;
    expect(drawnSplitRatio.value).toBe(0.5);
    expect(drawnDrawerShown.value).toBe(true);
    expect(drawnRailShown.value).toBe(true);
    expect(drawnDrawerWidth.value).toBe(threadDrawerWidth.value);
    expect(splitFitsBoth()).toBe(true);
    expect(threadListCanShow()).toBe(true);
    expect(homeRailCanShow()).toBe(true);
  });

  it('keeps the ordinary toggles through an unsized first frame', () => {
    store(true, true);
    resize(0);
    expect(drawnSplitRatio.value).toBe(0.5);
    expect(drawnDrawerShown.value).toBe(true);
    expect(splitFitsBoth()).toBe(true);
    expect(threadListCanShow()).toBe(true);
  });

  it('yields the Canvas when the room goes, and brings it back unchanged', () => {
    resize(BOTH - 100);
    expect(drawnSplitRatio.value).toBe(1);
    expect(splitFitsBoth()).toBe(false);
    resize(ROOMY);
    expect(drawnSplitRatio.value).toBe(0.5);
  });

  it('counts the drawn side panes when it decides the Canvas yield', () => {
    store(true, false);
    resize(BOTH + 50);
    expect(drawnSplitRatio.value).toBe(1);
    store(false, false);
    expect(drawnSplitRatio.value).toBe(0.5);
  });

  it('yields the rail before the drawer, and the drawer last', () => {
    store(true, true);
    resize(BOTH_SIDES);
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([true, true]);
    resize(BOTH_SIDES - 1);
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([true, false]);
    resize(DRAWER_ONLY - 1);
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([false, false]);
    expect(drawnSplitRatio.value).toBe(1);
  });

  it('holds the drawer wide enough to leave the Conversation its floor', () => {
    store(true, false);
    threadDrawerWidth.value = 900;
    resize(DRAWER_ONLY + 60);
    expect(drawnDrawerWidth.value).toBe(FLOORS.drawerPx + 60);
  });

  it('holds the rail for the drawn drawer', () => {
    store(true, true);
    threadDrawerWidth.value = FLOORS.drawerPx + 100;
    homeRailWidth.value = 2000;
    resize(ROOMY);
    expect(drawnRailWidth.value).toBe(ROOMY - FLOORS.drawerPx - 100 - DIVIDER - BOTH);
  });

  it('answers whether a yielded pane could show, by its own floors', () => {
    resize(DRAWER_ONLY);
    expect(threadListCanShow()).toBe(true);
    resize(DRAWER_ONLY - 1);
    expect(threadListCanShow()).toBe(false);
    resize(FLOORS.minThreadPx + FLOORS.railPx);
    expect(homeRailCanShow()).toBe(true);
    resize(FLOORS.minThreadPx + FLOORS.railPx - 1);
    expect(homeRailCanShow()).toBe(false);
  });

  it('counts a collapsed Conversation as returning when the drawer shows', () => {
    splitRatio.value = 0;
    resize(DRAWER_ONLY - 1);
    expect(threadListCanShow()).toBe(false);
    resize(DRAWER_ONLY);
    expect(threadListCanShow()).toBe(true);
  });

  it('never writes a stored layout key for lack of room', () => {
    store(true, true);
    const before = STORED_KEYS.map((key) => localStorage.getItem(key));
    const signals = [splitRatio.value, threadDrawerWidth.value, homeRailWidth.value];
    resize(DRAWER_ONLY - 1);
    void [drawnSplitRatio.value, drawnDrawerShown.value, drawnRailShown.value];
    resize(ROOMY);
    expect(STORED_KEYS.map((key) => localStorage.getItem(key))).toEqual(before);
    expect([splitRatio.value, threadDrawerWidth.value, homeRailWidth.value]).toEqual(signals);
    expect([threadDrawerOpen.value, homeRailShown.value]).toEqual([true, true]);
  });

  it('animates a yield decided at release, like a toggle', () => {
    document.body.innerHTML = '<div class="split-layout"></div>';
    beginPaneResize();
    resize(BOTH - 100);
    endPaneResize();
    expect(document.querySelector('.split-layout')!.classList.contains('pane-animate')).toBe(true);
    document.body.innerHTML = '';
  });

  it('animates a side pane that yields for room at release, as the Canvas does', () => {
    document.body.innerHTML = '<div class="split-layout"></div>';
    store(true, true);
    resize(BOTH_SIDES);
    beginPaneResize();
    resize(BOTH_SIDES - 1);
    endPaneResize();
    expect(document.querySelector('.split-layout')!.classList.contains('pane-animate')).toBe(true);
    document.body.innerHTML = '';
  });

  it('holds an open Canvas open through a drag, and yields at release', () => {
    beginPaneResize();
    resize(BOTH - 100);
    const mid = drawnSplitRatio.value;
    expect(mid).toBeGreaterThan(0);
    expect(mid).toBeLessThan(1);
    endPaneResize();
    expect(drawnSplitRatio.value).toBe(1);
  });

  it('holds a yielded Canvas collapsed through a drag, and returns at release', () => {
    resize(BOTH - 100);
    beginPaneResize();
    resize(ROOMY);
    expect(drawnSplitRatio.value).toBe(1);
    endPaneResize();
    expect(drawnSplitRatio.value).toBe(0.5);
  });

  it('holds both side panes drawn through a drag, and yields them at release', () => {
    store(true, true);
    resize(BOTH_SIDES);
    beginPaneResize();
    resize(DRAWER_ONLY - 1);
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([true, true]);
    endPaneResize();
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([false, false]);
  });

  it('holds yielded side panes yielded through a drag, and returns them at release', () => {
    store(true, true);
    resize(DRAWER_ONLY);
    beginPaneResize();
    resize(ROOMY);
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([true, false]);
    endPaneResize();
    expect([drawnDrawerShown.value, drawnRailShown.value]).toEqual([true, true]);
  });
});
