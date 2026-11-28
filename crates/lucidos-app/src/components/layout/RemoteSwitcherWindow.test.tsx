// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { toasts } from '../../store/store';
import { listRemoteWorkspaces } from '../../api/client/control';

const replaceDocument = vi.hoisted(() => vi.fn());
const showWorkspaceClientMismatch = vi.hoisted(() => vi.fn(async (_mismatch: unknown) => {}));
vi.mock('../../utils/documentNavigation', async (original) => ({
  ...await original<typeof import('../../utils/documentNavigation')>(),
  replaceDocument,
}));
vi.mock('../shared/WorkspaceClientMismatchDialog', () => ({ showWorkspaceClientMismatch }));

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

afterEach(async () => {
  act(() => render(null, container));
  await vi.dynamicImportSettled();
  platform.isStandalone = false;
  vi.unstubAllGlobals();
  toasts.value = [];
  vi.clearAllMocks();
});

async function remoteRow(onClose = vi.fn()): Promise<HTMLElement> {
  rememberWorkspaceSwitcherExpanded(true);
  await act(async () => { render(<WorkspacesMenuRow onClose={onClose} />, container); });
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

it('switches a browser tab in place after the remote answers', async () => {
  platform.isTauri = false;
  vi.stubGlobal('fetch', vi.fn(async () => new Response('ok')));
  const row = await remoteRow();
  act(() => { row.click(); });
  await vi.waitFor(() => expect(replaceDocument).toHaveBeenCalledExactlyOnceWith('/dev@cloud/?preflight=1'));
  expect(showWorkspaceInNativeWindow).not.toHaveBeenCalled();
});

it('keeps the startup navigation available for a booting workspace', async () => {
  platform.isTauri = false;
  vi.mocked(listRemoteWorkspaces).mockResolvedValueOnce([
    { gateway: 'cloud', state: 'reachable', workspaces: [
      { id: 'dev', name: 'dev', health: 'booting' },
    ] },
  ]);
  vi.stubGlobal('fetch', vi.fn(async () => new Response('starting', { status: 503 })));
  const row = await remoteRow();
  act(() => row.click());
  expect(openWorkspace).toHaveBeenCalledExactlyOnceWith('dev@cloud', undefined);
  expect(replaceDocument).not.toHaveBeenCalled();
});

it.each([200, 426, 503])('keeps the menu open during a remote preflight that returns %i', async (status) => {
  platform.isTauri = false;
  let finish: (response: Response) => void = () => {};
  const fetchHealth = vi.fn(() => new Promise<Response>((resolve) => { finish = resolve; }));
  vi.stubGlobal('fetch', fetchHealth);
  const onClose = vi.fn();
  const row = await remoteRow(onClose);
  await vi.dynamicImportSettled();
  act(() => row.click());
  expect(row.textContent).toContain('Opening…');
  expect(row.querySelector('.mini-spinner')).not.toBeNull();
  await vi.dynamicImportSettled();
  expect(fetchHealth).toHaveBeenCalledOnce();
  expect(replaceDocument).not.toHaveBeenCalled();

  const body = status === 426 ? JSON.stringify({
    error: 'workspace_client_mismatch', incompatible: 'dev@cloud', behind: 'this-gateway', ours: 2, theirs: '3-3',
  }) : 'unavailable';
  await act(async () => finish(new Response(body, { status })));
  expect(onClose).not.toHaveBeenCalled();
  if (status === 200) {
    await vi.waitFor(() => expect(replaceDocument).toHaveBeenCalledExactlyOnceWith('/dev@cloud/?preflight=1'));
  } else if (status === 426) {
    await vi.waitFor(() => expect(showWorkspaceClientMismatch).toHaveBeenCalledOnce());
    expect(row.textContent).not.toContain('Opening…');
  } else {
    await vi.waitFor(() => expect(row.getAttribute('aria-disabled')).toBeNull());
    expect(toasts.value[0]?.message).toContain('Could not open dev@cloud');
    expect(row.textContent).not.toContain('Opening…');
  }
});
