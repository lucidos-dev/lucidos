import { signal } from '@preact/signals';
import { clampToRange } from '../layout/splitHelpers';


export const SIDEBAR_DEFAULT_REM = 16;
export const SIDEBAR_MIN_REM = 12;
export const DIFF_PANE_MIN_REM = 24;
export const SIDEBAR_KEY_STEP_REM = 1;
export const SIDEBAR_WIDTH_KEY = 'lucidos-repo-preview-sidebar-width';

export function sidebarMaxRem(containerRem: number): number {
  return Math.max(SIDEBAR_MIN_REM, containerRem - DIFF_PANE_MIN_REM);
}

export function clampSidebarRem(rem: number, containerRem: number): number {
  if (containerRem <= 0) return Math.max(rem, SIDEBAR_MIN_REM);
  return clampToRange(rem, SIDEBAR_MIN_REM, containerRem - DIFF_PANE_MIN_REM);
}

export function readStoredSidebarRem(): number {
  const stored = parseFloat(localStorage.getItem(SIDEBAR_WIDTH_KEY) ?? '');
  return Number.isFinite(stored) ? Math.max(stored, SIDEBAR_MIN_REM) : SIDEBAR_DEFAULT_REM;
}

export const repoPreviewSidebarRem = signal(readStoredSidebarRem());

export function setSidebarRem(rem: number): void {
  repoPreviewSidebarRem.value = rem;
  localStorage.setItem(SIDEBAR_WIDTH_KEY, String(rem));
}
