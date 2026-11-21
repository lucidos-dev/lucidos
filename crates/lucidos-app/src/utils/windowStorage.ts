/**
 * Window-scoped storage (`docs/glossary.md` § window-scoped storage).
 *
 * Navigation belongs to a window, not to the device. Each window keeps its own
 * copy of a navigation key under `<key>@window:<id>`, so a reload or a relaunch
 * restores what THAT window showed. Every write also lands on the plain key, the
 * shared seed, which a window with no state of its own starts from.
 *
 * A removal writes an empty own value rather than deleting it. Otherwise a
 * window that went back to Home would fall through to another window's seed.
 *
 * Keys stay workspace-namespaced: `installWorkspaceStorage` overrides the shared
 * `Storage.prototype`, which covers `sessionStorage` and every key here.
 */

import { currentWindowLabel } from './tauri';
import { generateUuid } from './uuid';
import { TAB_WINDOW_ID_KEY } from './windowIdKey';


/** When each window id last booted, and which keys it wrote. */
export const WINDOW_LAST_SEEN_KEY = 'lucidos-window-last-seen';

/** A window unseen this long has closed for good, and its keys are removed. */
export const WINDOW_STATE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

/** How often a window in use re-stamps itself as seen. */
const SEEN_REFRESH_MS = 60 * 60 * 1000;

type LastSeen = Record<string, { seen: number; keys: string[] }>;

let cachedWindowId: string | null = null;

/** This window's stable id: its Tauri label on the desktop, else a per-tab id
 *  held in `sessionStorage`, which a reload and a browser session restore keep. */
export function windowId(): string {
  if (cachedWindowId) return cachedWindowId;
  cachedWindowId = currentWindowLabel() ?? tabWindowId();
  return cachedWindowId;
}

/** Give this tab a new window id, carrying its own values over. For a
 *  duplicated tab, which copied the original's id. A desktop label is unique,
 *  so it is never reassigned. Returns whether the id changed. */
export function reassignWindowId(): boolean {
  if (currentWindowLabel() !== null) return false;
  const previous = windowId();
  const keys = [...noted()];
  const minted = generateUuid();
  try {
    sessionStorage.setItem(TAB_WINDOW_ID_KEY, minted);
  } catch { /* no sessionStorage: the new id still holds for this page */ }
  cachedWindowId = minted;
  seenStampedAt = 0;
  try {
    for (const key of keys) {
      const value = localStorage.getItem(ownKey(key, previous));
      if (value !== null) localStorage.setItem(ownKey(key), value);
    }
    stampSeen(Date.now());
  } catch { /* storage full or unavailable: the tab starts from the seed */ }
  return true;
}

function tabWindowId(): string {
  try {
    const saved = sessionStorage.getItem(TAB_WINDOW_ID_KEY);
    if (saved) return saved;
    const minted = generateUuid();
    sessionStorage.setItem(TAB_WINDOW_ID_KEY, minted);
    return minted;
  } catch {
    // No sessionStorage: the id still scopes this page's writes, and a reload
    // starts from the seed.
    return generateUuid();
  }
}

function ownKey(key: string, id = windowId()): string {
  return `${key}@window:${id}`;
}

/** This window's value for `key`, else the shared seed. */
export function readWindowItem(key: string): string | null {
  try {
    const own = localStorage.getItem(ownKey(key));
    if (own !== null) return own === '' ? null : own;
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function writeWindowItem(key: string, value: string): void {
  try {
    localStorage.setItem(ownKey(key), value);
    localStorage.setItem(key, value);
    noteKey(key);
  } catch { /* storage full or unavailable: navigation still works */ }
}

export function removeWindowItem(key: string): void {
  try {
    localStorage.setItem(ownKey(key), '');
    localStorage.removeItem(key);
    noteKey(key);
  } catch { /* storage full or unavailable: navigation still works */ }
}

/** Forget this window's value without touching the seed another window wrote. */
export function clearOwnWindowItem(key: string): void {
  try {
    localStorage.setItem(ownKey(key), '');
    noteKey(key);
  } catch { /* storage full or unavailable: navigation still works */ }
}

/** The shared seed alone: the last value any window wrote. */
export function readSeedItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function readLastSeen(): LastSeen {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY) ?? '{}');
    return parsed && typeof parsed === 'object' ? (parsed as LastSeen) : {};
  } catch {
    return {};
  }
}

function writeLastSeen(lastSeen: LastSeen): void {
  localStorage.setItem(WINDOW_LAST_SEEN_KEY, JSON.stringify(lastSeen));
}

let notedKeys: Set<string> | null = null;
let seenStampedAt = 0;

function noted(): Set<string> {
  notedKeys ??= new Set(readLastSeen()[windowId()]?.keys ?? []);
  return notedKeys;
}

function stampSeen(now: number): void {
  const lastSeen = readLastSeen();
  lastSeen[windowId()] = { seen: now, keys: [...noted()] };
  writeLastSeen(lastSeen);
  seenStampedAt = now;
}

function noteKey(key: string): void {
  const keys = noted();
  const now = Date.now();
  if (keys.has(key) && now - seenStampedAt < SEEN_REFRESH_MS) return;
  keys.add(key);
  stampSeen(now);
}

/** Re-stamp this window as seen, at most hourly, so a window left open for
 *  weeks is not pruned while it is still in use. */
export function touchWindowSeen(): void {
  const now = Date.now();
  if (now - seenStampedAt < SEEN_REFRESH_MS) return;
  try {
    stampSeen(now);
  } catch { /* best effort: a missed stamp only risks an early prune */ }
}

/** Stamp this window as seen, and remove the keys of windows gone longer than
 *  {@link WINDOW_STATE_TTL_MS}. Called once at boot. */
export function pruneClosedWindows(now = Date.now()): void {
  try {
    const lastSeen = readLastSeen();
    const self = windowId();
    for (const [id, entry] of Object.entries(lastSeen)) {
      if (id === self || now - entry.seen < WINDOW_STATE_TTL_MS) continue;
      for (const key of entry.keys) localStorage.removeItem(ownKey(key, id));
      delete lastSeen[id];
    }
    lastSeen[self] = { seen: now, keys: lastSeen[self]?.keys ?? [] };
    writeLastSeen(lastSeen);
    seenStampedAt = now;
  } catch { /* best effort: a stale key only costs a few bytes */ }
}

/** Forget the cached id and noted keys. Tests only. */
export function _resetWindowStorageForTests(): void {
  cachedWindowId = null;
  notedKeys = null;
  seenStampedAt = 0;
}
