
import { signal } from '@preact/signals';
import { GatewayError, listPairedDevices, type PairedDevice } from '../../api/client/control';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

export const NO_GATEWAY = 'no-gateway' as const;

export type PairedDevicesLoadable = Loadable<PairedDevice[] | typeof NO_GATEWAY>;

export const pairedDevices = signal<PairedDevicesLoadable>({ status: 'not-loaded' });

export async function loadPairedDevices(): Promise<void> {
  setLoadingIfFresh(pairedDevices);
  try {
    pairedDevices.value = { status: 'loaded', data: await listPairedDevices() };
  } catch (e) {
    pairedDevices.value =
      e instanceof GatewayError && e.isAbsent ? { status: 'loaded', data: NO_GATEWAY } : toFailed(e);
  }
}

export function pairedRows(paired: PairedDevicesLoadable): PairedDevice[] | null {
  if (paired.status !== 'loaded' || paired.data === NO_GATEWAY) return null;
  return paired.data;
}

export function pairingIsKnown(paired: PairedDevicesLoadable): boolean {
  return paired.status === 'loaded' && paired.data !== NO_GATEWAY;
}
