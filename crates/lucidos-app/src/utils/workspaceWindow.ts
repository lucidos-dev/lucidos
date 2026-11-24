
import { openWorkspace } from '../api/client/control';
import { workspaceRouteUrlSegment } from './basePath';
import { isMac, isStandalone, isTauri } from './platform';
import type { ClickModifiers } from './documentNavigation';
import { openNewTab } from './newTab';
import { showWorkspaceInNativeWindow } from './tauri';
import { landingHash, type WorkspaceLanding } from './workspaceLanding';
import type { WorkspaceState } from './workspaceState';

export type WorkspaceOpenMode = 'in-place' | 'separate';

const MIDDLE_BUTTON = 1;

export function separateViewIsAvailable(): boolean {
  return isTauri() || !isStandalone();
}

export function defaultOpenMode(): WorkspaceOpenMode {
  return isTauri() ? 'separate' : 'in-place';
}

export function alternateOpenMode(
  state: WorkspaceState,
  currentId: string | null,
  rowId: string,
): WorkspaceOpenMode | null {
  if (state === 'unhealthy') return null;
  if (defaultOpenMode() === 'in-place') {
    return separateViewIsAvailable() ? 'separate' : null;
  }
  return currentId !== null && currentId !== rowId ? 'in-place' : null;
}

export function openModeLabel(mode: WorkspaceOpenMode): string {
  if (mode === 'in-place') return 'Switch this window';
  return isTauri() ? 'Open in new window' : 'Open in new tab';
}

function asksForSeparateView(e: ClickModifiers): boolean {
  if (e.button === MIDDLE_BUTTON) return true;
  return (isMac ? e.metaKey : e.ctrlKey) === true;
}

function isMacContextClick(e: ClickModifiers): boolean {
  return isMac && e.ctrlKey === true && e.button !== MIDDLE_BUTTON;
}

export function openModeForClick(e: ClickModifiers): WorkspaceOpenMode | null {
  if (isMacContextClick(e)) return null;
  if (asksForSeparateView(e) && separateViewIsAvailable()) return 'separate';
  return defaultOpenMode();
}

export function middleClickActivates(state: WorkspaceState): boolean {
  return separateViewIsAvailable() && state !== 'unhealthy';
}

export function middleClickHandler(activate: (e: MouseEvent) => void) {
  return (e: MouseEvent): void => {
    if (e.button !== MIDDLE_BUTTON) return;
    e.preventDefault();
    activate(e);
  };
}

export function workspacePath(id: string, landing?: WorkspaceLanding): string {
  return `/${workspaceRouteUrlSegment(id)}/${landingHash(landing)}`;
}

export function workspaceTabName(slug: string): string {
  return `lucidos-ws-${slug}`;
}

export async function openWorkspaceIn(
  mode: WorkspaceOpenMode,
  id: string,
  landing?: WorkspaceLanding,
): Promise<void> {
  if (mode === 'in-place') {
    openWorkspace(id, landing);
    return;
  }
  if (isTauri()) {
    await showWorkspaceInNativeWindow(id, landing);
    return;
  }
  if (!openNewTab(workspacePath(id, landing), workspaceTabName(id))) {
    throw new Error('Your browser blocked the new tab. Allow pop-ups for this site.');
  }
}
