import { computed, signal } from '@preact/signals';
import { listRemoteGateways, type RemoteGateway } from '../../api/client/control';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';

export const pairedRemoteGateways = signal<Loadable<RemoteGateway[]>>({ status: 'not-loaded' });

const pickedSettingsGateway = signal<string | null>(REMOTE_GATEWAY);

export const settingsGateway = computed<string | null>(() => {
  const picked = pickedSettingsGateway.value;
  const list = pairedRemoteGateways.value;
  if (picked === null || list.status !== 'loaded') return picked;
  return list.data.some((remote) => remote.name === picked) ? picked : null;
});

export const remoteGatewaysPaired = computed<boolean>(() => {
  const list = pairedRemoteGateways.value;
  return list.status === 'loaded' && list.data.length > 0;
});

export function selectSettingsGateway(gateway: string | null): void {
  pickedSettingsGateway.value = gateway;
}

export async function loadPairedRemoteGateways(): Promise<void> {
  setLoadingIfFresh(pairedRemoteGateways);
  try {
    pairedRemoteGateways.value = { status: 'loaded', data: await listRemoteGateways() };
  } catch (e) {
    pairedRemoteGateways.value = toFailed(e);
  }
}
