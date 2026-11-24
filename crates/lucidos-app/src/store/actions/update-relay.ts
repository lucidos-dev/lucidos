import { updateRelay, relayedUpdate, releaseCheck, type RelayedUpdate } from '../store';
import {
  loadRelayedUpdate,
  saveRelayedUpdate,
  settleRelay,
  type RelaySettlement,
} from '../updateRelay';
import { getGatewayStatus, type GatewayStatus } from '../../api/client/control';

const RELAY_POLL_MS = 2_000;

export type RelayNews = Exclude<RelaySettlement, { kind: 'waiting' }>;

let timer: ReturnType<typeof setTimeout> | null = null;
let listener: ((news: RelayNews) => void) | null = null;

function setMarker(marker: RelayedUpdate | null): void {
  relayedUpdate.value = marker;
  saveRelayedUpdate(marker);
}

export function watchRelayedUpdate(marker: RelayedUpdate, onNews: (news: RelayNews) => void): void {
  setMarker(marker);
  listener = onNews;
  schedule();
}

export function resumeRelayedUpdate(onNews: (news: RelayNews) => void): void {
  const marker = loadRelayedUpdate();
  if (!marker || relayedUpdate.value) return;
  relayedUpdate.value = marker;
  listener = onNews;
  void lookOnce();
}

export async function refreshUpdateRelay(): Promise<void> {
  try {
    updateRelay.value = (await getGatewayStatus()).update_relay ?? null;
  } catch (e) {
    console.warn('[app-update] update relay status unavailable; retried on next resume', e);
  }
}

function schedule(): void {
  if (timer) return;
  timer = setTimeout(() => {
    timer = null;
    void lookOnce();
  }, RELAY_POLL_MS);
}

async function lookOnce(): Promise<void> {
  const marker = relayedUpdate.value;
  if (!marker) return;
  let status: GatewayStatus | null = null;
  try {
    status = await getGatewayStatus();
  } catch {
  }
  if (status?.release_check) releaseCheck.value = status.release_check;
  if (status) updateRelay.value = status.update_relay ?? null;
  const settled = settleRelay(marker, status, Date.now());
  if (settled.kind === 'waiting' || settled.kind === 'progress') {
    schedule();
  } else {
    setMarker(null);
  }
  if (settled.kind !== 'waiting') listener?.(settled);
}
