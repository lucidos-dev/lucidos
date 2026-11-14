/**
 * The device-id header, and the storage key behind it.
 *
 * A leaf on purpose: two callers need it and neither may reach the other. The
 * engine API client (`api/client/_core.ts`) carried an inlined copy to dodge a
 * circular import with `store/actions/devices.ts` (which imports `json()`), and
 * the gateway control client (`api/client/control.ts`) is loaded by the
 * workspace picker, which has no business pulling in the store to read one
 * string. Importing nothing from either solves both.
 *
 * Minting the id stays in `devices.ts` (`getDeviceId()`), which owns device
 * registration. Reading it is all this file does, so a surface that has no id
 * yet sends no header rather than quietly registering a device.
 */

import { HEADER_DEVICE_ID } from '@lucidos/engine-constants';
import { DEVICE_ID_KEY } from './deviceIdKey';

export { DEVICE_ID_KEY };

let relayedDeviceId: string | null = null;

/**
 * Name the id this page sends, in memory only, for a page a remote gateway
 * relays. The relaying gateway namespaces the id it asserts, so the engine
 * sees that id whatever we send. Holding it here keeps `lucidos-device-id`
 * for the page's own direct routes.
 */
export function setRelayedDeviceId(id: string | null): void {
  relayedDeviceId = id;
}

/** The id a relayed route holds, or null on a direct route. */
export function getRelayedDeviceId(): string | null {
  return relayedDeviceId;
}

/** This page's device id, or null if none has been minted yet. */
export function readDeviceId(): string | null {
  if (relayedDeviceId) return relayedDeviceId;
  if (typeof localStorage === 'undefined') return null;
  return localStorage.getItem(DEVICE_ID_KEY);
}

/**
 * `{ [HEADER_DEVICE_ID]: <id> }`, or `{}` when there is no id to send.
 *
 * The engine resolves the actor for a mutating request off this header, so
 * omitting it is not neutral: it is the difference between an action attributed
 * to "You" and one attributed to an anonymous API caller (or, at a restart
 * boundary, to the system). Send it on anything a person did.
 */
export function deviceIdHeader(): Record<string, string> {
  const id = readDeviceId();
  return id ? { [HEADER_DEVICE_ID]: id } : {};
}
