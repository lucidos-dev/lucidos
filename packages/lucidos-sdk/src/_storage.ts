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
