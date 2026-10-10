/** The class every protected surface carries: a card, panel or dialog where
 *  the user grants, denies, answers or confirms (ADR 0309). */
export const PROTECTED_SURFACE = 'protected-surface';

/** ` protected-surface` when `anchor` sits inside a protected surface, else
 *  empty. A popover portalled out of one leaves the surface's token map
 *  behind, so it takes the class along. */
export function protectedClassFrom(anchor: { closest(selector: string): unknown } | null): string {
  return anchor?.closest(`.${PROTECTED_SURFACE}`) ? ` ${PROTECTED_SURFACE}` : '';
}
