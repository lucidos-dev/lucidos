/**
 * The gateway's list of paired devices: who may reach this machine at all.
 *
 * Read from the gateway's own `/~/api/v1/auth/devices`, so it answers nothing
 * when no gateway serves the page. Startup loads it, because a device's pairing
 * label is part of its name, and every screen that names a device reads it
 * (`deviceName` in `utils/deviceFriendlyName.ts`).
 */

import { signal } from '@preact/signals';
import { GatewayError, listPairedDevices, type PairedDevice } from '../../api/client/control';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

/** What this deployment has instead of a pairing list.
 *
 *  A LOADED value, not a failure: reaching an engine's own port answers 404 for
 *  `/~/`, and that is a settled fact about the deployment rather than something
 *  going wrong. A gateway that answers anything else IS a failure and takes the
 *  `failed` arm, so a broken one can never read as an absent one. */
export const NO_GATEWAY = 'no-gateway' as const;

/** The gateway's half of the device list.
 *
 *  All four `Loadable` states are distinct here: in flight, loaded with a list,
 *  loaded with [`NO_GATEWAY`], and failed. Collapsing any two would let a
 *  gateway outage render as a deployment that never had one. */
export type PairedDevicesLoadable = Loadable<PairedDevice[] | typeof NO_GATEWAY>;

export const pairedDevices = signal<PairedDevicesLoadable>({ status: 'not-loaded' });

/** Fetch the list. A refetch keeps the loaded list on screen until it lands. */
export async function loadPairedDevices(): Promise<void> {
  setLoadingIfFresh(pairedDevices);
  try {
    pairedDevices.value = { status: 'loaded', data: await listPairedDevices() };
  } catch (e) {
    pairedDevices.value =
      e instanceof GatewayError && e.isAbsent ? { status: 'loaded', data: NO_GATEWAY } : toFailed(e);
  }
}

/** The rows to join onto the engine's, or `null` when there are none to join.
 *
 *  `null` for every state but a loaded list, so an in-flight or failed fetch
 *  never renders as "nothing is paired". */
export function pairedRows(paired: PairedDevicesLoadable): PairedDevice[] | null {
  if (paired.status !== 'loaded' || paired.data === NO_GATEWAY) return null;
  return paired.data;
}

/** Has a gateway told us, one way or the other, which devices are paired?
 *
 *  Gates the pairing clause on a row. False while loading, on a failure, and
 *  where no gateway serves the page, because none of the three knows. */
export function pairingIsKnown(paired: PairedDevicesLoadable): boolean {
  return paired.status === 'loaded' && paired.data !== NO_GATEWAY;
}
