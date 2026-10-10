// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { viewportIsMobile } from '../utils/viewport';
import {
  clampWindowPosition,
  clampWindowSize,
  closeWidgetWindow,
  forgetWidgetWindow,
  moveWidgetWindow,
  openWidgetWindow,
  raiseWidgetWindow,
  resizeWidgetWindow,
  setWidgetWindowMode,
  stackRanks,
  widgetWindowLayout,
  widgetWindowLayouts,
  widgetWindows,
} from './widgetWindows';

const at = { x: 10, y: 60 };

const placed = (key: string) => {
  const { x, y } = widgetWindowLayout('home', key);
  return [key, x, y];
};

beforeEach(() => {
  localStorage.clear();
  widgetWindows.value = [];
  widgetWindowLayouts.value = [];
  viewportIsMobile.value = false;
});

describe('widget windows', () => {
  it('opens several, each stepped off the last', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    expect(['fares', 'weather'].map(placed)).toEqual([['fares', 10, 60], ['weather', 34, 84]]);
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
    expect(widgetWindows.value.map((w) => w.instanceKey)).toEqual(['fares', 'weather']);
    expect(stackRanks(widgetWindows.value)).toEqual(new Map([['weather', 0], ['fares', 1]]));
  });

  it('moves and closes one window, leaving the others', () => {
    openWidgetWindow('home', 'fares', at, 24);
    openWidgetWindow('home', 'weather', at, 24);
    moveWidgetWindow('home', 'weather', 200, 300);
    closeWidgetWindow('home', 'fares');
    expect(widgetWindows.value.map((w) => w.instanceKey)).toEqual(['weather']);
    expect(placed('weather')).toEqual(['weather', 200, 300]);
  });

  it('survives a reload', async () => {
    openWidgetWindow('home', 'fares', at, 24);
    moveWidgetWindow('home', 'fares', 120, 140);
    vi.resetModules();
    const reloaded = await import('./widgetWindows');
    expect(reloaded.widgetWindows.value).toEqual([{ threadId: 'home', instanceKey: 'fares', raise: 1 }]);
    expect(reloaded.widgetWindowLayout('home', 'fares')).toEqual({ x: 120, y: 140, mode: 'minimal' });
  });

  it('opens minimal on a desktop and docked on a phone', () => {
    openWidgetWindow('home', 'fares', at, 24);
    viewportIsMobile.value = true;
    openWidgetWindow('home', 'weather', at, 24);
    expect(widgetWindowLayout('home', 'fares').mode).toBe('minimal');
    expect(widgetWindowLayout('home', 'weather').mode).toBe('docked');
  });

  it('comes back where it was left, in its mode and size, after a close', () => {
    openWidgetWindow('home', 'fares', at, 24);
    moveWidgetWindow('home', 'fares', 200, 300);
    setWidgetWindowMode('home', 'fares', 'window');
    resizeWidgetWindow('home', 'fares', { width: 400, height: 260 });
    closeWidgetWindow('home', 'fares');
    openWidgetWindow('home', 'weather', at, 24);
    openWidgetWindow('home', 'fares', at, 24);
    expect(widgetWindowLayout('home', 'fares')).toEqual({ x: 200, y: 300, mode: 'window', size: { width: 400, height: 260 } });
  });

  it('forgets the layout of a widget unpinned from Home', () => {
    openWidgetWindow('home', 'fares', at, 24);
    moveWidgetWindow('home', 'fares', 200, 300);
    forgetWidgetWindow('home', 'fares');
    expect(widgetWindows.value).toEqual([]);
    openWidgetWindow('home', 'fares', at, 24);
    expect(placed('fares')).toEqual(['fares', 10, 60]);
  });

  it('reads a window stored before modes existed as open, at the default mode', async () => {
    localStorage.setItem('lucidos-widget-windows', JSON.stringify([{ threadId: 'home', instanceKey: 'fares', x: 5, y: 6, raise: 1 }]));
    vi.resetModules();
    const reloaded = await import('./widgetWindows');
    expect(reloaded.widgetWindows.value.map((w) => w.instanceKey)).toEqual(['fares']);
    expect(reloaded.widgetWindowLayout('home', 'fares').mode).toBe('minimal');
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

describe('clampWindowSize', () => {
  const bounds = { left: 0, top: 50, right: 400, bottom: 800 };
  const min = { width: 160, height: 100 };

  it('keeps a size that fits', () => {
    expect(clampWindowSize({ width: 300, height: 200 }, { x: 20, y: 100 }, bounds, min)).toEqual({ width: 300, height: 200 });
  });

  it('stops at the bounds from the window\'s corner, and never under the minimum', () => {
    expect(clampWindowSize({ width: 900, height: 900 }, { x: 100, y: 100 }, bounds, min)).toEqual({ width: 300, height: 700 });
    expect(clampWindowSize({ width: 10, height: 10 }, { x: 100, y: 100 }, bounds, min)).toEqual(min);
  });
});
