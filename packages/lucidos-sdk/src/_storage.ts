/**
 * Per-workspace browser-storage namespacing for the SDK (iframe realm).
 *
 * App iframes are same-origin but a SEPARATE JS realm, so the parent app's
 * `Storage.prototype` override
 * (`crates/lucidos-app/src/utils/workspaceStorage.ts`) does NOT apply here.
 * Every storage key the SDK touches is per-workspace (theme, fonts, scale,
 * device id, per-app scroll), so it must be read/written under
 * `ws:<slug>:<key>` — the SAME namespace the parent writes — or a parent write
 * won't match an iframe read.
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

/** Read a per-workspace localStorage value (namespaced). Null if unavailable. */
export function wsLocalGet(key: string): string | null {
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
  try {
    localStorage.removeItem(nsKey(key));
  } catch {
    /* a storage-less realm has nothing to clear */
  }
}

/** Read a per-workspace sessionStorage value (namespaced). Null if unavailable. */
export function wsSessionGet(key: string): string | null {
  try {
    return sessionStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

/** Write a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionSet(key: string, value: string): void {
  sessionStorage.setItem(nsKey(key), value);
}

/** Remove a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionRemove(key: string): void {
  sessionStorage.removeItem(nsKey(key));
}
