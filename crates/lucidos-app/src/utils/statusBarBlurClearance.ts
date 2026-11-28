import { isIOSPwa } from './platform';

export const CLEARANCE_PX = 24;

export function installStatusBarBlurClearance(): void {
  if (!isIOSPwa()) return;
  document.documentElement.style.setProperty('--status-bar-blur-clearance', `${CLEARANCE_PX}px`);
}
