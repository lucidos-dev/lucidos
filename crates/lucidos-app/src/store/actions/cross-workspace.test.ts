import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  fetchWorkspaces: vi.fn(),
  openUrl: vi.fn(),
  showToast: vi.fn(),
  isTauri: vi.fn(() => false),
  windowOpen: vi.fn(),
  focusThreadOrBootstrap: vi.fn(),
}));

vi.mock('../../api/client', () => ({ fetchWorkspaces: mocks.fetchWorkspaces }));
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

const wsInfo = (overrides: Partial<{ name: string; port: number | null; engine_running: boolean }>) => ({
  name: 'work',
  path: '/tmp/x',
  port: 5175 as number | null,
  engine_running: true,
  engine_version: 'test',
  ...overrides,
});

describe('openThreadInWorkspace', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.isTauri.mockReturnValue(false);
    vi.stubGlobal('window', { ...window, open: mocks.windowOpen });
  });

  it('opens a named browser tab for the target workspace when running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({
      workspaces: [wsInfo({ name: 'work', port: 5175 }), wsInfo({ name: 'personal', port: 5174 })],
    });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).toHaveBeenCalledWith(`https://localhost:5175/#thread=${TID}`, 'lucidos-ws-work');
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('uses openUrl (panel) under Tauri', async () => {
    mocks.isTauri.mockReturnValue(true);
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', port: 5175 })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.openUrl).toHaveBeenCalledWith(`https://localhost:5175/#thread=${TID}`);
    expect(mocks.windowOpen).not.toHaveBeenCalled();
  });

  it('shows an error toast when the workspace is not in the list', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'personal' })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' not found"),
      'error',
    );
  });

  it('shows an error toast when the workspace exists but engine is not running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', engine_running: false })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' is not running"),
      'error',
    );
  });

  it('shows an error toast when the workspace has no port assigned', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', port: null })] });

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining("Workspace 'work' is not running"),
      'error',
    );
  });

  it('shows an error toast when the API request fails', async () => {
    mocks.fetchWorkspaces.mockRejectedValue(new Error('boom'));

    await openThreadInWorkspace('work', TID);

    expect(mocks.windowOpen).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to open thread'),
      'error',
    );
  });
});

describe('openThreadAcrossWorkspaces', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    workspaceName.value = 'dev';
  });
  afterEach(() => {
    workspaceName.value = '';
  });

  it('focuses in place for a same-workspace link', () => {
    openThreadAcrossWorkspaces('dev', TID);
    expect(mocks.focusThreadOrBootstrap).toHaveBeenCalledWith(TID);
    expect(mocks.fetchWorkspaces).not.toHaveBeenCalled();
  });

  it('focuses in place for an untagged link (no workspace tag)', () => {
    openThreadAcrossWorkspaces(undefined, TID);
    expect(mocks.focusThreadOrBootstrap).toHaveBeenCalledWith(TID);
    expect(mocks.fetchWorkspaces).not.toHaveBeenCalled();
  });

  it('hops to the source workspace for a cross-workspace link', () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'work', port: 5175 })] });
    openThreadAcrossWorkspaces('work', TID);
    // openThreadInWorkspace invokes fetchWorkspaces synchronously before its
    // first await, so we can assert the routing decision without flushing.
    expect(mocks.focusThreadOrBootstrap).not.toHaveBeenCalled();
    expect(mocks.fetchWorkspaces).toHaveBeenCalled();
  });
});

describe('ensureCrossWorkspaceThreadTitle', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('fetches and caches the title from the source workspace engine', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'dev', port: 5180 })] });
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ title: 'Resolved name' }) });
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('dev', TID);

    expect(fetchMock).toHaveBeenCalledWith(`https://localhost:5180/api/v1/threads/${TID}`);
    expect(crossWorkspaceThreadTitle('dev', TID)).toBe('Resolved name');
  });

  it('caches nothing (and never throws) when the source workspace is not running', async () => {
    mocks.fetchWorkspaces.mockResolvedValue({ workspaces: [wsInfo({ name: 'stopped', engine_running: false })] });
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    await ensureCrossWorkspaceThreadTitle('stopped', TID);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(crossWorkspaceThreadTitle('stopped', TID)).toBeUndefined();
  });
});
