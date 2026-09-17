import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  fetchWorkspaces: vi.fn(),
  listWorkspaces: vi.fn(),
  locateWorkspace: vi.fn(),
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
  // Same, for the gateway port the engine stamps into the shell. A peer hop
  // compares it against the port the page was actually reached on.
  gatewayPort: 5251 as number | null,
}));

vi.mock('../../api/client', () => ({ fetchWorkspaces: mocks.fetchWorkspaces }));
vi.mock('../../api/client/control', () => ({
  listWorkspaces: mocks.listWorkspaces,
  locateWorkspace: mocks.locateWorkspace,
  slugifyWorkspaceName: mocks.slugifyWorkspaceName,
}));
vi.mock('../../utils/basePath', () => ({
  get WORKSPACE_ID() {
    return mocks.workspaceId;
  },
  get GATEWAY_PORT() {
    return mocks.gatewayPort;
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
  vi.stubGlobal('location', {
    origin: u.origin,
    protocol: u.protocol,
    hostname: u.hostname,
    port: u.port,
  });
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.isTauri.mockReturnValue(false);
  // No other install carries it, which is what every pre-existing case assumes.
  mocks.locateWorkspace.mockResolvedValue(null);
  mocks.gatewayPort = 5251;
  // A tab the browser actually opened. `openNewTab` reads the return value to
  // tell an open from a blocked pop-up. A bare `vi.fn()` returns undefined,
  // which would make every one of these look blocked.
  mocks.windowOpen.mockReturnValue({ closed: false, opener: null });
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

  // The tab is named off the SLUG, never the display name. A workspace row
  // names its own tab the same way. So a thread link and a row reach one tab
  // between them, rather than opening two on the same workspace.
  it('resolves the authoritative slug from the workspace name when they differ', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'my-space', name: 'My Space' })]);

    await openThreadInWorkspace('My Space', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5251/my-space/#thread=${TID}`, 'lucidos-ws-my-space');
  });

  it('falls back to slugifying the name when the control plane is unreachable', async () => {
    mocks.listWorkspaces.mockRejectedValue(new Error('no gateway control here'));

    await openThreadInWorkspace('Dev', TID);

    // The guessed slug names the tab too, so the fallback cannot open a second
    // tab beside the one the resolved path would have found.
    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5251/dev/#thread=${TID}`, 'lucidos-ws-dev');
  });

  // Going through `openNewTab` is what buys this. The raw `window.open` it
  // replaced dropped a blocked pop-up silently, so the link was a dead click.
  it('says so when the browser blocked the tab', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev' })]);
    mocks.windowOpen.mockReturnValue(null);

    await openThreadInWorkspace('dev', TID);

    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringMatching(/blocked/i),
      'error',
    );
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

// A machine can run the packaged app beside a source checkout, each with its
// own gateway, port and registry (ADR 0189). A thread link into the other one
// used to die on "not available" while the workspace ran one port away.
describe('openThreadInWorkspace: a workspace on another install', () => {
  const PEER = {
    status: 'reachable' as const,
    install: 'Lucidos.app in /Applications',
    gateway_port: 5252,
    scheme: 'http',
    slug: 'work',
  };

  beforeEach(() => {
    mocks.workspaceId = 'dev'; // served by the dev gateway on 5251
    mocks.gatewayPort = 5251;
    stubLocation('https://localhost:5251');
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev' })]);
  });

  // The scheme is the peer's, not ours: the packaged gateway serves plain http
  // while this page is on https, so reusing our own would compose a dead URL.
  it("opens the peer gateway's own origin, on the scheme it answered", async () => {
    mocks.locateWorkspace.mockResolvedValue(PEER);

    await openThreadInWorkspace('work', TID);

    expect(mocks.locateWorkspace).toHaveBeenCalledWith('work');
    expect(mocks.windowOpen).toHaveBeenCalledWith(
      `http://localhost:5252/work/#thread=${TID}`,
      'lucidos-ws-5252-work',
    );
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // Two installs may both serve a slug, so a tab named off the slug alone would
  // re-point the tab holding the other one.
  it('names the tab off the peer port, not the slug alone', async () => {
    mocks.locateWorkspace.mockResolvedValue({ ...PEER, slug: 'dev', gateway_port: 5252 });

    await openThreadInWorkspace('their-dev', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(expect.any(String), 'lucidos-ws-5252-dev');
  });

  it('keeps the current host, so a tailnet address stays one', async () => {
    stubLocation('https://tail.host:5251');
    mocks.locateWorkspace.mockResolvedValue(PEER);

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(
      `http://tail.host:5252/work/#thread=${TID}`,
      'lucidos-ws-5252-work',
    );
  });

  // Swapping the port only addresses anything when the page reached this
  // gateway on its own port. Behind `tailscale serve` it did not.
  it('declines to swap the port when this page came through a proxy', async () => {
    stubLocation('https://tail.host'); // 443, fronting 5251
    mocks.locateWorkspace.mockResolvedValue(PEER);

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('cannot reach it'),
      'error',
    );
  });

  it('names the install when its gateway is not running', async () => {
    mocks.locateWorkspace.mockResolvedValue({
      status: 'install-not-running',
      install: 'Lucidos.app in /Applications',
      gateway_port: 5252,
      slug: 'work',
    });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('which is not running'),
      'error',
    );
  });

  it('refuses to guess when two installs carry the name', async () => {
    mocks.locateWorkspace.mockResolvedValue({
      status: 'ambiguous',
      installs: ['Lucidos.app in /Applications', 'install.sh instance "alt"'],
    });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('Open it from the one you mean'),
      'error',
    );
  });

  // Our own gateway answers first. A peer lookup for a workspace it serves
  // would be a wasted request, and could offer a second copy of the same name.
  it('never looks at other installs when our own gateway serves it', async () => {
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'other-ws', name: 'other-ws' })]);

    await openThreadInWorkspace('other-ws', TID);

    expect(mocks.locateWorkspace).not.toHaveBeenCalled();
  });

  it('surfaces the cause when the lookup itself fails', async () => {
    mocks.locateWorkspace.mockRejectedValue(new Error('boom'));

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to open thread'),
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

  // The peer hop belongs to navigation. A popover render must not reach across
  // the machine to another install just to label a link.
  it('never looks across installs for a title', async () => {
    const t = '5c2419a1-aaaa-bbbb-cccc-ddddeeeeffff';
    mocks.workspaceId = 'dev';
    stubLocation('https://localhost:5251');
    mocks.listWorkspaces.mockResolvedValue([gwEntry({ id: 'dev', name: 'dev' })]);
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('work', t);

    expect(mocks.locateWorkspace).not.toHaveBeenCalled();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(crossWorkspaceThreadTitle('work', t)).toBeUndefined();
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
