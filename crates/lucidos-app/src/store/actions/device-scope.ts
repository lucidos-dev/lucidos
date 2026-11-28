import { getDeviceId } from './devices';

export interface DeviceScopeActor {
  kind?: string;
  device_id?: string;
}

export function isForThisDevice(actor: DeviceScopeActor | null | undefined): boolean {
  return actor?.kind !== 'device' || actor.device_id === getDeviceId();
}
