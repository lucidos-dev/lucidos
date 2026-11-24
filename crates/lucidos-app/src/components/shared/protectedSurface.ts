export const PROTECTED_SURFACE = 'protected-surface';

export function protectedClassFrom(anchor: { closest(selector: string): unknown } | null): string {
  return anchor?.closest(`.${PROTECTED_SURFACE}`) ? ` ${PROTECTED_SURFACE}` : '';
}
