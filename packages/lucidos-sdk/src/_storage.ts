/**
 * Per-workspace browser-storage namespacing for the SDK (iframe realm).
 *
 * The SDK is a SEPARATE JS realm from the host app, so the host's
 * `Storage.prototype` override
 * (`crates/lucidos-app/src/utils/workspaceStorage.ts`) does NOT apply here.
 * Every storage key the SDK touches is per-workspace (theme, fonts, scale,
 * device id, per-app scroll), so it must be read/written under
 * `ws:<slug>:<key>`, the SAME namespace the host writes. Otherwise a host write
 * won't match a standalone app tab's read. An app FRAME reaches no host key at
 * all: its store is the host's `appbridge:` space, primed over the bridge.
 *
 * The slug comes from the SDK base path (`_fetch.ts` `getBaseUrl()`):
 * `/<slug>` behind the gateway, `''` at a legacy direct-engine root, `/~` in the
 * picker (no app iframes there). No slug → raw key (legacy/direct, where there
 * is a single workspace per origin and nothing to isolate).
 *
 * This is the ONLY SDK file allowed to touch `localStorage` / `sessionStorage`
 * directly — the `no-raw-storage` guard test fails the build on any other raw
 * access in the SDK.
 */

import { getBaseUrl } from './_fetch';
import { callHost, isBridged, tellHost } from './_bridge';

/** The workspace slug this iframe runs under, or null (picker / legacy root). */
function workspaceSlug(): string | null {
  const base = getBaseUrl(); // '' | '/<slug>' | '/~'
  if (!base) return null;
  const seg = base.replace(/^\/+|\/+$/g, '');
  return seg === '' || seg === '~' ? null : seg;
}

/** `ws:<slug>:<key>` in a workspace, else the raw key (picker / legacy). */
function nsKey(key: string): string {
  const slug = workspaceSlug();
  return slug ? `ws:${slug}:${key}` : key;
}

/**
 * What the host holds for this frame, when the frame cannot read storage.
 *
 * An isolated frame's own `localStorage` throws, and the bridge is async where
 * every reader below is synchronous. So the whole of this frame's store is
 * fetched once by [`primeBridgedStorage`] and served from here. A write updates
 * the mirror at once and tells the host after, so a read never waits.
 */
const mirror = new Map<string, string>();

/** Mirror key. The two stores are separate, and merging them would let a
 *  session write answer a local read under the same name. */
function mirrorKey(key: string, session: boolean): string {
  return `${session ? 'session' : 'local'}:${key}`;
}

/**
 * Fetch this frame's stored values before anything reads them.
 *
 * Called once at SDK load, beside `primeDevicePreferences`, and awaited by the
 * one reader whose answer must be right on the first try: scroll memory. A
 * failure leaves the mirror empty, which reads as "nothing stored" and is what
 * a first visit looks like anyway.
 */
export async function primeBridgedStorage(): Promise<void> {
  if (!isBridged()) return;
  // Keyed by store on the host side too, so the two arrive already apart.
  const entries = await callHost('storage.prime', {}) as Record<string, string> | null;
  for (const [key, value] of Object.entries(entries ?? {})) mirror.set(key, value);
}

function bridgedGet(key: string, session: boolean): string | null {
  return mirror.get(mirrorKey(key, session)) ?? null;
}

function bridgedSet(key: string, value: string, session: boolean): void {
  mirror.set(mirrorKey(key, session), value);
  tellHost('storage.set', { key, value, session });
}

function bridgedRemove(key: string, session: boolean): void {
  mirror.delete(mirrorKey(key, session));
  tellHost('storage.remove', { key, session });
}

/** Read a per-workspace localStorage value (namespaced). Null if unavailable. */
export function wsLocalGet(key: string): string | null {
  if (isBridged()) return bridgedGet(nsKey(key), false);
  try {
    return localStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

/** The storage key the parent app mints this workspace's device id under. */
const DEVICE_ID_KEY = 'lucidos-device-id';

/**
 * The device id the parent app minted for this workspace, or null.
 *
 * Two readers, which is why it sits here rather than in either of them.
 * `preferences` scopes its read by it, and `_fetch` sends it on every request
 * so the engine can attribute what an app changed.
 */
export function wsDeviceId(): string | null {
  return wsLocalGet(DEVICE_ID_KEY);
}

/** Remove a per-workspace localStorage value (namespaced). Best-effort: used by
 *  the boot script's `?style-reset` escape hatch, which must work even when the
 *  UI it is rescuing does not. */
export function wsLocalRemove(key: string): void {
  if (isBridged()) return bridgedRemove(nsKey(key), false);
  try {
    localStorage.removeItem(nsKey(key));
  } catch {
    /* a storage-less realm has nothing to clear */
  }
}

/** Read a per-workspace sessionStorage value (namespaced). Null if unavailable. */
export function wsSessionGet(key: string): string | null {
  if (isBridged()) return bridgedGet(nsKey(key), true);
  try {
    return sessionStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

/** Write a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionSet(key: string, value: string): void {
  if (isBridged()) return bridgedSet(nsKey(key), value, true);
  sessionStorage.setItem(nsKey(key), value);
}

/** Remove a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionRemove(key: string): void {
  if (isBridged()) return bridgedRemove(nsKey(key), true);
  sessionStorage.removeItem(nsKey(key));
}

/** Test-only: empty the bridged mirror so module state cannot leak between
 *  cases. Not part of the runtime surface. */
export function _resetBridgedStorageForTesting(): void {
  mirror.clear();
}
