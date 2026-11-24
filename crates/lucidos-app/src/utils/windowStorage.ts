
import { currentWindowLabel } from './tauri';
import { generateUuid } from './uuid';
import { TAB_WINDOW_ID_KEY } from './windowIdKey';


export const WINDOW_LAST_SEEN_KEY = 'lucidos-window-last-seen';

export const WINDOW_STATE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

const SEEN_REFRESH_MS = 60 * 60 * 1000;

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

function ownKey(key: string, id = windowId()): string {
  return `${key}@window:${id}`;
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
    localStorage.setItem(ownKey(key), value);
    localStorage.setItem(key, value);
    noteKey(key);
  } catch {  }
}

export function removeWindowItem(key: string): void {
  try {
    localStorage.setItem(ownKey(key), '');
    localStorage.removeItem(key);
    noteKey(key);
  } catch {  }
}

export function clearOwnWindowItem(key: string): void {
  try {
    localStorage.setItem(ownKey(key), '');
    noteKey(key);
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
  if (keys.has(key) && now - seenStampedAt < SEEN_REFRESH_MS) return;
  keys.add(key);
  stampSeen(now);
}

export function touchWindowSeen(): void {
  const now = Date.now();
  if (now - seenStampedAt < SEEN_REFRESH_MS) return;
  try {
    stampSeen(now);
  } catch {  }
}

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
  } catch {  }
}

export function _resetWindowStorageForTests(): void {
  cachedWindowId = null;
  notedKeys = null;
  seenStampedAt = 0;
}
