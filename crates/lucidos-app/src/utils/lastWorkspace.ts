
export const LAST_WORKSPACE_KEY = 'lucidos-last-workspace';

export function rememberLastWorkspace(id: string): void {
  try {
    localStorage.setItem(LAST_WORKSPACE_KEY, id);
  } catch {
  }
}

export function recallLastWorkspace(): string | null {
  try {
    return localStorage.getItem(LAST_WORKSPACE_KEY);
  } catch {
    return null;
  }
}

export function forgetLastWorkspace(): void {
  try {
    localStorage.removeItem(LAST_WORKSPACE_KEY);
  } catch {
  }
}

const MAX_SKELETON_ROWS = 20;

export const LAST_WORKSPACE_COUNT_KEY = 'lucidos-last-workspace-count';

export function rememberLastWorkspaceCount(n: number): void {
  try {
    localStorage.setItem(LAST_WORKSPACE_COUNT_KEY, String(n));
  } catch {
  }
}

export function recallLastWorkspaceCount(): number | null {
  try {
    const raw = localStorage.getItem(LAST_WORKSPACE_COUNT_KEY);
    if (raw === null) return null;
    const n = Number.parseInt(raw, 10);
    if (!Number.isFinite(n) || n <= 0) return null;
    return Math.min(n, MAX_SKELETON_ROWS);
  } catch {
    return null;
  }
}

export const WORKSPACE_SWITCHER_EXPANDED_KEY = 'lucidos-workspace-switcher-expanded';

export function rememberWorkspaceSwitcherExpanded(expanded: boolean): void {
  try {
    localStorage.setItem(WORKSPACE_SWITCHER_EXPANDED_KEY, expanded ? '1' : '0');
  } catch {
  }
}

export function recallWorkspaceSwitcherExpanded(): boolean {
  try {
    return localStorage.getItem(WORKSPACE_SWITCHER_EXPANDED_KEY) === '1';
  } catch {
    return false;
  }
}
