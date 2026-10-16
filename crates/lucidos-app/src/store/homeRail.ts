/** The desktop rail that holds the Home board: whether it shows and how wide
 *  it is. Both are per device, in `localStorage`, like the drawer's width. */

import { signal } from '@preact/signals';
import { minHomeRailPx } from './paneMinimums';
import { HOME_RAIL_SHOWN_KEY, HOME_RAIL_WIDTH_KEY } from './homeRailKeys';

/** Shown unless this device hid it. */
export const homeRailShown = signal<boolean>(localStorage.getItem(HOME_RAIL_SHOWN_KEY) !== 'false');

/** Never below the floor: a width saved under a smaller UI scale would clip
 *  the board's two columns. */
export const homeRailWidth = signal<number>(
  Math.max(minHomeRailPx(), Number(localStorage.getItem(HOME_RAIL_WIDTH_KEY)) || 0),
);

export function setHomeRailShown(shown: boolean): void {
  homeRailShown.value = shown;
  localStorage.setItem(HOME_RAIL_SHOWN_KEY, String(shown));
}

export function persistHomeRailWidth(): void {
  localStorage.setItem(HOME_RAIL_WIDTH_KEY, String(homeRailWidth.value));
}

/** Re-apply the floor after a UI-scale change, as the drawer's width does. */
export function clampHomeRailWidth(): void {
  const min = minHomeRailPx();
  if (homeRailWidth.value >= min) return;
  homeRailWidth.value = min;
  persistHomeRailWidth();
}
