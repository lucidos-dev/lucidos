export function isIOSAgent(
  nav: { userAgent: string; platform?: string; maxTouchPoints?: number } | undefined =
    typeof navigator !== 'undefined' ? navigator : undefined,
): boolean {
  if (!nav) return false;
  return /iPad|iPhone|iPod/.test(nav.userAgent) ||
    (nav.platform === 'MacIntel' && (nav.maxTouchPoints ?? 0) > 1);
}

export function inIOSStandalone(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  if (!isIOSAgent()) return false;
  return window.matchMedia?.('(display-mode: standalone)').matches === true
    || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

export function inDesktopApp(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
