/**
 * iOS / iPadOS detection, mirroring `crates/lucidos-app/src/utils/platform.ts`.
 * `nav` defaults to the global navigator. Two readers: `inIOSStandalone` below,
 * and `autocorrectStamp.ts` resolves an unset Autocorrect switch.
 */
export function isIOSAgent(
  nav: { userAgent: string; platform?: string; maxTouchPoints?: number } | undefined =
    typeof navigator !== 'undefined' ? navigator : undefined,
): boolean {
  if (!nav) return false;
  return /iPad|iPhone|iPod/.test(nav.userAgent) ||
    (nav.platform === 'MacIntel' && (nav.maxTouchPoints ?? 0) > 1);
}

/** Whether this page is inside an installed iOS PWA. An app iframe inherits
 *  the host's display mode, so the same check the host makes works here. */
export function inIOSStandalone(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  if (!isIOSAgent()) return false;
  return window.matchMedia?.('(display-mode: standalone)').matches === true
    || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

/** Whether this page runs in the desktop app's webview, by the same global
 *  the host's `utils/platform.ts` reads. */
export function inDesktopApp(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
