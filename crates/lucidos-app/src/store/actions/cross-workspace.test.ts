import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  fetchWorkspaces: vi.fn(),
  listWorkspaces: vi.fn(),
  slugifyWorkspaceName: vi.fn((n: string) =>
    n.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/-+$/, '') || 'workspace',
  ),
  openUrl: vi.fn(),
  showToast: vi.fn(),
  isTauri: vi.fn(() => false),
  windowOpen: vi.fn(),
  focusThreadOrBootstrap: vi.fn(),
  // Mutable per-test stand-in for basePath's load-time `WORKSPACE_ID` const:
  // non-null = this page is served behind the gateway under `/<slug>/`; null =
  // served directly on an engine port. Read via a getter so the SUT sees the
  // current value at call time.
  workspaceId: null as string | null,
}));

vi.mock('../../api/client', () => ({ fetchWorkspaces: mocks.fetchWorkspaces }));
vi.mock('../../api/client/control', () => ({
  listWorkspaces: mocks.listWorkspaces,
  slugifyWorkspaceName: mocks.slugifyWorkspaceName,
}));
vi.mock('../../utils/basePath', () => ({
  get WORKSPACE_ID() {
    return mocks.workspaceId;
  },
}));
vi.mock('./artifacts', () => ({ openUrl: mocks.openUrl }));
vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));
// Stub the heavy threads module — we only need the routing spy, not its chain.
vi.mock('./threads', () => ({ focusThreadOrBootstrap: mocks.focusThreadOrBootstrap }));

vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, showToast: mocks.showToast };
});

const { openThreadInWorkspace, openThreadAcrossWorkspaces, ensureCrossWorkspaceThreadTitle, crossWorkspaceThreadTitle } =
  await import('./cross-workspace');
const { workspaceName } = await import('../store');

const TID = '1c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';

const gwEntry = (
  overrides: Partial<{ id: string; name: string; port: number; health: 'booting' | 'healthy' | 'unhealthy' }>,
) => ({
  id: 'other-ws',
  name: 'other-ws',
  port: 5175,
  health: 'healthy' as 'booting' | 'healthy' | 'unhealthy',
  autostart: true,
  ...overrides,
});

const wsInfo = (overrides: Partial<{ name: string; port: number | null; engine_running: boolean }>) => ({
  name: 'other-ws',
  path: '/tmp/x',
  port: 5175 as number | null,
  engine_running: true,
  engine_version: 'test',
  ...overrides,
});

const stubLocation = (origin: string) => {
  const u = new URL(origin);
  vi.stubGlobal('location', { origin: u.origin, protocol: u.protocol, hostname: u.hostname });
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.isTauri.mockReturnValue(false);
  vi.stubGlobal('window', { ...window, open: mocks.windowOpen });
});

describe('openThreadInWorkspace — behind the gateway', () => {
  beforeEach(() => {
    mocks.workspaceId = 'myws'; // served at https://<gateway>/myws/
    stubLocation('https://localhost:5251');
  });

  it('routes through the gateway by the target slug (not the engine port)', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev', port: 5173 })]);

    await openThreadInWorkspace('dev', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5251/dev/#thread=${TID}`, 'lucidos-ws-dev');
    expect(mocks.fetchWorkspaces).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('resolves the authoritative slug from the workspace name when they differ', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'my-space', name: 'My Space' })]);

    await openThreadInWorkspace('My Space', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5251/my-space/#thread=${TID}`, 'lucidos-ws-My Space');
  });

  it('falls back to slugifying the name when the control plane is unreachable', async () => {
    mocks.listWorkspaces.mockRejectedValue(new Error('no gateway control here'));

    await openThreadInWorkspace('Dev', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5251/dev/#thread=${TID}`, 'lucidos-ws-Dev');
  });

  it('preserves the current host (Tailscale) for the gateway origin', async () => {
    stubLocation('https://tail.host:5251');
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev' })]);

    await openThreadInWorkspace('dev', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://tail.host:5251/dev/#thread=${TID}`, 'lucidos-ws-dev');
  });

  it('uses openUrl (panel) under Tauri', async () => {
    mocks.isTauri.mockReturnValue(true);
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev' })]);

    await openThreadInWorkspace('dev', TID);

    expect(mocks.openUrl).toHaveBeenCalledWith(`https://localhost:5251/dev/#thread=${TID}`);
    expect(mocks.windowOpen).not.toHaveBeenCalled();
  });

  it('toasts when the workspace is not registered with the gateway', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'myws', name: 'myws' })]);

    await openThreadInWorkspace('ghost', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'ghost' is not available"),
      'error',
    );
  });
});

describe('openThreadInWorkspace — served directly on an engine port', () => {
  beforeEach(() => {
    mocks.workspaceId = null; // base '/', no gateway prefix
    stubLocation('https://localhost:5173');
  });

  it("opens the target engine's own port and never touches the gateway", async () => {
    mocks.fetchWorkspaces.mockResolvedValue({
      workspaces: [wsInfo({ name: 'other-ws', port: 5175 }), wsInfo({ name: 'myws', port: 5174 })],
    });

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5175/#thread=${TID}`, 'lucidos-ws-other-ws');
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('keeps the current host (Tailscale) for the dedicated port', async () => {
    stubLocation('https://tail.host:5173');
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'other-ws', port: 5175 })] });

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://tail.host:5175/#thread=${TID}`, 'lucidos-ws-other-ws');
  });

  it('toasts when the workspace is not in the list', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'myws' })] });

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'other-ws' is not available"),
      'error',
    );
  });

  it('toasts when the workspace exists but the engine is not running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'other-ws', engine_running: false })] });

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'other-ws' is not available"),
      'error',
    );
  });

  it('toasts the cause when the workspace-list request fails', async () => {
    mocks.fetchWorkspaces.mockRejectedValue(new Error('boom'));

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to open thread'),
      'error',
    );
  });
});

describe('openThreadAcrossWorkspaces', () => {
  beforeEach(() => {
    workspaceName.value = 'dev';
    mocks.workspaceId = 'dev';
    stubLocation('https://localhost:5251');
  });
  afterEach(() => {
    workspaceName.value = '';
  });

  it('focuses in place for a same-workspace link', () => {
    openThreadAcrossWorkspaces('dev', TID);
    expect(mocks.focusThreadOrBootstrap).toHaveBeenCalledWith(TID);
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(mocks.fetchWorkspaces).not.toHaveBeenCalled();
  });

  it('focuses in place for an untagged link (no workspace tag)', () => {
    openThreadAcrossWorkspaces(undefined, TID);
    expect(mocks.focusThreadOrBootstrap).toHaveBeenCalledWith(TID);
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
  });

  it('hops to the source workspace for a cross-workspace link', () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'other-ws', name: 'other-ws' })]);
    openThreadAcrossWorkspaces('other-ws', TID);
    // openThreadInWorkspace resolves the slug via listWorkspaces synchronously
    // before its first await, so we can assert the routing decision without flushing.
    expect(mocks.focusThreadOrBootstrap).not.toHaveBeenCalled();
    expect(mocks.listWorkspaces).toHaveBeenCalled();
  });
});

describe('ensureCrossWorkspaceThreadTitle', () => {
  it('fetches same-origin through the gateway when behind it', async () => {
    mocks.workspaceId = 'dev';
    stubLocation('https://localhost:5251');
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'myws', name: 'myws' })]);
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ title: 'Gateway name' }) });
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('myws', TID);

    expect(fetchMock).toHaveBeenCalledWith(`https://localhost:5251/myws/api/v1/threads/${TID}`);
    expect(crossWorkspaceThreadTitle('myws', TID)).toBe('Gateway name');
  });

  it('does NOT boot a stopped peer through the gateway just to read a title', async () => {
    const t = '4c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';
    mocks.workspaceId = 'dev';
    stubLocation('https://localhost:5251');
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'asleep', name: 'asleep', health: 'booting' })]);
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('asleep', t);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(crossWorkspaceThreadTitle('asleep', t)).toBeUndefined();
  });

  it("fetches the target engine's port when served directly", async () => {
    const t = '2c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';
    mocks.workspaceId = null;
    stubLocation('https://localhost:5173');
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'dev', port: 5180 })] });
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ title: 'Direct name' }) });
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('dev', t);

    expect(fetchMock).toHaveBeenCalledWith(`https://localhost:5180/api/v1/threads/${t}`);
    expect(crossWorkspaceThreadTitle('dev', t)).toBe('Direct name');
  });

  it('caches nothing (and never throws) when the source workspace is not running', async () => {
    const t = '3c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';
    mocks.workspaceId = null;
    stubLocation('https://localhost:5173');
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'stopped', engine_running: false })] });
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('stopped', t);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(crossWorkspaceThreadTitle('stopped', t)).toBeUndefined();
  });
});
