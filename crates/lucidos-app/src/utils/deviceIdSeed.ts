
import { DEVICE_ID_KEY } from './deviceIdHeader';
import { isDevServerBundle } from './devServerBundle';

export const DEVICE_ID_PARAM = 'device-id';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

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
