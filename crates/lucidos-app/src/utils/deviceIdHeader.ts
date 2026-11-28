
import { HEADER_DEVICE_ID } from '@lucidos/engine-constants';
import { DEVICE_ID_KEY } from './deviceIdKey';

export { DEVICE_ID_KEY };

let relayedDeviceId: string | null = null;

export function setRelayedDeviceId(id: string | null): void {
  relayedDeviceId = id;
}

export function getRelayedDeviceId(): string | null {
  return relayedDeviceId;
}

export function readDeviceId(): string | null {
  if (relayedDeviceId) return relayedDeviceId;
  if (typeof localStorage === 'undefined') return null;
  return localStorage.getItem(DEVICE_ID_KEY);
}

export function deviceIdHeader(): Record<string, string> {
  const id = readDeviceId();
  return id ? { [HEADER_DEVICE_ID]: id } : {};
}
