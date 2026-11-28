// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const replaceDocument = vi.hoisted(() => vi.fn());
const fetchHealth = vi.hoisted(() => vi.fn<typeof fetch>());
const firstRemote = vi.hoisted(() => ({ health: 'healthy' }));
const showWorkspaceClientMismatch = vi.hoisted(() => vi.fn(async (_mismatch: unknown) => {}));
vi.mock('../../utils/documentNavigation', async (original) => ({
  ...await original<typeof import('../../utils/documentNavigation')>(),
  replaceDocument,
}));
vi.mock('../shared/WorkspaceClientMismatchDialog', () => ({ showWorkspaceClientMismatch }));
vi.mock('../shared/Disclosure', () => import('../shared/__tests__/disclosureStub'));
vi.mock('../../api/client/control', async (original) => ({
  ...await original<typeof import('../../api/client/control')>(),
  listWorkspacesWithOrder: async () => ({ workspaces: [{ id: 'local', name: 'Local', health: 'healthy' }], order: [] }),
  listRemoteWorkspaces: async () => [{
    gateway: 'cloud',
    state: 'reachable',
    workspaces: Array.from({ length: 10 }, (_, i) => ({
      id: `dev-${i}`, name: `Dev ${i}`, health: i === 0 ? firstRemote.health : 'healthy',
    })),
  }],
  getRestoreStatus: async () => ({ status: 'idle' }),
  getGatewayStatus: async () => ({ update_available: false }),
  listTrash: async () => [],
}));

const { WorkspacePicker } = await import('./WorkspacePicker');

const container = document.createElement('div');

afterEach(() => {
  act(() => render(null, container));
  vi.unstubAllGlobals();
  vi.resetAllMocks();
  firstRemote.health = 'healthy';
  localStorage.clear();
});

const mismatchBody = {
  error: 'workspace_client_mismatch',
  incompatible: 'dev-0@cloud',
  behind: 'remote-gateway',
  ours: 2,
  theirs: '1-1',
};

async function mount() {
  vi.stubGlobal('fetch', fetchHealth);
  await act(async () => { render(<WorkspacePicker />, container); });
  await vi.waitFor(() => expect(fetchHealth).toHaveBeenCalledTimes(6));
  const row = container.querySelector<HTMLElement>('[aria-label="Open Dev 0 on cloud"]');
  expect(row).not.toBeNull();
  return row!;
}

it.each([
  ['success', 200, ''],
  ['mismatch', 426, JSON.stringify(mismatchBody)],
  ['failure', 503, 'remote workspace engine is stopped'],
])('keeps the row visible through a pending %s preflight', async (outcome, status, body) => {
  let finish: (response: Response) => void = () => {};
  fetchHealth.mockImplementation(async () => new Response('ok'));
  fetchHealth.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const row = await mount();
  act(() => row.click());
  expect(row.textContent).toContain('Opening…');
  expect(row.querySelector('.mini-spinner')).not.toBeNull();
  expect(replaceDocument).not.toHaveBeenCalled();
  expect(fetchHealth).toHaveBeenCalledTimes(6);

  await act(async () => finish(new Response(body, { status })));
  if (outcome === 'success') {
    await vi.waitFor(() => expect(replaceDocument).toHaveBeenCalledExactlyOnceWith('/dev-0@cloud/?preflight=1'));
    expect(row.textContent).toContain('Opening…');
  } else if (outcome === 'mismatch') {
    await vi.waitFor(() => expect(showWorkspaceClientMismatch).toHaveBeenCalledOnce());
    expect(replaceDocument).not.toHaveBeenCalled();
    expect(row.textContent).not.toContain('Opening…');
  } else {
    await vi.waitFor(() => expect(container.querySelector('.ws-picker-error')?.textContent).toContain('Could not open Dev 0 on cloud'));
    expect(replaceDocument).not.toHaveBeenCalled();
    expect(row.getAttribute('aria-disabled')).toBeNull();
  }
});

it('warms at most six rows and silently drops failed warm-ups', async () => {
  fetchHealth.mockImplementation(async () => new Response('unavailable', { status: 503 }));
  fetchHealth.mockResolvedValueOnce(new Response(JSON.stringify(mismatchBody), { status: 426 }));
  await mount();
  expect(container.querySelector('.ws-picker-error')).toBeNull();
  expect(container.querySelector('.mini-spinner')).toBeNull();
  expect(showWorkspaceClientMismatch).not.toHaveBeenCalled();
});

it('opens the startup page when the listing says the workspace is booting', async () => {
  firstRemote.health = 'booting';
  fetchHealth.mockImplementation(async () => new Response('starting', { status: 503 }));
  const row = await mount();
  act(() => row.click());
  await vi.waitFor(() => expect(replaceDocument).toHaveBeenCalledExactlyOnceWith('/dev-0@cloud/'));
  expect(container.querySelector('.ws-picker-error')).toBeNull();
});
