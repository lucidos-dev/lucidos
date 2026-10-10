// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clampWindowPosition,
  closeWidgetWindow,
  moveWidgetWindow,
  openWidgetWindow,
  raiseWidgetWindow,
  stackRanks,
  widgetWindows,
} from './widgetWindows';

const at = { x: 10, y: 60 };

beforeEach(() => {
  localStorage.clear();
  widgetWindows.value = [];
});

describe('widget windows', () => {
  it('opens several, each stepped off the last', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    expect(widgetWindows.value.map((w) => [w.appId, w.x, w.y])).toEqual([['fares', 10, 60], ['weather', 34, 84]]);
  });

  it('raises an open widget rather than opening a second window for it', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    openWidgetWindow('home', 'fares', at, 24);
    expect(widgetWindows.value).toHaveLength(2);
    expect(stackRanks(widgetWindows.value).get('fares')).toBe(1);
  });

  it('raises without reordering the list, so no frame moves in the DOM', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    raiseWidgetWindow('home', 'fares');
    expect(widgetWindows.value.map((w) => w.appId)).toEqual(['fares', 'weather']);
    expect(stackRanks(widgetWindows.value)).toEqual(new Map([['weather', 0], ['fares', 1]]));
  });

  it('moves and closes one window, leaving the others', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    moveWidgetWindow('home', 'weather', 200, 300);
    closeWidgetWindow('home', 'fares');
    expect(widgetWindows.value.map((w) => [w.appId, w.x, w.y])).toEqual([['weather', 200, 300]]);
  });

  it('survives a reload', async () => {
    openWidgetWindow('home', 'fares', at, 24);
    moveWidgetWindow('home', 'fares', 120, 140);
    vi.resetModules();
    const reloaded = await import('./widgetWindows');
    expect(reloaded.widgetWindows.value).toEqual([{ threadId: 'home', appId: 'fares', x: 120, y: 140, raise: 1 }]);
  });

  it('starts empty from a corrupt store', async () => {
    localStorage.setItem('lucidos-widget-windows', '{not json');
    vi.resetModules();
    const reloaded = await import('./widgetWindows');
    expect(reloaded.widgetWindows.value).toEqual([]);
  });
});

describe('clampWindowPosition', () => {
  const bounds = { left: 0, top: 50, right: 400, bottom: 800 };
  const size = { width: 300, barHeight: 40 };

  it('keeps a window where it was put when it fits', () => {
    expect(clampWindowPosition({ x: 20, y: 100 }, size, bounds)).toEqual({ x: 20, y: 100 });
  });

  it('keeps the whole width and the bar on screen, below the header', () => {
    expect(clampWindowPosition({ x: 390, y: 10 }, size, bounds)).toEqual({ x: 100, y: 50 });
    expect(clampWindowPosition({ x: -50, y: 900 }, size, bounds)).toEqual({ x: 0, y: 760 });
  });

  it('pins to the leading edge when the window is wider than the screen', () => {
    expect(clampWindowPosition({ x: 30, y: 100 }, { width: 500, barHeight: 40 }, bounds).x).toBe(0);
  });
});
