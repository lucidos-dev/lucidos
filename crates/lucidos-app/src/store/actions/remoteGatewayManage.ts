/**
 * The users and devices of the remote gateway this page's workspace lives on,
 * read through this gateway's manage hop (ADR 0656).
 *
 * Only a remote workspace (`/<slug>@<name>/`) loads them. Two refusals are
 * settled facts about the pairing, so they load as a `ManageRefusal` rather
 * than failing: the remote gateway has not granted the hop, or it is too old
 * to serve it. Anything else, such as an unreachable gateway, fails.
 */

import { signal, type Signal } from '@preact/signals';
import {
  GatewayError,
  listGatewayUsers,
  listPairedDevices,
  type GatewayUsersResponse,
  type PairedDevice,
} from '../../api/client/control';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

/** Why the remote gateway's lists are not shown. Mirrors the reason kinds
 *  in the gateway's `manage_hop.rs`. */
export type ManageRefusal = { refused: 'not-granted' } | { refused: 'too-old' };

export type RemoteManageLoadable<T> = Loadable<T | ManageRefusal>;

export const remoteGatewayDevices = signal<RemoteManageLoadable<PairedDevice[]>>({ status: 'not-loaded' });
export const remoteGatewayUsers = signal<RemoteManageLoadable<GatewayUsersResponse>>({ status: 'not-loaded' });

export function isManageRefusal(data: unknown): data is ManageRefusal {
  return typeof data === 'object' && data !== null && 'refused' in data;
}

function manageRefusal(e: unknown): ManageRefusal | null {
  if (!(e instanceof GatewayError)) return null;
  if (e.reasonKind === 'manage-not-granted') return { refused: 'not-granted' };
  if (e.reasonKind === 'remote-gateway-too-old') return { refused: 'too-old' };
  return null;
}

async function loadInto<T>(
  target: Signal<RemoteManageLoadable<T>>,
  read: () => Promise<T>,
): Promise<void> {
  setLoadingIfFresh(target);
  try {
    target.value = { status: 'loaded', data: await read() };
  } catch (e) {
    const refusal = manageRefusal(e);
    target.value = refusal ? { status: 'loaded', data: refusal } : toFailed(e);
  }
}

/** Fetch remote gateway `gateway`'s paired devices. */
export function loadRemoteGatewayDevices(gateway: string): Promise<void> {
  return loadInto(remoteGatewayDevices, () => listPairedDevices(gateway));
}

/** Fetch remote gateway `gateway`'s users. */
export function loadRemoteGatewayUsers(gateway: string): Promise<void> {
  return loadInto(remoteGatewayUsers, () => listGatewayUsers(gateway));
}

/** Re-read both lists of the remote gateway this page's workspace lives on.
 *  Nothing outside a remote workspace. */
export function reloadRemoteGatewayLists(): Promise<unknown> {
  if (REMOTE_GATEWAY === null) return Promise.resolve();
  return Promise.all([loadRemoteGatewayDevices(REMOTE_GATEWAY), loadRemoteGatewayUsers(REMOTE_GATEWAY)]);
}
