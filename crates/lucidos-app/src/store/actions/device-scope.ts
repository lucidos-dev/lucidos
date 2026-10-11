import { getDeviceId } from './devices';

/** The `actor` a device-scoped event carries, in the shape the test reads. */
export interface DeviceScopeActor {
  kind?: string;
  device_id?: string;
}

/** Whether an event scoped to `actor` is this device's to act on. The engine
 *  scopes an agent's navigate, app capture and app refresh to one device, the
 *  turn's last used device. One with no device actor (a trigger turn, an app
 *  iframe) belongs to every device. */
export function isForThisDevice(actor: DeviceScopeActor | null | undefined): boolean {
  return actor?.kind !== 'device' || actor.device_id === getDeviceId();
}
