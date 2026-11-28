import { signal } from '@preact/signals';
import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { peerClient } from '../../../../packages/lucidos-sdk/src/_peerScope';
import { withBase, REMOTE_GATEWAY } from '../utils/basePath';
import { isUnstampedBuildId } from '../utils/buildId';
import { preferences, showToast } from '../store/store';
import { setPreference } from '../api/client';
import { markSplashQuiet } from '../utils/workspaceStorage';
import { errorDetail } from '../utils/errorDetail';

export const clientRefreshing = signal(false);


const CLIENT_REFRESH_DISMISSED_KEY = 'client_refresh_dismissed_build';
const ENGINE_SWITCH_DISMISSED_KEY = 'engine_switch_dismissed_build';

function dismissedBuild(key: string): string | undefined {
  return preferences.value.status === 'loaded' ? preferences.value.data[key] : undefined;
}

function writeDismissedBuild(key: string, buildId: string): void {
  if (preferences.value.status === 'loaded') {
    preferences.value = {
      status: 'loaded',
      data: { ...preferences.value.data, [key]: buildId },
    };
  }
  setPreference(key, buildId).catch((e) => {
    showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error');
  });
}

let pendingUpdateBuildId: string | null = null;

export function noteUpdateBuildId(servedBuildId: string): void {
  pendingUpdateBuildId = servedBuildId;
}

export function markSwUpdateDismissed(): void {
  if (pendingUpdateBuildId === null) return;
  writeDismissedBuild(CLIENT_REFRESH_DISMISSED_KEY, pendingUpdateBuildId);
}

export function wasSwUpdateDismissed(servedBuildId: string): boolean {
  return dismissedBuild(CLIENT_REFRESH_DISMISSED_KEY) === servedBuildId;
}

let announcedEngineVersionId: string | null = null;

export function noteAnnouncedEngineVersion(versionId: string): void {
  announcedEngineVersionId = versionId;
}

export function markEngineVersionDismissed(): void {
  if (announcedEngineVersionId === null) return;
  writeDismissedBuild(ENGINE_SWITCH_DISMISSED_KEY, announcedEngineVersionId);
}

export function dismissedEngineVersion(): string | undefined {
  return dismissedBuild(ENGINE_SWITCH_DISMISSED_KEY);
}

export function wasEngineVersionDismissed(versionId: string): boolean {
  return dismissedEngineVersion() === versionId;
}

let announcedBuildFailureId: string | null = null;
let dismissedBuildFailureId: string | null = null;

export function noteAnnouncedBuildFailure(failureId: string): void {
  announcedBuildFailureId = failureId;
}

export function markBuildFailureDismissed(): void {
  dismissedBuildFailureId = announcedBuildFailureId;
}

export function wasBuildFailureDismissed(failureId: string): boolean {
  return dismissedBuildFailureId === failureId;
}

const REFRESH_SWAP_TIMEOUT_MS = 4000;

const REFRESH_DEADLINE_MS = 15_000;

const BUST_BOUND_MS = 2000;

export const SHELL_CACHE_PREFIX = 'lucidos-shell-';

export async function bustShellCaches(): Promise<void> {
  if (typeof caches === 'undefined') return;
  try {
    const names = await caches.keys();
    await Promise.all(
      names.filter((n) => n.startsWith(SHELL_CACHE_PREFIX)).map((n) => caches.delete(n)),
    );
  } catch {
  }
}

export function isRunningServedBuild(servedBuildId: string | null): boolean {
  return servedBuildId !== null
    && !isUnstampedBuildId(CLIENT_BUILD_ID)
    && servedBuildId === CLIENT_BUILD_ID;
}

export function refreshClient(opts: { evictAssets?: boolean } = {}): void {
  clientRefreshing.value = true;
  markSplashQuiet();
  if (REMOTE_GATEWAY !== null || typeof navigator === 'undefined' || !('serviceWorker' in navigator)) {
    window.location.reload();
    return;
  }
  let reloaded = false;
  let swapTimer: ReturnType<typeof setTimeout> | null = null;
  const reloadOnce = (): void => {
    if (reloaded) return;
    reloaded = true;
    if (swapTimer !== null) clearTimeout(swapTimer);
    clearTimeout(deadlineTimer);
    window.location.reload();
  };
  const bustThenReload = (): void => {
    setTimeout(reloadOnce, BUST_BOUND_MS);
    void bustShellCaches().finally(reloadOnce);
  };
  const deadlineTimer = setTimeout(bustThenReload, REFRESH_DEADLINE_MS);
  navigator.serviceWorker.addEventListener('controllerchange', reloadOnce, { once: true });
  getServedBuildId()
    .then(async (served) => {
      if (isRunningServedBuild(served)) {
        if (opts.evictAssets) bustThenReload();
        else reloadOnce();
        return;
      }
      const reg = await navigator.serviceWorker.getRegistration();
      if (!reg) { reloadOnce(); return; }
      await reg.update().catch(() => {  });
      if (reg.installing || reg.waiting) {
        swapTimer = setTimeout(() => {
          swapTimer = null;
          bustThenReload();
        }, REFRESH_SWAP_TIMEOUT_MS);
      } else {
        bustThenReload();
      }
    })
    .catch(reloadOnce);
}

const CHUNK_RELOAD_WINDOW_MS = 30_000;
const CHUNK_RELOAD_KEY = 'lucidos-chunk-reload-at';

export function shouldReloadForStaleChunk(now: number): boolean {
  try {
    const last = Number(sessionStorage.getItem(CHUNK_RELOAD_KEY) || '0');
    if (now - last < CHUNK_RELOAD_WINDOW_MS) return false;
    sessionStorage.setItem(CHUNK_RELOAD_KEY, String(now));
    return true;
  } catch {
    return false;
  }
}

let staleChunkReloadStarted = false;

export const NAVIGATION_CANCEL_WINDOW_MS = 2000;
let leavingAt: number | null = null;
if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', () => {
    leavingAt = Date.now();
  });
}

function pageIsLeaving(now: number): boolean {
  return leavingAt !== null && now - leavingAt < NAVIGATION_CANCEL_WINDOW_MS;
}

export function reloadForStaleChunk(): boolean {
  const now = Date.now();
  if (pageIsLeaving(now)) return true;
  if (staleChunkReloadStarted) return true;
  if (!shouldReloadForStaleChunk(now)) return false;
  staleChunkReloadStarted = true;
  refreshClient({ evictAssets: true });
  return true;
}

const SHELL_BUILD_ID_RE = /<meta\s+name="lucidos-build-id"\s+content="([^"]+)"\s*\/?>/;
const PEER_FALLBACK_RE = /<meta\s+name="lucidos-peer-client-unavailable"\s/;
const SHELL_BUILD_ID_SCAN_BYTES = 8 * 1024;

async function readShellBuildId(response: Response): Promise<string | null> {
  const runningPeerClient = peerClient() !== null;
  const inspect = (prefix: string): string | null | undefined => runningPeerClient && PEER_FALLBACK_RE.test(prefix)
    ? null
    : SHELL_BUILD_ID_RE.exec(prefix)?.[1];
  if (!response.body) {
    return inspect((await response.text()).slice(0, SHELL_BUILD_ID_SCAN_BYTES)) ?? null;
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let remaining = SHELL_BUILD_ID_SCAN_BYTES;
  let prefix = '';
  try {
    while (remaining > 0) {
      const { done, value } = await reader.read();
      if (done) return null;
      const chunk = value.subarray(0, remaining);
      remaining -= chunk.byteLength;
      prefix += decoder.decode(chunk, { stream: true });
      const id = inspect(prefix);
      if (id !== undefined) return id;
    }
    return null;
  } finally {
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function getServedBuildId(): Promise<string | null> {
  try {
    const response = await fetch(withBase('/'), { cache: 'no-store', headers: { Accept: 'text/html' } });
    if (!response.ok || response.headers.has('x-lucidos-pairing')) return null;
    const id = await readShellBuildId(response);
    return id && !isUnstampedBuildId(id) ? id : null;
  } catch {
    return null;
  }
}

export function requestServiceWorkerBuildId(): void {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return;
  const sw = navigator.serviceWorker;
  if (sw.controller) {
    sw.controller.postMessage({ type: 'lucidos:get-build-id' });
    return;
  }
  sw.ready
    .then((reg) => reg.active?.postMessage({ type: 'lucidos:get-build-id' }))
    .catch(() => {  });
}
