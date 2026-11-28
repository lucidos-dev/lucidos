
import { currentWindowLabel } from './tauri';
import { generateUuid } from './uuid';
import { TAB_WINDOW_ID_KEY } from './windowIdKey';
import { shellStorageKeys } from './workspaceStorage';


export const WINDOW_LAST_SEEN_KEY = 'lucidos-window-last-seen';

export const WINDOW_STATE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

export const MAX_REMEMBERED_WINDOWS = 8;

const SEEN_REFRESH_MS = 60 * 60 * 1000;

export const POSSIBLY_OPEN_MS = 2 * SEEN_REFRESH_MS;

type LastSeen = Record<string, { seen: number; keys: string[] }>;

let cachedWindowId: string | null = null;

export function windowId(): string {
  if (cachedWindowId) return cachedWindowId;
  cachedWindowId = currentWindowLabel() ?? tabWindowId();
  return cachedWindowId;
}

export function reassignWindowId(): boolean {
  if (currentWindowLabel() !== null) return false;
  const previous = windowId();
  const keys = [...noted()];
  const minted = generateUuid();
  try {
    sessionStorage.setItem(TAB_WINDOW_ID_KEY, minted);
  } catch {  }
  cachedWindowId = minted;
  seenStampedAt = 0;
  try {
    for (const key of keys) {
      const value = localStorage.getItem(ownKey(key, previous));
      if (value !== null) localStorage.setItem(ownKey(key), value);
    }
    stampSeen(Date.now());
  } catch {  }
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
    return generateUuid();
  }
}

const WINDOW_KEY_MARKER = '@window:';

function ownKey(key: string, id = windowId()): string {
  return `${key}${WINDOW_KEY_MARKER}${id}`;
}

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
    noteKey(key);
    localStorage.setItem(ownKey(key), value);
    localStorage.setItem(key, value);
  } catch {  }
}

export function removeWindowItem(key: string): void {
  try {
    noteKey(key);
    localStorage.setItem(ownKey(key), '');
    localStorage.removeItem(key);
  } catch {  }
}

export function clearOwnWindowItem(key: string): void {
  try {
    noteKey(key);
    localStorage.setItem(ownKey(key), '');
  } catch {  }
}

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
  const known = keys.has(key);
  if (known && now - seenStampedAt < SEEN_REFRESH_MS) return;
  keys.add(key);
  try {
    stampSeen(now);
  } catch (error) {
    if (!known) keys.delete(key);
    throw error;
  }
}

export function touchWindowSeen(): void {
  const now = Date.now();
  if (now - seenStampedAt < SEEN_REFRESH_MS) return;
  try {
    stampSeen(now);
  } catch {  }
}

export function pruneClosedWindows(
  now = Date.now(),
  keep = MAX_REMEMBERED_WINDOWS,
  open: ReadonlySet<string> = new Set(),
): number {
  let pruned = 0;
  try {
    const lastSeen = readLastSeen();
    const self = windowId();
    const prunedIds = new Set<string>();
    Object.entries(lastSeen)
      .filter(([id, entry]) => id !== self && !open.has(id) && now - entry.seen >= POSSIBLY_OPEN_MS)
      .sort(([, a], [, b]) => b.seen - a.seen)
      .forEach(([id, entry], rank) => {
        if (rank < keep - 1 && now - entry.seen < WINDOW_STATE_TTL_MS) return;
        delete lastSeen[id];
        prunedIds.add(id);
      });
    lastSeen[self] = { seen: now, keys: lastSeen[self]?.keys ?? [] };
    const registered = new Set([
      ...open,
      ...[...Object.keys(lastSeen), ...Object.keys(readLastSeen())].filter((id) => !prunedIds.has(id)),
    ]);
    pruned = prunedIds.size + removeUnregisteredWindowKeys(registered, prunedIds);
    writeLastSeen(lastSeen);
    seenStampedAt = now;
  } catch {  }
  return pruned;
}

function removeUnregisteredWindowKeys(registered: Set<string>, countedAsPruned: ReadonlySet<string>): number {
  const orphaned = new Set<string>();
  for (const key of shellStorageKeys(localStorage)) {
    const at = key.lastIndexOf(WINDOW_KEY_MARKER);
    if (at < 0) continue;
    const id = key.slice(at + WINDOW_KEY_MARKER.length);
    if (registered.has(id)) continue;
    localStorage.removeItem(key);
    orphaned.add(id);
  }
  for (const id of countedAsPruned) orphaned.delete(id);
  return orphaned.size;
}

export function _resetWindowStorageForTests(): void {
  cachedWindowId = null;
  notedKeys = null;
  seenStampedAt = 0;
}
