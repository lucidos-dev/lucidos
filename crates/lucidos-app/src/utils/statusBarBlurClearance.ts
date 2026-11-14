import { isIOSPwa } from './platform';

/** In CSS px. The blur ends about 40pt below the status bar, and the icon row
 *  already keeps its glyphs about 12pt clear of the bar. */
export const CLEARANCE_PX = 24;

/** The installed iOS app runs its header behind a clear status bar (ADR 0553).
 *  Its controls sit `CLEARANCE_PX` below iOS 27's top-edge blur. Everywhere
 *  else the stylesheet default of 0 stands. */
export function installStatusBarBlurClearance(): void {
  if (!isIOSPwa()) return;
  document.documentElement.style.setProperty('--status-bar-blur-clearance', `${CLEARANCE_PX}px`);
}
