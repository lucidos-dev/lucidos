import { request, requestVoid } from './_fetch';
import { assertString } from './_validate';
import { isBridged } from './_bridge';
import { wsDeviceId } from './_storage';

export type Preferences = Record<string, string>;

/**
 * What an isolated frame sends instead of the device id it cannot read.
 *
 * The host substitutes the real one, the same id it stamps on the header
 * (`crates/lucidos-app/src/store/actions/app-bridge.ts`). ADR 0227 keeps the id
 * out of the frame, so the frame names the device it is in rather than learning
 * which one that is.
 */
const THIS_DEVICE = '@device';

/**
 * The device to scope a read to, in whichever realm the SDK is running.
 *
 * User-facing preferences (theme, font, scale) are device-scoped, and a read
 * with no device gets only the global rows. So an app that named none would
 * theme itself from a value the shell around it is not using.
 *
 * A standalone app tab is a top-level document and reads the id itself. The id
 * is per-workspace (`ws:<slug>:lucidos-device-id`), which is why it comes from
 * `_storage.ts` rather than raw `localStorage`.
 */
function thisDevice(): string | undefined {
  if (isBridged()) return THIS_DEVICE;
  return wsDeviceId() ?? undefined;
}

export const preferences = {
  /**
   * Defaults to the device the app is running on, so it sees the same merged
   * view as the shell around it. Pass `null` for the unscoped/global view.
   */
  get(deviceId: string | null | undefined = thisDevice()): Promise<Preferences> {
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
