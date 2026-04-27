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
