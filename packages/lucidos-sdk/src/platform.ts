/**
 * iOS / iPadOS detection, mirroring `crates/lucidos-app/src/utils/platform.ts`.
 * `nav` defaults to the global navigator. Two readers: `ui.ts` spots an
 * installed iOS PWA, and `autocorrectStamp.ts` resolves an unset Autocorrect
 * switch.
 */
export function isIOSAgent(
  nav: { userAgent: string; platform?: string; maxTouchPoints?: number } | undefined =
    typeof navigator !== 'undefined' ? navigator : undefined,
): boolean {
  if (!nav) return false;
  return /iPad|iPhone|iPod/.test(nav.userAgent) ||
    (nav.platform === 'MacIntel' && (nav.maxTouchPoints ?? 0) > 1);
}
