/** Guards against showing the SW update toast spuriously.
 *
 *  Two checks prevent false positives:
 *  1. `hadControllerAtStartup` — false on the very first install (no prior SW),
 *     which means `updatefound` is not a genuine update.
 *  2. sessionStorage dismiss flag — set when the user dismisses the toast or
 *     clicks Refresh. Consumed on next check so a genuinely new update still
 *     shows the toast.
 */

const SW_DISMISSED_KEY = 'lucidos-sw-update-dismissed';

export function shouldShowSwUpdateToast(hadControllerAtStartup: boolean): boolean {
  if (!hadControllerAtStartup) return false;
  try {
    if (sessionStorage.getItem(SW_DISMISSED_KEY)) {
      sessionStorage.removeItem(SW_DISMISSED_KEY);
      return false;
    }
  } catch { /* sessionStorage unavailable (e.g. opaque origin) */ }
  return true;
}

export function markSwUpdateDismissed(): void {
  try {
    sessionStorage.setItem(SW_DISMISSED_KEY, 'true');
  } catch { /* sessionStorage unavailable */ }
}

/** Spread of delays (ms) over which we re-check the service worker for a new
 *  build after a frontend-affecting apply. Covers a typical `vite build`
 *  rebuild window without hammering. */
const SW_UPDATE_CHECK_DELAYS_MS = [3_000, 8_000, 15_000, 30_000];

/** Nudge the service worker to re-check for a new build after a
 *  frontend-affecting apply.
 *
 *  In `web-dev.sh --built` mode the frontend rebuilds (`vite build --watch`)
 *  over a few seconds after a change is applied. Each rebuild stamps a new
 *  BUILD_ID into sw.js (see vite.config.ts `lucidos-sw-stamp`), so a
 *  `registration.update()` then detects the new worker and fires the "New
 *  version available → Refresh" toast (hooks/useStartup.ts). Without this nudge
 *  the toast would only appear on the next resume or the 5-min SW health probe
 *  — this makes "push Apply → get told when it's ready" prompt and hands-free.
 *
 *  Best-effort and self-recovering: a failed `update()` is ignored because the
 *  next scheduled check, the resume-time `reg.update()`, or a manual reload all
 *  re-surface the new build. No-op in the live dev server (sw.js never changes,
 *  so no update is found) and where service workers are unavailable. */
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
