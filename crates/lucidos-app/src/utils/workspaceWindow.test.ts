/**
 * The one place "open this workspace somewhere else" is decided, asserted on
 * both platform branches.
 *
 * `isTauri` and `isIOSPwa` are mocked rather than faked through the user-agent:
 * both are resolved at module load from the real environment, so a test that
 * poked `navigator` would be asserting on this machine.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false }));
vi.mock('./platform', () => ({
  isTauri: () => platform.isTauri,
  isIOSPwa: () => platform.isIOSPwa,
}));

const openWorkspaceInNativeWindow = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('./tauri', () => ({ openWorkspaceInNativeWindow }));

const openNewTab = vi.hoisted(() => vi.fn(() => true));
vi.mock('./newTab', () => ({ openNewTab }));

const {
  offersWorkspaceWindow,
  workspaceWindowLabel,
  workspacePath,
  openWorkspaceWindow,
} = await import('./workspaceWindow');

beforeEach(() => {
  platform.isTauri = false;
  platform.isIOSPwa = false;
  openWorkspaceInNativeWindow.mockClear();
  openWorkspaceInNativeWindow.mockResolvedValue(undefined);
  openNewTab.mockClear();
  openNewTab.mockReturnValue(true);
});

describe('the label', () => {
  it('promises a tab in a browser and a window in the desktop client', () => {
    expect(workspaceWindowLabel()).toBe('Open in new tab');
    platform.isTauri = true;
    expect(workspaceWindowLabel()).toBe('Open in new window');
  });
});

describe('which rows offer the action', () => {
  it('offers it on every state a row can be opened in', () => {
    expect(offersWorkspaceWindow('healthy')).toBe(true);
    expect(offersWorkspaceWindow('booting')).toBe(true);
    // A stopped workspace lazy-starts behind the gateway's boot splash, which
    // is the good path, so a new window on it is fine.
    expect(offersWorkspaceWindow('stopped')).toBe(true);
    platform.isTauri = true;
    expect(offersWorkspaceWindow('healthy')).toBe(true);
  });

  // A second window on a dead engine is the same dead app shell in a new
  // frame, which `openOrRetry` and the switcher's unhealthy row refuse.
  it('withholds it from an unhealthy workspace', () => {
    expect(offersWorkspaceWindow('unhealthy')).toBe(false);
  });

  // There `window.open` hands the URL to Safari, so the "second window" is the
  // user leaving the app. The app popout hides itself for the same reason.
  it('withholds it from an installed iOS PWA, whatever the row', () => {
    platform.isIOSPwa = true;
    expect(offersWorkspaceWindow('healthy')).toBe(false);
    expect(offersWorkspaceWindow('stopped')).toBe(false);
  });
});

describe('the path', () => {
  it('is the origin-relative one a switch navigates to', () => {
    expect(workspacePath('work')).toBe('/work/');
  });

  // A slug is `[a-z0-9-]`, so this never fires in practice. It is here because
  // the value reaches an href: encoding it is what keeps that true of a name
  // the gateway one day slugs differently.
  it('escapes an id that is not a plain slug', () => {
    expect(workspacePath('a b/c')).toBe('/a%20b%2Fc/');
  });
});

describe('opening', () => {
  it('opens a tab in a browser and never touches the shell', async () => {
    await openWorkspaceWindow('work');
    expect(openNewTab).toHaveBeenCalledWith('/work/');
    expect(openWorkspaceInNativeWindow).not.toHaveBeenCalled();
  });

  // WKWebView drops `window.open` entirely, so a tab here would be a click that
  // does nothing at all.
  it('asks the shell for a native window under Tauri, and never opens a tab', async () => {
    platform.isTauri = true;
    await openWorkspaceWindow('work');
    expect(openWorkspaceInNativeWindow).toHaveBeenCalledWith('work');
    expect(openNewTab).not.toHaveBeenCalled();
  });

  // The caller owes the user a message either way, so neither branch may
  // resolve on a failure.
  it('rejects when the browser blocked the tab', async () => {
    openNewTab.mockReturnValue(false);
    await expect(openWorkspaceWindow('work')).rejects.toThrow(/blocked/i);
  });

  it('rejects when the shell refused', async () => {
    platform.isTauri = true;
    openWorkspaceInNativeWindow.mockRejectedValue('"nope" is not a workspace');
    await expect(openWorkspaceWindow('nope')).rejects.toBe('"nope" is not a workspace');
  });
});
