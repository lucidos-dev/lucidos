/**
 * The one place "what does activating a workspace row do" is decided, asserted
 * on all three client shapes.
 *
 * The platform probes are mocked rather than faked through the user-agent: they
 * resolve at module load from the real environment, so a test that poked
 * `navigator` would be asserting on this machine. `isMac` is mocked as a getter
 * because the real module exports it as a const, not a function.
 */
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

/** Every non-test source under `src/`, as `[path relative to src, code]`. The
 *  definition of a production caller for the source scan below. */
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

/** The alternate for a PEER row: this view is on `dev`, the row is `work`. */
const peer = (state: Parameters<typeof alternateOpenMode>[0] = 'healthy') =>
  alternateOpenMode(state, 'dev', 'work');
/** The alternate for the row of the workspace this view is already on. */
const current = () => alternateOpenMode('healthy', 'dev', 'dev');
/** The alternate on the PICKER, which is inside no workspace at all. */
const fromPicker = () => alternateOpenMode('healthy', null, 'work');

/** The packaged desktop client. */
const tauri = () => {
  platform.isTauri = true;
};
/** An installed PWA, iOS or macOS. */
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
    // A stopped workspace lazy-starts behind the gateway's boot splash, which
    // is the good path, so a second view on it is fine.
    for (const state of ['healthy', 'booting', 'stopped'] as const) {
      expect(peer(state)).toBe('separate');
    }
  });

  // A second view on a dead engine is the same dead app shell in a new frame,
  // which `openOrRetry` and the switcher's unhealthy row refuse.
  it('is withheld from an unhealthy workspace, on every client', () => {
    expect(peer('unhealthy')).toBe(null);
    tauri();
    expect(peer('unhealthy')).toBe(null);
  });

  // There `window.open` hands the URL to the browser, so the "separate view" is
  // the user leaving the app. The app popout hides itself for the same reason.
  it('is withheld from an installed PWA, whatever the row', () => {
    pwa();
    expect(peer()).toBe(null);
    expect(peer('stopped')).toBe(null);
  });

  // Switching a window to the workspace it is already on does nothing, so the
  // row would be dead. File > New Window is what serves a second window there.
  it('is withheld from the current workspace on the desktop client', () => {
    tauri();
    expect(current()).toBe(null);
  });

  // A second tab on the workspace you are in is a real want, and unlike the
  // desktop alternate it is not a no-op.
  it('is kept on the current workspace in a browser', () => {
    expect(current()).toBe('separate');
  });

  // The picker is inside no workspace, so the desktop default ALREADY repoints
  // that window. "Switch this window" beside it would be a second affordance
  // on one outcome, and a second window on a workspace that already has one.
  it('is withheld on the picker, on the desktop client', () => {
    tauri();
    expect(fromPicker()).toBe(null);
  });

  // In a browser the picker's alternate is a tab, which is a real second thing.
  it('is kept on the picker in a browser', () => {
    expect(fromPicker()).toBe('separate');
  });
});

describe('which rows a middle click may activate', () => {
  it('activates a row that has somewhere to open beside', () => {
    expect(middleClickActivates('healthy')).toBe(true);
    expect(middleClickActivates('stopped')).toBe(true);
  });

  // Its primary action is a RESTART, so falling through would answer a wheel
  // press by rebooting an engine.
  it('never activates an unhealthy row', () => {
    expect(middleClickActivates('unhealthy')).toBe(false);
  });

  // Nowhere to open beside, so falling back to the primary action would
  // navigate the whole app off a wheel press.
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

  // A macOS Ctrl-click IS the context menu, and some engines dispatch a `click`
  // beside the `contextmenu`. So the row must do NOTHING rather than fall back
  // to its default, which would navigate away from the action row it just
  // unfolded. Off a Mac, Ctrl is the ordinary accelerator.
  it('reads Ctrl on a Mac as no activation, and off a Mac as the accelerator', () => {
    expect(openModeForClick({ button: 0, ctrlKey: true })).toBe(null);
    platform.isMac = false;
    expect(openModeForClick({ button: 0, ctrlKey: true })).toBe('separate');
    // And the accelerator swaps with the platform, rather than being both.
    expect(openModeForClick({ button: 0, metaKey: true })).toBe('in-place');
  });

  // Ctrl is only the context gesture for a PRIMARY press. A ctrl-held wheel
  // press is still a wheel press, and must not be read as a right-click.
  it('keeps the middle button an activation even with Ctrl held on a Mac', () => {
    expect(openModeForClick({ button: 1, ctrlKey: true })).toBe('separate');
  });

  // On a link Shift means a new window and Alt means a download. A row offers
  // neither, so both stay plain.
  it('reads Shift and Alt as plain', () => {
    expect(openModeForClick({ button: 0, shiftKey: true })).toBe('in-place');
    expect(openModeForClick({ button: 0, altKey: true })).toBe('in-place');
  });

  // The gesture is honoured only where a separate view exists at all. Without
  // this a cmd-click in a PWA would eject the user into the browser.
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

  // A slug is `[a-z0-9-]`, so this never fires in practice. It is here because
  // the value reaches an href: encoding it is what keeps that true of a name
  // the gateway one day slugs differently.
  it('escapes an id that is not a plain slug', () => {
    expect(workspacePath('a b/c')).toBe('/a%20b%2Fc/');
  });

  // The reuse key. Two activations of one workspace must name one tab, or the
  // browser opens a second instead of fronting the first.
  it('names one tab per workspace', () => {
    expect(workspaceTabName('work')).toBe('lucidos-ws-work');
    expect(workspaceTabName('dev')).not.toBe(workspaceTabName('work'));
  });

  // A landing rides the same path, so a tab and a switch cannot disagree about
  // where inside the workspace the row was aiming.
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

    // The desktop client's alternate takes this branch too, so it must not go
    // back through the shell and open yet another window.
    tauri();
    openWorkspace.mockClear();
    await openWorkspaceIn('in-place', 'work');
    expect(openWorkspace).toHaveBeenCalledWith('work', undefined);
    expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
  });
});

describe('opening separately', () => {
  // The name is what lets the browser front the tab it already opened for this
  // workspace rather than stack a duplicate.
  it('opens a NAMED tab in a browser and never touches the shell', async () => {
    await openWorkspaceIn('separate', 'work');
    expect(openNewTab).toHaveBeenCalledWith('/work/', 'lucidos-ws-work');
    expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
    expect(openWorkspace).not.toHaveBeenCalled();
  });

  // WKWebView drops `window.open` entirely, so a tab here would be a click that
  // does nothing at all. The shell also owns which window it lands in.
  it('asks the shell under Tauri, and never opens a tab', async () => {
    tauri();
    await openWorkspaceIn('separate', 'work');
    expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('work', undefined);
    expect(openNewTab).not.toHaveBeenCalled();
    expect(openWorkspace).not.toHaveBeenCalled();
  });

  // The caller owes the user a message either way, so neither branch may
  // resolve on a failure.
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

// A notifications row wants the same window rule as every other row PLUS the
// notifications view, so the landing has to survive all three mechanisms. A
// mode that drops it lands the user on the workspace's default view, hunting
// for the bell the row already counted for them.
describe('the landing', () => {
  it('rides the in-place navigation by name, never as a branch on the value', () => {
    // Passed THROUGH, so a landing added to the closed set reaches this
    // mechanism by construction. A branch per landing here would drop the next
    // one silently, with nothing failing to compile.
    void openWorkspaceIn('in-place', 'work', 'notifications');
    expect(openWorkspace).toHaveBeenCalledWith('work', 'notifications');
  });

  it('rides the named tab as a fragment, in the same tab as a plain row', async () => {
    await openWorkspaceIn('separate', 'work', 'notifications');
    expect(openNewTab).toHaveBeenCalledWith('/work/#notifications', 'lucidos-ws-work');
  });

  // By NAME, never as a fragment: the shell composes the URL (ADR 0028).
  it('crosses to the shell as a name', async () => {
    tauri();
    await openWorkspaceIn('separate', 'work', 'notifications');
    expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('work', 'notifications');
  });

  // The whole point of the change this test arrived with: a workspace row must
  // not navigate in place on its own, whatever the client shape. So the
  // same-window navigation keeps a SHORT allow-list. A surface joining it is a
  // surface that built its own rule instead of asking for the mode.
  it('leaves the same-window navigation with two callers, both deliberate', () => {
    // Matched on the IMPORT, so the defining module does not count itself.
    const imports = /^import[^;]*\bopenWorkspace\b[^;]*from '[^']*client\/control'/m;
    const callers = sources().filter(([, code]) => imports.test(code));
    expect(callers.map(([path]) => path).sort()).toEqual([
      // Its auto-open of the remembered workspace, which is not a row
      // activation and has no gesture to read a mode from.
      'components/picker/WorkspacePicker.tsx',
      'utils/workspaceWindow.ts',
    ]);
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
