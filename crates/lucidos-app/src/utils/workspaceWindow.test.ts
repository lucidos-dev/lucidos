import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const platform = vi.hoisted(() => ({ isTauri: false, isStandalone: false, isMac: true }));
vi.mock('./platform', () => ({
  isTauri: () => platform.isTauri,
  isStandalone: () => platform.isStandalone,
  get isMac() {
    return platform.isMac;
  },
}));

const showWorkspaceInNativeWindow = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('./tauri', () => ({ showWorkspaceInNativeWindow }));

const openNewTab = vi.hoisted(() => vi.fn(() => true));
vi.mock('./newTab', () => ({ openNewTab }));

const openWorkspace = vi.hoisted(() => vi.fn());
vi.mock('../api/client/control', () => ({ openWorkspace }));

const {
  alternateOpenMode,
  defaultOpenMode,
  middleClickActivates,
  openModeForClick,
  openModeLabel,
  openWorkspaceIn,
  workspacePath,
  workspaceTabName,
} = await import('./workspaceWindow');

function sources(): Array<[string, string]> {
  const root: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const found: Array<[string, string]> = [];
  const walk = (dir: string): void => {
    const entries = readdirSync(dir, { withFileTypes: true }) as
      Array<{ name: string; isDirectory(): boolean }>;
    for (const entry of entries) {
      const full = resolve(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
        found.push([full.slice(root.length + 1), readFileSync(full, 'utf-8')]);
      }
    }
  };
  walk(root);
  return found;
}

const peer = (state: Parameters<typeof alternateOpenMode>[0] = 'healthy') =>
  alternateOpenMode(state, 'dev', 'work');
const current = () => alternateOpenMode('healthy', 'dev', 'dev');
const fromPicker = () => alternateOpenMode('healthy', null, 'work');

const tauri = () => {
  platform.isTauri = true;
};
const pwa = () => {
  platform.isStandalone = true;
};

beforeEach(() => {
  platform.isTauri = false;
  platform.isStandalone = false;
  platform.isMac = true;
  showWorkspaceInNativeWindow.mockClear();
  showWorkspaceInNativeWindow.mockResolvedValue(undefined);
  openNewTab.mockClear();
  openNewTab.mockReturnValue(true);
  openWorkspace.mockClear();
});

describe('the default mode', () => {
  it('is a separate window on the desktop client and a switch everywhere else', () => {
    expect(defaultOpenMode()).toBe('in-place');
    pwa();
    expect(defaultOpenMode()).toBe('in-place');
    tauri();
    expect(defaultOpenMode()).toBe('separate');
  });
});

describe('the alternate mode', () => {
  it('is the other one, so a right-click always offers what a click does not', () => {
    expect(peer()).toBe('separate');
    tauri();
    expect(peer()).toBe('in-place');
  });

  it('is offered on every state a row can be opened in', () => {
    for (const state of ['healthy', 'booting', 'stopped'] as const) {
      expect(peer(state)).toBe('separate');
    }
  });

  it('is withheld from an unhealthy workspace, on every client', () => {
    expect(peer('unhealthy')).toBe(null);
    tauri();
    expect(peer('unhealthy')).toBe(null);
  });

  it('is withheld from an installed PWA, whatever the row', () => {
    pwa();
    expect(peer()).toBe(null);
    expect(peer('stopped')).toBe(null);
  });

  it('is withheld from the current workspace on the desktop client', () => {
    tauri();
    expect(current()).toBe(null);
  });

  it('is kept on the current workspace in a browser', () => {
    expect(current()).toBe('separate');
  });

  it('is withheld on the picker, on the desktop client', () => {
    tauri();
    expect(fromPicker()).toBe(null);
  });

  it('is kept on the picker in a browser', () => {
    expect(fromPicker()).toBe('separate');
  });
});

describe('which rows a middle click may activate', () => {
  it('activates a row that has somewhere to open beside', () => {
    expect(middleClickActivates('healthy')).toBe(true);
    expect(middleClickActivates('stopped')).toBe(true);
  });

  it('never activates an unhealthy row', () => {
    expect(middleClickActivates('unhealthy')).toBe(false);
  });

  it('never activates in an installed PWA', () => {
    pwa();
    expect(middleClickActivates('healthy')).toBe(false);
  });
});

describe('the label', () => {
  it('promises a tab in a browser and a window in the desktop client', () => {
    expect(openModeLabel('separate')).toBe('Open in new tab');
    tauri();
    expect(openModeLabel('separate')).toBe('Open in new window');
  });

  it('names the current window for the desktop alternate', () => {
    tauri();
    expect(openModeLabel('in-place')).toBe('Switch this window');
  });
});

describe('the mode a click asks for', () => {
  it('is the default for a plain click', () => {
    expect(openModeForClick({ button: 0 })).toBe('in-place');
    tauri();
    expect(openModeForClick({ button: 0 })).toBe('separate');
  });

  it('is separate for the accelerator and the middle button', () => {
    expect(openModeForClick({ button: 0, metaKey: true })).toBe('separate');
    expect(openModeForClick({ button: 1 })).toBe('separate');
  });

  it('reads Ctrl on a Mac as no activation, and off a Mac as the accelerator', () => {
    expect(openModeForClick({ button: 0, ctrlKey: true })).toBe(null);
    platform.isMac = false;
    expect(openModeForClick({ button: 0, ctrlKey: true })).toBe('separate');
    expect(openModeForClick({ button: 0, metaKey: true })).toBe('in-place');
  });

  it('keeps the middle button an activation even with Ctrl held on a Mac', () => {
    expect(openModeForClick({ button: 1, ctrlKey: true })).toBe('separate');
  });

  it('reads Shift and Alt as plain', () => {
    expect(openModeForClick({ button: 0, shiftKey: true })).toBe('in-place');
    expect(openModeForClick({ button: 0, altKey: true })).toBe('in-place');
  });

  it('falls back to the default in an installed PWA', () => {
    pwa();
    expect(openModeForClick({ button: 0, metaKey: true })).toBe('in-place');
    expect(openModeForClick({ button: 1 })).toBe('in-place');
  });
});

describe('the path and the tab name', () => {
  it('is the origin-relative path a switch navigates to', () => {
    expect(workspacePath('work')).toBe('/work/');
  });

  it('escapes an id that is not a plain slug', () => {
    expect(workspacePath('a b/c')).toBe('/a%20b%2Fc/');
  });

  it('names one tab per workspace', () => {
    expect(workspaceTabName('work')).toBe('lucidos-ws-work');
    expect(workspaceTabName('dev')).not.toBe(workspaceTabName('work'));
  });

  it('carries the landing fragment, and nothing when there is none', () => {
    expect(workspacePath('work', 'notifications')).toBe('/work/#notifications');
    expect(workspacePath('work', undefined)).toBe('/work/');
  });
});

describe('opening in place', () => {
  it('replaces this document, on every client', async () => {
    await openWorkspaceIn('in-place', 'work');
    expect(openWorkspace).toHaveBeenCalledWith('work', undefined);
    expect(openNewTab).not.toHaveBeenCalled();
    expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();

    tauri();
    openWorkspace.mockClear();
    await openWorkspaceIn('in-place', 'work');
    expect(openWorkspace).toHaveBeenCalledWith('work', undefined);
    expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
  });
});

describe('opening separately', () => {
  it('opens a NAMED tab in a browser and never touches the shell', async () => {
    await openWorkspaceIn('separate', 'work');
    expect(openNewTab).toHaveBeenCalledWith('/work/', 'lucidos-ws-work');
    expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
    expect(openWorkspace).not.toHaveBeenCalled();
  });

  it('asks the shell under Tauri, and never opens a tab', async () => {
    tauri();
    await openWorkspaceIn('separate', 'work');
    expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('work', undefined);
    expect(openNewTab).not.toHaveBeenCalled();
    expect(openWorkspace).not.toHaveBeenCalled();
  });

  it('rejects when the browser blocked the tab', async () => {
    openNewTab.mockReturnValue(false);
    await expect(openWorkspaceIn('separate', 'work')).rejects.toThrow(/blocked/i);
  });

  it('rejects when the shell refused', async () => {
    tauri();
    showWorkspaceInNativeWindow.mockRejectedValue('"nope" is not a workspace');
    await expect(openWorkspaceIn('separate', 'nope')).rejects.toBe('"nope" is not a workspace');
  });
});

describe('the landing', () => {
  it('rides the in-place navigation by name, never as a branch on the value', () => {
    void openWorkspaceIn('in-place', 'work', 'notifications');
    expect(openWorkspace).toHaveBeenCalledWith('work', 'notifications');
  });

  it('rides the named tab as a fragment, in the same tab as a plain row', async () => {
    await openWorkspaceIn('separate', 'work', 'notifications');
    expect(openNewTab).toHaveBeenCalledWith('/work/#notifications', 'lucidos-ws-work');
  });

  it('crosses to the shell as a name', async () => {
    tauri();
    await openWorkspaceIn('separate', 'work', 'notifications');
    expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('work', 'notifications');
  });

  it('leaves the same-window navigation with two callers, both deliberate', () => {
    const imports = /^import[^;]*\bopenWorkspace\b[^;]*from '[^']*client\/control'/m;
    const callers = sources().filter(([, code]) => imports.test(code));
    expect(callers.map(([path]) => path).sort()).toEqual([
      'components/picker/WorkspacePicker.tsx',
      'utils/workspaceWindow.ts',
    ]);
  });

  it('takes its mode from the gesture, never from a literal at the call site', () => {
    const literal = /openWorkspaceIn\(\s*['"`]/;
    const callers = sources().filter(([, code]) => literal.test(code));
    expect(callers.map(([path]) => path)).toEqual([]);
  });

  it('is absent from every mechanism when no row asked for one', async () => {
    await openWorkspaceIn('in-place', 'work');
    expect(openWorkspace).toHaveBeenCalledWith('work', undefined);
    await openWorkspaceIn('separate', 'work');
    expect(openNewTab).toHaveBeenCalledWith('/work/', 'lucidos-ws-work');
    tauri();
    await openWorkspaceIn('separate', 'work');
    expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('work', undefined);
  });
});
