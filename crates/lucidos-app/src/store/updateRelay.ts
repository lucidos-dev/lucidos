import { updateRelay, releaseCheck, type RelayedUpdate } from './store';
import { isNewerVersion } from '../utils/version';
import type { GatewayStatus } from '../api/client/control';
import type { AppUpdateProgress } from '../utils/tauri';

export const RELAY_GIVE_UP_MS = 15 * 60_000;

const STORAGE_KEY = 'lucidos.relayed-update';

export function relayClientReady(): boolean {
  const client = updateRelay.value?.client;
  return !!client && client.blocker === null;
}

export function relayAvailable(hasNewerRelease: boolean): boolean {
  const client = updateRelay.value?.client;
  if (!client || client.blocker !== null) return false;
  const latest = releaseCheck.value?.latest?.version;
  return latest ? isNewerVersion(latest, client.version) : hasNewerRelease;
}

export type RelaySettlement =
  | { kind: 'waiting' }
  | { kind: 'progress'; frame: AppUpdateProgress }
  | { kind: 'succeeded'; version: string }
  | { kind: 'ended'; frame: AppUpdateProgress }
  | { kind: 'did-not-run' };

export function settleRelay(
  marker: RelayedUpdate,
  status: GatewayStatus | null,
  now: number,
): RelaySettlement {
  const running = status?.release_check?.current_version;
  if (
    running
    && isNewerVersion(running, marker.fromVersion)
    && !isNewerVersion(marker.version, running)
  ) {
    return { kind: 'succeeded', version: running };
  }
  const request = status?.update_relay?.request;
  if (status && (!request || request.id !== marker.id)) return { kind: 'did-not-run' };
  if (request?.state === 'ended' && request.progress) return { kind: 'ended', frame: request.progress };
  if (now - marker.since >= RELAY_GIVE_UP_MS) return { kind: 'did-not-run' };
  if (request?.state === 'running' && request.progress) return { kind: 'progress', frame: request.progress };
  return { kind: 'waiting' };
}

export function loadRelayedUpdate(): RelayedUpdate | null {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null');
    if (
      parsed && typeof parsed === 'object'
      && typeof (parsed as RelayedUpdate).id === 'string'
      && typeof (parsed as RelayedUpdate).version === 'string'
      && typeof (parsed as RelayedUpdate).fromVersion === 'string'
      && typeof (parsed as RelayedUpdate).since === 'number'
    ) {
      return parsed as RelayedUpdate;
    }
  } catch {
  }
  return null;
}

export function saveRelayedUpdate(marker: RelayedUpdate | null): void {
  if (marker) localStorage.setItem(STORAGE_KEY, JSON.stringify(marker));
  else localStorage.removeItem(STORAGE_KEY);
}
