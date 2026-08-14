/** CSS width for a `.progress-bar-fill`, shared by every surface that draws one.
 *
 *  Lived in `Toast.tsx` as `toastProgressWidth` while the toast was the only
 *  caller. The progress dialog draws the same bar, so the name moved with the
 *  scope: a helper called "toast" inside a dialog is the kind of drift
 *  `.claude/rules/glossary.md` bans.
 *
 *  Clamps to [0, 1] and rejects a non-finite fraction. A bad value then paints
 *  an empty track, rather than `NaN%` or a bar running past its container. */
export function progressFillWidth(fraction: number): string {
  if (!Number.isFinite(fraction)) return '0%';
  return `${Math.min(1, Math.max(0, fraction)) * 100}%`;
}
