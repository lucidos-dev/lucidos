import { request } from './_fetch';
import { assertString } from './_validate';
import { isBridged } from './_bridge';
import { wsDeviceId } from './_storage';

export type Preferences = Record<string, string>;

const THIS_DEVICE = '@device';

function thisDevice(): string | undefined {
  if (isBridged()) return THIS_DEVICE;
  return openerDevice() ?? wsDeviceId() ?? undefined;
}

function openerDevice(): string | null {
  const search = globalThis.location?.search;
  return search ? new URLSearchParams(search).get('device') : null;
}

export const preferences = {
  get(deviceId: string | null | undefined = thisDevice()): Promise<Preferences> {
    const qs = deviceId ? `?device_id=${encodeURIComponent(deviceId)}` : '';
    return request<{ preferences: Preferences }>(`/preferences${qs}`)
      .then(r => r.preferences);
  },

  set(key: string, value: string, deviceId?: string): Promise<void> {
    assertString('key', key);
    assertString('value', value);
    return request<{ success?: boolean; error?: string } | null>(
      `/preferences?key=${encodeURIComponent(key)}`,
      {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ value, device_id: deviceId }),
      },
    ).then((result) => {
      if (result?.success === false) {
        throw new Error(result.error || `the engine refused the '${key}' preference`);
      }
    });
  },
};
