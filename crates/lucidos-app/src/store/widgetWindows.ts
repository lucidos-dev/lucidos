/** Widget windows (ADR 0419): widgets floating over the app, opened from
 *  Home's long-press menu. Several can be open, each where the user left it,
 *  and they stay open until their own Close.
 *
 *  Both the open set and each window's layout are per device, so they live in
 *  `localStorage` and survive a reload. Close keeps the layout, so a widget
 *  opened again comes back as it was. Each entry names its thread, so a
 *  workspace sharing the origin never draws another's. */

import { signal } from '@preact/signals';
import { viewportIsMobile } from '../utils/viewport';

/** How a window draws (ADR 0419). `minimal` is the widget alone, shaped as it
 *  draws itself. `window` is a card with a bar, at a size the user sets.
 *  `docked` snaps it under the thread's title row. */
export type WidgetWindowMode = 'minimal' | 'window' | 'docked';

export const WIDGET_WINDOW_MODES: readonly WidgetWindowMode[] = ['minimal', 'window', 'docked'];

/** An open window. */
export interface WidgetWindow {
  threadId: string;
  /** The widget instance's key (`threadWidgetKey`), since one widget can be
   *  pinned twice with different params (ADR 0415). */
  instanceKey: string;
  /** Higher is nearer the front. */
  raise: number;
}

export interface WindowSize {
  width: number;
  height: number;
}

/** Where and how a window draws, remembered across Close. */
export interface WidgetWindowLayout {
  /** Top-left corner, in viewport CSS pixels, as last placed. A docked window
   *  keeps it for when it floats again. */
  x: number;
  y: number;
  mode: WidgetWindowMode;
  /** Window mode's size, once known. */
  size?: WindowSize;
}

interface StoredLayout extends WidgetWindowLayout {
  threadId: string;
  instanceKey: string;
}

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

const OPEN_KEY = 'lucidos-widget-windows';
const LAYOUT_KEY = 'lucidos-widget-window-layouts';

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object';

function isWidgetWindow(value: unknown): value is WidgetWindow {
  return isRecord(value) && typeof value.threadId === 'string' && typeof value.instanceKey === 'string'
    && typeof value.raise === 'number';
}

function isSize(value: unknown): value is WindowSize {
  return isRecord(value) && typeof value.width === 'number' && typeof value.height === 'number';
}

function isStoredLayout(value: unknown): value is StoredLayout {
  return isRecord(value) && typeof value.threadId === 'string' && typeof value.instanceKey === 'string'
    && typeof value.x === 'number' && typeof value.y === 'number'
    && WIDGET_WINDOW_MODES.includes(value.mode as WidgetWindowMode)
    && (value.size === undefined || isSize(value.size));
}

function readStored<T>(key: string, isEntry: (value: unknown) => value is T): T[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter(isEntry) : [];
  } catch {
    // A corrupt entry costs the layout, not the app: start with no windows.
    return [];
  }
}

/** In the order they were opened. Raising a window never reorders this list:
 *  moving an iframe in the DOM reloads it. */
export const widgetWindows = signal<readonly WidgetWindow[]>(readStored(OPEN_KEY, isWidgetWindow));

/** Every window's remembered layout, open or closed. */
export const widgetWindowLayouts = signal<readonly StoredLayout[]>(readStored(LAYOUT_KEY, isStoredLayout));

function setWindows(next: readonly WidgetWindow[]): void {
  widgetWindows.value = next;
  localStorage.setItem(OPEN_KEY, JSON.stringify(next));
}

function setLayouts(next: readonly StoredLayout[]): void {
  widgetWindowLayouts.value = next;
  localStorage.setItem(LAYOUT_KEY, JSON.stringify(next));
}

const isWindow = (threadId: string, instanceKey: string) => (w: { threadId: string; instanceKey: string }) =>
  w.threadId === threadId && w.instanceKey === instanceKey;

/** The mode a window opens in before the user picks one: docked under the
 *  title on a phone, where a floating window covers too much. */
export function defaultWidgetWindowMode(): WidgetWindowMode {
  return viewportIsMobile.value ? 'docked' : 'minimal';
}

/** A window's layout. One never placed sits at the bounds' corner, which the
 *  view's clamp resolves. */
export function widgetWindowLayout(threadId: string, instanceKey: string): WidgetWindowLayout {
  const stored = widgetWindowLayouts.value.find(isWindow(threadId, instanceKey));
  if (!stored) return { x: 0, y: 0, mode: defaultWidgetWindowMode() };
  const { x, y, mode, size } = stored;
  return size ? { x, y, mode, size } : { x, y, mode };
}

function patchLayout(threadId: string, instanceKey: string, patch: Partial<WidgetWindowLayout>): void {
  const match = isWindow(threadId, instanceKey);
  const current = widgetWindowLayout(threadId, instanceKey);
  const next = { threadId, instanceKey, ...current, ...patch };
  const rest = widgetWindowLayouts.value.filter((l) => !match(l));
  setLayouts([...rest, next]);
}

function nextRaise(): number {
  return widgetWindows.value.reduce((top, w) => Math.max(top, w.raise), 0) + 1;
}

function bringToFront(threadId: string, instanceKey: string): void {
  const match = isWindow(threadId, instanceKey);
  const raise = nextRaise();
  setWindows(widgetWindows.value.map((w) => (match(w) ? { ...w, raise } : w)));
}

/** Open a widget instance's window where it was last left. One never opened
 *  before goes at `at`, stepped by `stepPx` per window already open, so a new
 *  one never hides the last. An open one comes to the front instead, which
 *  also cancels a close still fading out. */
export function openWidgetWindow(threadId: string, instanceKey: string, at: { x: number; y: number }, stepPx: number): void {
  if (widgetWindows.value.some(isWindow(threadId, instanceKey))) {
    bringToFront(threadId, instanceKey);
    return;
  }
  if (!widgetWindowLayouts.value.some(isWindow(threadId, instanceKey))) {
    const offset = widgetWindows.value.filter((w) => w.threadId === threadId).length * stepPx;
    patchLayout(threadId, instanceKey, { x: at.x + offset, y: at.y + offset });
  }
  setWindows([...widgetWindows.value, { threadId, instanceKey, raise: nextRaise() }]);
}

/** A press on a window. One already at the front stays as it is. */
export function raiseWidgetWindow(threadId: string, instanceKey: string): void {
  const target = widgetWindows.value.find(isWindow(threadId, instanceKey));
  if (target && target.raise !== nextRaise() - 1) bringToFront(threadId, instanceKey);
}

export function moveWidgetWindow(threadId: string, instanceKey: string, x: number, y: number): void {
  patchLayout(threadId, instanceKey, { x, y });
}

export function resizeWidgetWindow(threadId: string, instanceKey: string, size: WindowSize): void {
  patchLayout(threadId, instanceKey, { size });
}

export function setWidgetWindowMode(threadId: string, instanceKey: string, mode: WidgetWindowMode): void {
  patchLayout(threadId, instanceKey, { mode });
}

/** Shut a window. Its layout stays for the next open. */
export function closeWidgetWindow(threadId: string, instanceKey: string): void {
  const match = isWindow(threadId, instanceKey);
  setWindows(widgetWindows.value.filter((w) => !match(w)));
}

/** Shut a window and drop its layout, for a widget unpinned from Home. */
export function forgetWidgetWindow(threadId: string, instanceKey: string): void {
  closeWidgetWindow(threadId, instanceKey);
  const match = isWindow(threadId, instanceKey);
  setLayouts(widgetWindowLayouts.value.filter((l) => !match(l)));
}

/** Each window's place in the stack, 0 at the back. A rank rather than the
 *  raise counter keeps every window inside its z-index band. */
export function stackRanks(windows: readonly WidgetWindow[]): Map<string, number> {
  const byRaise = [...windows].sort((a, b) => a.raise - b.raise);
  return new Map(byRaise.map((w, rank) => [w.instanceKey, rank]));
}

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), Math.max(lo, hi));

/** Where a window of `size` placed at `at` draws inside `bounds`. Its whole
 *  width and its bar stay inside, so it can always be dragged back. */
export function clampWindowPosition(
  at: { x: number; y: number },
  size: { width: number; barHeight: number },
  bounds: Box,
): { x: number; y: number } {
  return {
    x: clamp(at.x, bounds.left, bounds.right - size.width),
    y: clamp(at.y, bounds.top, bounds.bottom - size.barHeight),
  };
}

/** A window mode size inside `bounds` from its corner at `at`, and never
 *  under `min`. */
export function clampWindowSize(size: WindowSize, at: { x: number; y: number }, bounds: Box, min: WindowSize): WindowSize {
  return {
    width: Math.round(clamp(size.width, min.width, bounds.right - at.x)),
    height: Math.round(clamp(size.height, min.height, bounds.bottom - at.y)),
  };
}
