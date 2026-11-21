/**
 * Watching a relayed update (ADR 0338): the polling half of the update relay.
 *
 * Kept apart from `app-update.ts` on purpose. That module carries no timer at
 * all, because the release CHECK belongs to the gateway (ADR 0108). This one
 * polls only while a request this session made is in flight. The gateway has
 * no push channel to say how the run went.
 *
 * It decides nothing about the UI. Each settlement goes to the caller, which
 * owns the dialog and the toasts.
 */
import { updateRelay, relayedUpdate, releaseCheck, type RelayedUpdate } from '../store';
import {
  loadRelayedUpdate,
  saveRelayedUpdate,
  settleRelay,
  type RelaySettlement,
} from '../updateRelay';
import { getGatewayStatus, type GatewayStatus } from '../../api/client/control';

/** How often a watched request looks at the gateway. The client's own heartbeat
 *  runs at 1s during a run, so a faster look would see the same frame twice. */
const RELAY_POLL_MS = 2_000;

/** Every settlement but `waiting`, which the watch absorbs. */
export type RelayNews = Exclude<RelaySettlement, { kind: 'waiting' }>;

let timer: ReturnType<typeof setTimeout> | null = null;
let listener: ((news: RelayNews) => void) | null = null;

function setMarker(marker: RelayedUpdate | null): void {
  relayedUpdate.value = marker;
  saveRelayedUpdate(marker);
}

/** Start watching a request the gateway just accepted. */
export function watchRelayedUpdate(marker: RelayedUpdate, onNews: (news: RelayNews) => void): void {
  setMarker(marker);
  listener = onNews;
  schedule();
}

/** Pick a request back up after a reload. The service restart a successful run
 *  causes is what reloads the page, mid-run. */
export function resumeRelayedUpdate(onNews: (news: RelayNews) => void): void {
  const marker = loadRelayedUpdate();
  if (!marker || relayedUpdate.value) return;
  relayedUpdate.value = marker;
  listener = onNews;
  void lookOnce();
}

/** Read whether a desktop app is attached, for the `relay` route.
 *
 *  Best-effort (frontend.md carve-out): it rides the release check, mostly on
 *  mount and resume without user intent. A forced check reports its own result.
 *  A failure here leaves the last answer, and the next resume retries. Should that answer be stale, the gateway refuses
 *  the request and the click gets its own error toast. */
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
    // Expected while the service restarts under this page. `settleRelay`
    // bounds the wait, so an outage that never ends still reports.
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
