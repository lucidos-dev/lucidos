/**
 * Adopt a device id handed over in the URL, so the frontend preview looks like
 * the app the user was just in.
 *
 * The preview runs on its own port, which is its own ORIGIN, and `localStorage`
 * is per-origin. So a preview mints a fresh device id, registers as a new
 * device, and renders with none of that phone's device-scoped preferences: on
 * this workspace that means UI scale, among others. For a surface whose entire
 * job is showing the user what a UI change looks like, rendering it at a
 * different scale than their real app is the one thing it must not do.
 *
 * The fix is one parameter. The preview link carries `?device-id=<uuid>`, this
 * runs before anything reads the id, and the preview resolves the same
 * preferences the real app does.
 *
 * **Only on a dev-server bundle.** A built bundle ignores the parameter
 * entirely, so this is not a way to set the device id of the real app. That is
 * not a security boundary and does not pretend to be one: the id already rides
 * every request as a plain `x-lucidos-device-id` header that any client can
 * set, and the engine has no auth by design (ADR 0014 §9). The gate is about
 * scope, keeping a preview affordance out of the shipped app.
 */

import { DEVICE_ID_KEY } from './deviceIdHeader';
import { isDevServerBundle } from './devServerBundle';

/** URL parameter carrying the device id to adopt. */
export const DEVICE_ID_PARAM = 'device-id';

/** Canonical UUID form. A malformed value is dropped rather than stored: the id
 *  reaches the engine on every request and is a primary key there. */
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * The device id to adopt from a query string, or `null` for "leave it alone".
 * Pure, so every rejection is testable without a DOM.
 */
export function deviceIdToAdopt(search: string, devServerBundle: boolean): string | null {
  if (!devServerBundle) return null;
  let raw: string | null;
  try {
    raw = new URLSearchParams(search).get(DEVICE_ID_PARAM);
  } catch {
    return null;
  }
  const id = raw?.trim().toLowerCase() ?? '';
  return UUID_RE.test(id) ? id : null;
}

/**
 * Adopt the id and drop the parameter from the address bar.
 *
 * Stripping it matters for more than tidiness: without it a reload, a bookmark
 * or a shared link keeps re-asserting an id that may no longer be the right
 * one, and the parameter would sit in the URL of every screenshot of the
 * preview.
 */
export function adoptDeviceIdFromUrl(): void {
  if (typeof window === 'undefined') return;
  const id = deviceIdToAdopt(window.location.search, isDevServerBundle());
  if (!id) return;
  if (localStorage.getItem(DEVICE_ID_KEY) !== id) {
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  const url = new URL(window.location.href);
  url.searchParams.delete(DEVICE_ID_PARAM);
  window.history.replaceState(null, '', url.toString());
}
