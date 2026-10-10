/**
 * The update relay's rules as reads over the signals (ADR 0338): may this
 * session ask the desktop app to update, and how did a request it made end?
 *
 * A leaf beside `packagedUpdate.ts`, for the same reason. The action next door
 * (`actions/app-update.ts`) owns the toasts and the polling, and these rules
 * are pure so every ending is unit-tested.
 */
import { updateRelay, releaseCheck, type RelayedUpdate } from './store';
import { isNewerVersion } from '../utils/version';
import type { GatewayStatus } from '../api/client/control';
import type { AppUpdateProgress } from '../utils/tauri';

/** How long a request may go without an ending before it reads as "did not
 *  run". Generous, because a slow link takes minutes over the download. */
export const RELAY_GIVE_UP_MS = 15 * 60_000;

const STORAGE_KEY = 'lucidos.relayed-update';

/**
 * Can the desktop app take the newest release for this session?
 *
 * All three: a client is attached, it reports no blocker, and the gateway knows
 * a release newer than the client. The gateway refuses any other request, so a
 * button offered outside this would be a button that errors.
 */
export function relayAvailable(): boolean {
  const client = updateRelay.value?.client;
  const latest = releaseCheck.value?.latest?.version;
  return !!client && client.blocker === null && !!latest && isNewerVersion(latest, client.version);
}

/** Where a watched request stands after one look at the gateway. */
export type RelaySettlement =
  /** Nothing new: not yet claimed, or the gateway is down for the restart. */
  | { kind: 'waiting' }
  /** The run is under way, and this is its latest frame. */
  | { kind: 'progress'; frame: AppUpdateProgress }
  /** The service came back on a newer version. */
  | { kind: 'succeeded'; version: string }
  /** The run ended without a restart, and the frame says how. */
  | { kind: 'ended'; frame: AppUpdateProgress }
  /** The request vanished with the version unchanged, or the wait ran out. */
  | { kind: 'did-not-run' };

/**
 * Where `marker` stands, given one gateway status, or `null` when the gateway
 * did not answer.
 *
 * Success is proven by the version and nothing else. A successful run restarts
 * the service, and the relay's own state dies with it. So its absence proves
 * nothing until the version says which way it went. The version must also be
 * at least the requested one, so an unrelated update cannot pass for this one.
 *
 * Every unfinished state shares one bound, a run still sending frames included.
 * A run can stop short with its client still beating, say when the service
 * restart fails after the swap. Its last frame would otherwise stand forever.
 */
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

/** The watched request a reload left behind, or `null`. A shape that does not
 *  parse is dropped, since a half-read marker would watch for nothing. */
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
    // Unreadable storage holds no marker. Nothing is lost: the request still
    // runs on the Mac, and only this page's narration of it is gone.
  }
  return null;
}

export function saveRelayedUpdate(marker: RelayedUpdate | null): void {
  if (marker) localStorage.setItem(STORAGE_KEY, JSON.stringify(marker));
  else localStorage.removeItem(STORAGE_KEY);
}
