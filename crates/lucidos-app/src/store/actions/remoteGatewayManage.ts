
import { signal, type Signal } from '@preact/signals';
import {
  GatewayError,
  listGatewayUsers,
  listPairedDevices,
  type GatewayUsersResponse,
  type PairedDevice,
} from '../../api/client/control';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';
import { settingsGateway } from './settingsGateway';

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

const listGateway = new WeakMap<Signal<unknown>, string>();

async function loadInto<T>(
  target: Signal<RemoteManageLoadable<T>>,
  gateway: string,
  read: () => Promise<T>,
): Promise<void> {
  if (settingsGateway.value !== gateway) return;
  if (listGateway.get(target) === gateway) setLoadingIfFresh(target);
  else target.value = { status: 'loading' };
  listGateway.set(target, gateway);
  let next: RemoteManageLoadable<T>;
  try {
    next = { status: 'loaded', data: await read() };
  } catch (e) {
    const refusal = manageRefusal(e);
    next = refusal ? { status: 'loaded', data: refusal } : toFailed(e);
  }
  if (listGateway.get(target) === gateway && settingsGateway.value === gateway) target.value = next;
}

export function loadRemoteGatewayDevices(gateway: string): Promise<void> {
  return loadInto(remoteGatewayDevices, gateway, () => listPairedDevices(gateway));
}

export function loadRemoteGatewayUsers(gateway: string): Promise<void> {
  return loadInto(remoteGatewayUsers, gateway, () => listGatewayUsers(gateway));
}

export function reloadRemoteGatewayLists(): Promise<unknown> {
  const gateway = settingsGateway.value;
  if (gateway === null) return Promise.resolve();
  return Promise.all([loadRemoteGatewayDevices(gateway), loadRemoteGatewayUsers(gateway)]);
}
