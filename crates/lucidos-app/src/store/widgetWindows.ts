/** Widget windows (ADR 0419): widgets floating over the app, opened from
 *  Home's long-press menu. Several can be open, each where the user dragged
 *  it, and they stay open until their own Close.
 *
 *  The open set is per device, so it lives in `localStorage` and survives a
 *  reload. Each window names its thread, so a workspace sharing the origin
 *  never draws another's. */

import { signal } from '@preact/signals';

export interface WidgetWindow {
  threadId: string;
  /** The widget instance's key (`threadWidgetKey`), since one widget can be
   *  pinned twice with different params (ADR 0415). */
  instanceKey: string;
  /** Top-left corner, in viewport CSS pixels, as last placed. */
  x: number;
  y: number;
  /** Higher is nearer the front. */
  raise: number;
}

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

const STORAGE_KEY = 'lucidos-widget-windows';

function isWidgetWindow(value: unknown): value is WidgetWindow {
  if (!value || typeof value !== 'object') return false;
  const w = value as Record<string, unknown>;
  return typeof w.threadId === 'string' && typeof w.instanceKey === 'string'
    && typeof w.x === 'number' && typeof w.y === 'number' && typeof w.raise === 'number';
}

function readStored(): WidgetWindow[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter(isWidgetWindow) : [];
  } catch {
    // A corrupt entry costs the layout, not the app: start with no windows.
    return [];
  }
}

/** In the order they were opened. Raising a window never reorders this list:
 *  moving an iframe in the DOM reloads it. */
export const widgetWindows = signal<readonly WidgetWindow[]>(readStored());

function setWindows(next: readonly WidgetWindow[]): void {
  widgetWindows.value = next;
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
}

const isWindow = (threadId: string, instanceKey: string) => (w: WidgetWindow) =>
  w.threadId === threadId && w.instanceKey === instanceKey;

function nextRaise(): number {
  return widgetWindows.value.reduce((top, w) => Math.max(top, w.raise), 0) + 1;
}

function bringToFront(threadId: string, instanceKey: string): void {
  const match = isWindow(threadId, instanceKey);
  const raise = nextRaise();
  setWindows(widgetWindows.value.map((w) => (match(w) ? { ...w, raise } : w)));
}

/** Open a widget instance's window at `at`, stepped by `stepPx` per window
 *  already open, so a new one never hides the last. An open one comes to the
 *  front instead, which also cancels a close still fading out. */
export function openWidgetWindow(threadId: string, instanceKey: string, at: { x: number; y: number }, stepPx: number): void {
  if (widgetWindows.value.some(isWindow(threadId, instanceKey))) {
    bringToFront(threadId, instanceKey);
    return;
  }
  const offset = widgetWindows.value.filter((w) => w.threadId === threadId).length * stepPx;
  setWindows([...widgetWindows.value, { threadId, instanceKey, x: at.x + offset, y: at.y + offset, raise: nextRaise() }]);
}

/** A press on a window. One already at the front stays as it is. */
export function raiseWidgetWindow(threadId: string, instanceKey: string): void {
  const target = widgetWindows.value.find(isWindow(threadId, instanceKey));
  if (target && target.raise !== nextRaise() - 1) bringToFront(threadId, instanceKey);
}

export function moveWidgetWindow(threadId: string, instanceKey: string, x: number, y: number): void {
  const match = isWindow(threadId, instanceKey);
  setWindows(widgetWindows.value.map((w) => (match(w) ? { ...w, x, y } : w)));
}

export function closeWidgetWindow(threadId: string, instanceKey: string): void {
  const match = isWindow(threadId, instanceKey);
  setWindows(widgetWindows.value.filter((w) => !match(w)));
}

/** Each window's place in the stack, 0 at the back. A rank rather than the
 *  raise counter keeps every window inside its z-index band. */
export function stackRanks(windows: readonly WidgetWindow[]): Map<string, number> {
  const byRaise = [...windows].sort((a, b) => a.raise - b.raise);
  return new Map(byRaise.map((w, rank) => [w.instanceKey, rank]));
}

/** Where a window of `size` placed at `at` draws inside `bounds`. Its whole
 *  width and its bar stay inside, so it can always be dragged back. */
export function clampWindowPosition(
  at: { x: number; y: number },
  size: { width: number; barHeight: number },
  bounds: Box,
): { x: number; y: number } {
  const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), Math.max(lo, hi));
  return {
    x: clamp(at.x, bounds.left, bounds.right - size.width),
    y: clamp(at.y, bounds.top, bounds.bottom - size.barHeight),
  };
}
