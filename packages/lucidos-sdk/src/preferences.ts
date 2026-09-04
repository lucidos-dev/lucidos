import { request, requestVoid } from './_fetch';
import { assertString } from './_validate';
import { wsDeviceId } from './_storage';

export type Preferences = Record<string, string>;

/**
 * Apps run same-origin in iframes and share the parent's localStorage. The
 * parent stores user-facing prefs (theme, font, scale) device-scoped under
 * this id; reading without it returns only globally-scoped rows, so e.g.
 * `theme` is missing and the iframe defaults to dark.
 *
 * The device id is PER-WORKSPACE (`ws:<slug>:lucidos-device-id`), so read it
 * through `_storage.ts` (the parent's prototype override doesn't reach this
 * realm).
 */
function parentDeviceId(): string | undefined {
  return wsDeviceId() ?? undefined;
}

export const preferences = {
  /**
   * Defaults to the parent device id so iframes see the same merged view as
   * the parent UI. Pass `null` for the unscoped/global view.
   */
  get(deviceId: string | null | undefined = parentDeviceId()): Promise<Preferences> {
    const qs = deviceId ? `?device_id=${encodeURIComponent(deviceId)}` : '';
    return request<{ preferences: Preferences }>(`/preferences${qs}`)
      .then(r => r.preferences);
  },

  set(key: string, value: string, deviceId?: string): Promise<void> {
    assertString('key', key);
    assertString('value', value);
    return requestVoid(`/preferences?key=${encodeURIComponent(key)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value, device_id: deviceId }),
    });
  },
};
