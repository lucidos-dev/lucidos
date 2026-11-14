// @vitest-environment jsdom
/**
 * A remote row in the Lucidos menu's switcher opens the way a local row does.
 *
 * On the packaged desktop client a plain click must reach the shell's window
 * chooser, which opens a new window. A same-window navigation instead turns the
 * window the user clicked from into the remote workspace, and the local one is
 * gone.
 */
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const platform = vi.hoisted(() => ({ isTauri: true, isStandalone: false }));
vi.mock('../../utils/platform', async () => ({
  ...await vi.importActual('../../utils/platform'),
  isTauri: () => platform.isTauri,
  isStandalone: () => platform.isStandalone,
}));
const showWorkspaceInNativeWindow = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', async () => ({
  ...await vi.importActual('../../utils/tauri'),
  showWorkspaceInNativeWindow,
}));
const openWorkspace = vi.hoisted(() => vi.fn());
const openNewTab = vi.hoisted(() => vi.fn(() => true));
vi.mock('../../utils/newTab', () => ({ openNewTab }));
vi.mock('../../utils/basePath', async () => ({
  ...await vi.importActual('../../utils/basePath'), WORKSPACE_ID: 'local', gatewayPickerHref: () => '/~/?pick',
}));
vi.mock('../../api/client/control', async () => ({
  ...await vi.importActual('../../api/client/control'),
  openWorkspace,
  listWorkspacesWithOrder: vi.fn().mockResolvedValue({ workspaces: [{ id: 'local', name: 'Local workspace', health: 'healthy', port: 5000, autostart: false }], order: [] }),
  listRemoteWorkspaces: vi.fn().mockResolvedValue([
    { gateway: 'cloud', state: 'reachable', workspaces: [{ id: 'dev', name: 'dev', health: 'healthy' }] },
  ]),
}));
vi.mock('../shared/Disclosure', () => import('../shared/__tests__/disclosureStub'));

const { WorkspacesMenuRow } = await import('./WorkspaceSwitcher');
const { rememberWorkspaceSwitcherExpanded } = await import('../../utils/lastWorkspace');

const container = document.createElement('div');

afterEach(() => {
  act(() => render(null, container));
  platform.isStandalone = false;
  vi.clearAllMocks();
});

/** Mount the menu row unfolded and hand back the `dev@cloud` row once it lands. */
async function remoteRow(): Promise<HTMLElement> {
  rememberWorkspaceSwitcherExpanded(true);
  await act(async () => { render(<WorkspacesMenuRow onClose={() => {}} />, container); });
  let row: HTMLElement | null = null;
  await vi.waitFor(() => {
    row = container.querySelector<HTMLElement>('a.brand-menu-ws-row[href="/dev@cloud/"]');
    expect(row).not.toBeNull();
  });
  return row!;
}

it('opens a remote workspace in a new window on the desktop client, never in this one', async () => {
  platform.isTauri = true;
  const row = await remoteRow();
  act(() => { row.click(); });
  expect(showWorkspaceInNativeWindow).toHaveBeenCalledWith('dev@cloud', undefined);
  expect(openWorkspace).not.toHaveBeenCalled();
});

it('answers a middle click in a browser with the named tab, not the anchor default', async () => {
  platform.isTauri = false;
  const row = await remoteRow();
  const press = new MouseEvent('auxclick', { button: 1, bubbles: true, cancelable: true });
  act(() => { row.dispatchEvent(press); });
  expect(press.defaultPrevented).toBe(true);
  expect(openNewTab).toHaveBeenCalledWith('/dev@cloud/', 'lucidos-ws-dev@cloud');
  expect(openWorkspace).not.toHaveBeenCalled();
});

it('ignores a middle click in an installed PWA, which has nowhere to open a separate view', async () => {
  platform.isTauri = false;
  platform.isStandalone = true;
  const row = await remoteRow();
  const press = new MouseEvent('auxclick', { button: 1, bubbles: true, cancelable: true });
  act(() => { row.dispatchEvent(press); });
  expect(openWorkspace).not.toHaveBeenCalled();
  expect(openNewTab).not.toHaveBeenCalled();
});

it('keeps the row of the remote workspace this window serves inert', async () => {
  const { workspaceSwitcherList } = await import('./WorkspaceSwitcher');
  const onOpenRemote = vi.fn();
  act(() => {
    render(workspaceSwitcherList({
      state: { status: 'loaded', data: [] },
      currentId: 'dev@cloud',
      manageHref: null,
      onActivate: () => {},
      onNavigate: () => {},
      contextId: null,
      onContext: () => {},
      onAlternate: () => {},
      remote: [{ gateway: 'cloud', workspaces: [{ id: 'dev', name: 'dev', health: 'healthy' }], unreachable: null }],
      remoteError: null,
      order: [],
      onOpenRemote,
    }), container);
  });
  const row = container.querySelector<HTMLElement>('[aria-current="page"]')!;
  expect(row.tagName).toBe('DIV');
  act(() => { row.click(); });
  expect(onOpenRemote).not.toHaveBeenCalled();
});

it('still switches a browser tab in place, as a local row does', async () => {
  platform.isTauri = false;
  const row = await remoteRow();
  act(() => { row.click(); });
  expect(openWorkspace).toHaveBeenCalledWith('dev@cloud', undefined);
  expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
});
