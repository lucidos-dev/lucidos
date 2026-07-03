import { signal } from '@preact/signals';
import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { withBase } from '../utils/basePath';
import { preferences, showToast } from '../store/store';
import { setPreference } from '../api/client';
import { errorDetail } from '../utils/errorDetail';

/** Whether a client refresh is in flight (blocks all user interaction until the
 *  page reloads). Drives the same UiBlockingOverlay as `engineRestarting`, so a
 *  refresh dims and locks the screen exactly like an engine restart does — the
 *  user can't fire a half-loaded interaction during the SW swap + reload window.
 *  Set by `refreshClient`; never cleared, because every refresh path ends in a
 *  `window.location.reload()` that tears the whole page (and this signal) down.
 *  Lives here, not in store.ts, to keep the refresh state with its owner and
 *  avoid a store ↔ sw-update import cycle. */
export const clientRefreshing = signal(false);

/** Build-id-aware, WORKSPACE-GLOBAL dismiss for the two "new version" toasts —
 *  the client-refresh ("refresh to sync", keyed on the served `/sw.js` BUILD_ID)
 *  and the engine switch ("Switch to new version", keyed on the on-disk binary
 *  `version-status.disk_build_id`).
 *
 *  Each toast remembers the EXACT build id the user dismissed so it stays gone for
 *  THAT build but a genuinely newer build re-surfaces it. The dismissal is stored
 *  as a GLOBAL preference (`device_id IS NULL`) — NOT per-device localStorage —
 *  because a *Refresh* reloads onto the shared served client and a *Switch*
 *  restarts the shared engine, so "not now" is a workspace-wide decision:
 *  dismissing on one device defers the toast on EVERY device. The write fans out
 *  via the `PreferencesChanged` SSE → `loadPreferences`, and each surface
 *  re-derives from the reloaded `preferences` signal (the Switch poll every 4s in
 *  engine-update.ts; the client-refresh re-sync on the `PreferencesChanged` arm in
 *  thread-sync.ts). The badge stays lit regardless — it's driven by
 *  readiness/staleness, never by the dismissal — so dismiss defers only the toast.
 *
 *  Durable across reload + cold relaunch (it lives in the DB). Preferences not yet
 *  loaded read as "not dismissed" so the toast fails OPEN (surfacing is the safe
 *  default). `note*BuildId` records which build the live toast is offering so a
 *  dismiss that only knows the toast key (the Toast X → `dismissToast(...)` in
 *  store.ts) can still pin the right id. */

/** Global preference key: last-dismissed served client `/sw.js` BUILD_ID. */
const CLIENT_REFRESH_DISMISSED_KEY = 'client_refresh_dismissed_build';
/** Global preference key: last-dismissed on-disk engine `disk_build_id`. */
const ENGINE_SWITCH_DISMISSED_KEY = 'engine_switch_dismissed_build';

/** Read a global dismissed-build preference; undefined until preferences load. */
function dismissedBuild(key: string): string | undefined {
  return preferences.value.status === 'loaded' ? preferences.value.data[key] : undefined;
}

/** Write a global dismissed-build preference (workspace-wide — NO device id, so
 *  the dismiss defers on every device). Inlines `savePreference`'s optimistic
 *  `preferences` update (so THIS device reflects the dismiss immediately, before
 *  its own `PreferencesChanged` round-trips) + error toast, deliberately rather
 *  than importing `store/actions/preferences`: that module reads the `preferences`
 *  signal at module-init, which — pulled in through the existing store ↔
 *  hooks/sw-update cycle — would re-enter a half-initialized store. */
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

/** The served BUILD_ID the currently-shown update toast is offering. Set by
 *  `surfaceUpdateToast` so `markSwUpdateDismissed` (no-arg, called from the
 *  store's keyed dismiss) can record THIS build as dismissed. */
let pendingUpdateBuildId: string | null = null;

export function noteUpdateBuildId(servedBuildId: string): void {
  pendingUpdateBuildId = servedBuildId;
}

export function markSwUpdateDismissed(): void {
  if (pendingUpdateBuildId === null) return; // nothing to pin
  writeDismissedBuild(CLIENT_REFRESH_DISMISSED_KEY, pendingUpdateBuildId);
}

export function wasSwUpdateDismissed(servedBuildId: string): boolean {
  return dismissedBuild(CLIENT_REFRESH_DISMISSED_KEY) === servedBuildId;
}

/** The on-disk build id the currently-shown Switch toast is offering. Set by the
 *  version poll so `markSwitchDismissed` (called from the store's keyed dismiss)
 *  can record THIS build as dismissed. */
let pendingSwitchBuildId: string | null = null;

export function noteSwitchBuildId(diskBuildId: string): void {
  pendingSwitchBuildId = diskBuildId;
}

export function markSwitchDismissed(): void {
  if (pendingSwitchBuildId === null) return; // nothing to pin
  writeDismissedBuild(ENGINE_SWITCH_DISMISSED_KEY, pendingSwitchBuildId);
}

export function wasSwitchDismissed(diskBuildId: string): boolean {
  return dismissedBuild(ENGINE_SWITCH_DISMISSED_KEY) === diskBuildId;
}

/** Spread of delays (ms) over which we re-check the service worker for a new
 *  build after a frontend-affecting apply. Covers a typical `vite build`
 *  rebuild window without hammering. */
const SW_UPDATE_CHECK_DELAYS_MS = [3_000, 8_000, 15_000, 30_000];

/** Nudge the service worker to re-check for a new build after a
 *  frontend-affecting apply.
 *
 *  In `web-dev.sh --built` mode the build-watch republishes `dist/` (a fresh
 *  `vite build`) over the next few seconds after a change is applied, stamping a
 *  new BUILD_ID into sw.js (see vite.config.ts `lucidos-sw-stamp`). The engine
 *  serves a boot-pinned snapshot, so the served sw.js advances once the engine
 *  re-snapshots: for a **frontend-only** Apply the engine does that in-process
 *  (`engine::frontend_refresh`), and a mixed change advances it via a Switch.
 *  Either way `registration.update()` then detects the new worker and fires the
 *  "New version available → Refresh" toast (hooks/useStartup.ts). Without this
 *  nudge the toast would only appear on the next resume or the 5-min SW health
 *  probe — this makes "push Apply → get told when it's ready" prompt and hands-free.
 *
 *  Best-effort and self-recovering: a failed `update()` is ignored because the
 *  next scheduled check, the resume-time `reg.update()`, or a manual reload all
 *  re-surface the new build. A no-op when the served sw.js genuinely doesn't
 *  change (an engine-only apply with no client delta) and where service workers
 *  are unavailable. */
export function scheduleServiceWorkerUpdateChecks(): void {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return;
  for (const delay of SW_UPDATE_CHECK_DELAYS_MS) {
    setTimeout(() => {
      navigator.serviceWorker.getRegistration()
        .then((reg) => reg?.update())
        .catch(() => { /* best-effort; next check / resume / manual reload covers it */ });
    }, delay);
  }
}

/** Safety net (ms): if a new worker is detected but never claims the page
 *  (delayed/dropped `controllerchange`, e.g. a wedged SW), stop waiting, bust
 *  the cached shell, and reload anyway so the click can't hang on a stale build.
 *  Sized to clear a normal install→activate→claim hop, even over a slow link, so
 *  it doesn't pre-empt the swap and reload stale content. */
const REFRESH_SWAP_TIMEOUT_MS = 4000;

/** Delete the `lucidos-shell-*` caches (the cache-first content-hashed
 *  `/assets/*` bundles plus the offline navigation-shell fallback, both written
 *  by public/sw.js) so the next reload refetches them from the network. The shell
 *  itself is served network-first now, but the `/assets/*` graph stays
 *  cache-first, so a refresh that must guarantee fresh content even when the SW
 *  won't swap (wedged / won't-claim, or an old pre-network-first worker still
 *  controlling) busts these first. The immutable blob cache is left intact (its
 *  content-addressed entries are never stale and are expensive to refetch).
 *  Best-effort: any failure just means the reload proceeds against whatever the
 *  SW has. */
export async function bustShellCaches(): Promise<void> {
  if (typeof caches === 'undefined') return;
  try {
    const names = await caches.keys();
    await Promise.all(
      names.filter((n) => n.startsWith('lucidos-shell-')).map((n) => caches.delete(n)),
    );
  } catch {
    /* best-effort — the reload still proceeds */
  }
}

/** Whether the running bundle is already the build the server is serving — so a
 *  refresh can be a fast cache-first reload (the content-hashed `/assets/*` the
 *  page references are byte-identical to what's cached) instead of busting the
 *  shell cache and refetching the whole JS/CSS graph from the network. True only
 *  when BOTH ids are known and equal; an un-stamped dev bundle (`__…__`) or an
 *  unknown served id (offline, dev server, or a failed fetch → null) returns
 *  false, so we fall back to the conservative swap + bust path. */
export function isRunningServedBuild(servedBuildId: string | null): boolean {
  return servedBuildId !== null
    && !CLIENT_BUILD_ID.startsWith('__')
    && servedBuildId === CLIENT_BUILD_ID;
}

/** Refresh the client, picking up a newer build if there is one — the single
 *  refresh path for the whole app (control panel, applied-change / reconnect /
 *  update toasts, recovery "Reload" buttons).
 *
 *  Blocks the UI for the (brief) reload window: it flips `clientRefreshing`,
 *  which raises the same UiBlockingOverlay an engine restart uses, so the screen
 *  dims and locks until the page reloads — no clicks land on a half-swapped
 *  build. The signal is never cleared here; the reload tears the page down.
 *
 *  A bare `location.reload()` keeps the CURRENT service worker: it won't fetch a
 *  new `sw.js` for this load, and an old (pre-network-first) worker still
 *  controlling the page can serve a stale cached shell, while the cache-first
 *  `/assets/*` graph is only refetched for hashes the served shell references
 *  (see `public/sw.js`). So we branch on whether the loaded code is current:
 *   - FAST PATH — the running bundle already matches the served `sw.js`
 *     BUILD_ID (`isRunningServedBuild`): nothing is newer and the assets the
 *     page references are byte-identical to cache, so a plain reload serves them
 *     cache-first (from disk). We skip the `reg.update()` round-trip AND the
 *     shell-cache bust — the bust would force a full network refetch of the
 *     whole JS/CSS graph (the cold-load cost) for no benefit, which is what made
 *     every up-to-date refresh slow.
 *   - otherwise the build is stale or unknown, so we ask the registration to
 *     check for a new `sw.js`, then:
 *       - if a new worker is installing/waiting, it `skipWaiting()`s and claims
 *         the page (public/sw.js), firing `controllerchange` — we reload on that
 *         so the NEW worker is in control. If it never claims within the timeout
 *         (wedged SW), we bust the shell cache and reload from the network;
 *       - otherwise nothing is swapping (the SW won't hand over, or the build id
 *         couldn't be read). A manual refresh is an explicit request for fresh
 *         content, so we bust the shell cache first so the reload can't be
 *         served stale assets, then reload.
 *  A `reloaded` guard ensures exactly one reload if multiple paths race. */
export function refreshClient(): void {
  // Block the UI immediately — before any async SW work — so the screen locks
  // the instant the refresh is requested, mirroring an engine restart.
  clientRefreshing.value = true;
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) {
    window.location.reload();
    return;
  }
  let reloaded = false;
  let swapTimer: ReturnType<typeof setTimeout> | null = null;
  const reloadOnce = (): void => {
    if (reloaded) return;
    reloaded = true;
    if (swapTimer !== null) clearTimeout(swapTimer);
    window.location.reload();
  };
  navigator.serviceWorker.addEventListener('controllerchange', reloadOnce, { once: true });
  getServedBuildId()
    .then(async (served) => {
      // Fast path: already on the served build — reload cache-first, no SW
      // round-trip and no asset-cache bust (see isRunningServedBuild).
      if (isRunningServedBuild(served)) { reloadOnce(); return; }
      const reg = await navigator.serviceWorker.getRegistration();
      if (!reg) { reloadOnce(); return; } // no SW registered — plain reload
      await reg.update().catch(() => { /* offline / 404 during swap — handled below */ });
      if (reg.installing || reg.waiting) {
        // A new build is taking over; controllerchange will reload onto it. If
        // claim is delayed or never fires (wedged SW), bust the stale shell so
        // the fallback reload pulls the new build from the network.
        swapTimer = setTimeout(() => {
          swapTimer = null;
          void bustShellCaches().finally(reloadOnce);
        }, REFRESH_SWAP_TIMEOUT_MS);
      } else {
        // Stale or unknown build and nothing swapping — bust the cached shell so
        // a stale/wedged SW can't serve the old build straight back, then reload.
        await bustShellCaches();
        reloadOnce();
      }
    })
    .catch(() => reloadOnce()); // getServedBuildId / getRegistration failed — reload anyway
}

/** Loop guard window (ms): at most one stale-chunk auto-reload per window, so a
 *  reload that doesn't fix the chunk can't loop. Sized well above a rebuild's
 *  install→activate→claim hop. */
const CHUNK_RELOAD_WINDOW_MS = 30_000;
const CHUNK_RELOAD_KEY = 'lucidos-chunk-reload-at';

/** Whether a stale-chunk auto-reload is allowed right now — and, if so, reserve
 *  the window by stamping `now`. Reads/writes a sessionStorage timestamp so a
 *  reload that DOESN'T fix the chunk can't loop: the second failure inside the
 *  window returns false and the caller falls back to a toast. That second
 *  failure happens for real — e.g. a deep-linked file preview re-mounts the same
 *  failing lazy component on the post-reload page, or the chunk is genuinely
 *  broken in the new build too. Returns false when sessionStorage is
 *  unavailable (opaque origin / private mode): with no guard, auto-reloading
 *  risks an infinite loop, so we'd rather surface the toast. */
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

/** Handle a failed dynamic `import()` of a content-hashed chunk. The
 *  overwhelmingly common cause is a newer build shipping while this tab kept the
 *  old one in memory: the in-memory entry chunk still references the OLD chunk
 *  hashes, whose URLs now 404 because the SW purged the prior build's /assets
 *  cache on activate (public/sw.js). Retrying the same `import()` can't help —
 *  the filename is baked into the loaded bundle — so we reload instead.
 *  `refreshClient()` swaps the SW to the new build first, so the fresh
 *  index.html requests the new chunk hashes.
 *
 *  Returns true if a reload was initiated (caller suppresses its error UI),
 *  false if the loop guard tripped (caller should surface a toast). */
export function reloadForStaleChunk(): boolean {
  if (staleChunkReloadStarted) return true; // a reload is already in flight this page-life
  if (!shouldReloadForStaleChunk(Date.now())) return false;
  staleChunkReloadStarted = true;
  refreshClient();
  return true;
}

/** Pull the stamped BUILD_ID out of a fetched sw.js (vite.config.ts
 *  `lucidos-sw-stamp` emits `const BUILD_ID = '<hex>';`). */
const SW_BUILD_ID_RE = /BUILD_ID\s*=\s*['"]([^'"]+)['"]/;

/** The BUILD_ID the server is currently serving in `/sw.js`, or null if it
 *  can't be determined — service workers unavailable, offline, a non-ok
 *  response, no BUILD_ID marker, or the un-stamped `__…__` dev placeholder.
 *  Returning null for "couldn't check" (vs. a real id) lets callers leave their
 *  state untouched on a transient failure instead of mis-clearing the update
 *  badge.
 *
 *  The BUILD_ID is stamped from the emitted asset hashes, so it changes ONLY
 *  when the frontend bundle is actually rebuilt — NOT on engine-only version
 *  bumps. Compared against the running bundle's `CLIENT_BUILD_ID`
 *  (store/actions/client-update.ts), it's the honest "is the loaded code stale?"
 *  signal: it tracks the code the page is actually running, not whichever
 *  service worker happens to control it.
 *
 *  `/sw.js` is NOT intercepted by the SW fetch handler (it only touches
 *  `/api/v1/*`, `/assets/*` and the `/` shell), and `cache: 'no-store'` bypasses
 *  the HTTP cache, so the fetched copy is the live served build. Returns null in
 *  the live dev server, where sw.js carries the un-stamped placeholder. */
export async function getServedBuildId(): Promise<string | null> {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return null;
  try {
    const resp = await fetch(withBase('/sw.js'), { cache: 'no-store' });
    if (!resp.ok) return null;
    const id = SW_BUILD_ID_RE.exec(await resp.text())?.[1];
    if (!id || id.startsWith('__')) return null;
    return id;
  } catch {
    // Offline / transient — the next load/resume check (or the browser's own SW
    // update) re-surfaces a genuine update; no user-facing error warranted.
    return null;
  }
}

/** Ask the active service worker for its stamped BUILD_ID (vite.config.ts
 *  `lucidos-sw-stamp`). The reply arrives as a `lucidos:build-id` message,
 *  handled in useStartup.ts where it lands in the `serviceWorkerBuildId` signal
 *  for the control panel.
 *
 *  We query the SW rather than baking the id into the app bundle so the reported
 *  value is the build that's ACTUALLY controlling the page — the same value
 *  whose byte-change drives the update toast — which makes it a real "did the SW
 *  update?" probe. Best-effort: no SW / no controller → no reply, and the
 *  control panel simply omits the build row. */
export function requestServiceWorkerBuildId(): void {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return;
  const sw = navigator.serviceWorker;
  if (sw.controller) {
    sw.controller.postMessage({ type: 'lucidos:get-build-id' });
    return;
  }
  // First load before the SW claims the page: there's no controller yet, so
  // post to the ready registration's active worker instead — the reply still
  // arrives on navigator.serviceWorker's message listener regardless of control.
  sw.ready
    .then((reg) => reg.active?.postMessage({ type: 'lucidos:get-build-id' }))
    .catch(() => { /* best-effort; controllerchange / panel-open re-queries */ });
}
