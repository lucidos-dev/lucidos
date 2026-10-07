/**
 * The *disclosure roll*'s curve and length: one source for the thread drawer's
 * FLIP roll (`hooks/useFlipAnimation.ts`) and every `<Disclosure>`
 * (`components/shared/Disclosure.tsx`), so a block unfolds the same way
 * wherever it sits.
 */

/** A disclosure gathers speed, then eases into its landing, on every client. */
export const EASING_DISCLOSURE = 'cubic-bezier(0.4, 0, 0.2, 1)';

/** The custom property the clipping box animates: how far its rows have slid
 *  under the reveal line. The box's mask fades that far down, capped at
 *  `--disclosure-fade` (`.disclosure.is-rolling` in host-components.css). */
export const FADE_REACH = '--disclosure-fade-reach';

const DISCLOSURE_PX_PER_SEC = 1200;
export const DISCLOSURE_MIN_MS = 260;
/** The longest roll, which a caller holding something still across one waits out. */
export const DISCLOSURE_MAX_MS = 420;

/** Base duration for a disclosure that rolls `distance` px, before scaling. */
export function disclosureDurationMs(distance: number): number {
    return Math.min(DISCLOSURE_MAX_MS, Math.max(DISCLOSURE_MIN_MS, distance / DISCLOSURE_PX_PER_SEC * 1000));
}

/** How far a block `height` px tall rolls under a reveal line at viewport y
 *  `line`. It rolls at most down to the screen's bottom edge, as the drawer's
 *  `rollDistance` caps a long section. */
export function rollCap(height: number, line: number, viewportHeight: number): number {
    return Math.max(0, Math.min(height, viewportHeight - line));
}
